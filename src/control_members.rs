//! Team membership: members, role changes, and invites without an email server.
//!
//! Everything here runs on the single control worker (`control.rs`), so each
//! operation is one SQLite transaction and cannot interleave with another.
//!
//! Role rules, enforced here and nowhere else:
//! - `owner`: everything, including inviting, promoting and removing owners.
//! - `admin`: invites, role changes and removals for admins and members; can
//!   never create, change or remove an owner.
//! - `member`: reads the member list, may leave. Nothing else.
//! - An organization always keeps at least one owner.
//! - Every change needs a session; a personal key (even a write key) cannot
//!   manage people or mint invite links.
//!
//! An invite token is 256 random bits, shown once, stored as SHA-256 and
//! looked up by that hash (no secret-dependent comparison happens in Rust or
//! in a loop over rows). It is single use, expires after 7 days, and an
//! organization holds at most `MAX_PENDING_INVITES`.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::contract::members::{CreatedInvite, Invite, InvitePreview, Member};
use crate::control::{
    AccessError, Command, Principal, ProjectAccess, Role, SetupResult, hash_password,
    random_secret, require_session, start_session, validate_email, validate_name,
    validate_password, verify_password,
};

/// Seven days.
pub const INVITE_TTL_SECONDS: i64 = 7 * 24 * 3600;
/// Pending (unused, unexpired) invites one organization may hold.
pub const MAX_PENDING_INVITES: i64 = 100;
/// Members one organization may hold.
pub const MAX_MEMBERS: i64 = 500;
const TOKEN_PREFIX: &str = "hgi_";
const TOKEN_LEN: usize = TOKEN_PREFIX.len() + 64;

/// A request that is valid but contradicts current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// Would leave the organization without an owner.
    LastOwner,
    AlreadyMember,
    TooManyInvites,
    TooManyMembers,
    /// Someone registered the invited address while the invite was open.
    EmailInUse,
}

impl Conflict {
    pub fn code(self) -> &'static str {
        match self {
            Self::LastOwner => "last_owner",
            Self::AlreadyMember => "already_member",
            Self::TooManyInvites => "too_many_invites",
            Self::TooManyMembers => "too_many_members",
            Self::EmailInUse => "email_in_use",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::LastOwner => "An organization needs at least one owner.",
            Self::AlreadyMember => "This person is already a member of the organization.",
            Self::TooManyInvites => "Too many pending invites. Revoke some first.",
            Self::TooManyMembers => "The organization has reached its member limit.",
            Self::EmailInUse => "An account with this email was created in the meantime.",
        }
    }
}

#[derive(Debug)]
pub enum MemberError {
    Access(AccessError),
    Conflict(Conflict),
    /// Unknown, expired, used or revoked: deliberately indistinguishable.
    InvalidInvite,
    /// A field the caller can fix; the message says which.
    Invalid(&'static str),
}

impl From<AccessError> for MemberError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<rusqlite::Error> for MemberError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Access(AccessError::Database(error))
    }
}

impl std::fmt::Display for MemberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Access(error) => error.fmt(f),
            Self::Conflict(conflict) => f.write_str(conflict.message()),
            Self::InvalidInvite => f.write_str("invalid, expired or used invite"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for MemberError {}

/// What the worker knows about an invite, plus the invited address's account.
pub struct InviteLookup {
    pub preview: InvitePreview,
    /// `(user_id, password_hash)` when an account with the invited email exists.
    pub account: Option<(String, String)>,
}

pub enum AcceptAccount {
    Existing { user_id: String },
    New { name: String, password_hash: String },
}

/// Whether `actor` may change a member who is `target` to `new_role`
/// (`None` = remove). Pure policy; last-owner protection is separate.
pub fn may_change(actor: Role, target: Role, new_role: Option<Role>) -> bool {
    match actor {
        Role::Owner => true,
        Role::Admin => target != Role::Owner && new_role != Some(Role::Owner),
        Role::Member => false,
    }
}

fn token_hash(token: &str) -> Option<String> {
    let well_formed = token.len() == TOKEN_LEN
        && token.starts_with(TOKEN_PREFIX)
        && token[TOKEN_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    well_formed.then(|| hex::encode(Sha256::digest(token.as_bytes())))
}

/// Trimmed, lower-cased, one address, no whitespace or control characters.
fn normalize_email(email: &str) -> Result<String, MemberError> {
    let email = email.trim().to_ascii_lowercase();
    validate_email(&email).map_err(|_| MemberError::Invalid("Enter a valid email address."))?;
    let (local, domain) = email
        .split_once('@')
        .ok_or(MemberError::Invalid("Enter a valid email address."))?;
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(MemberError::Invalid("Enter a valid email address."));
    }
    Ok(email)
}

fn member_role(
    connection: &Connection,
    organization_id: &str,
    user_id: &str,
) -> Result<Option<Role>, MemberError> {
    let role: Option<String> = connection
        .query_row(
            "SELECT role FROM organization_members WHERE organization_id=?1 AND user_id=?2",
            params![organization_id, user_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(role.as_deref().map(Role::parse).transpose()?)
}

/// The caller's role in the organization; a non-member (or an unknown
/// organization) is `Forbidden`, so ids cannot be probed.
fn actor_role(
    connection: &Connection,
    principal: &Principal,
    organization_id: &str,
) -> Result<Role, MemberError> {
    member_role(connection, organization_id, &principal.user_id)?
        .ok_or(MemberError::Access(AccessError::Forbidden))
}

fn require_manager(role: Role) -> Result<(), MemberError> {
    match role {
        Role::Owner | Role::Admin => Ok(()),
        Role::Member => Err(MemberError::Access(AccessError::Forbidden)),
    }
}

fn owner_count(connection: &Connection, organization_id: &str) -> Result<i64, MemberError> {
    Ok(connection.query_row(
        "SELECT count(*) FROM organization_members WHERE organization_id=?1 AND role='owner'",
        [organization_id],
        |row| row.get(0),
    )?)
}

fn read_member(
    connection: &Connection,
    organization_id: &str,
    user_id: &str,
) -> Result<Option<Member>, MemberError> {
    Ok(connection
        .query_row(
            MEMBER_SELECT_ONE,
            params![organization_id, user_id],
            member_row,
        )
        .optional()?)
}

const MEMBER_COLUMNS: &str = "u.id,u.name,u.email,m.role,
     CASE WHEN m.joined_at>0 THEN m.joined_at ELSE u.created_at/1000 END";
const MEMBER_SELECT_ONE: &str = "SELECT u.id,u.name,u.email,m.role,
     CASE WHEN m.joined_at>0 THEN m.joined_at ELSE u.created_at/1000 END
     FROM organization_members m JOIN users u ON u.id=m.user_id
     WHERE m.organization_id=?1 AND m.user_id=?2";

fn member_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Member> {
    let role: String = row.get(3)?;
    Ok(Member {
        user_id: row.get(0)?,
        name: row.get(1)?,
        email: row.get(2)?,
        role: Role::parse(&role).map_err(|_| rusqlite::Error::InvalidQuery)?,
        joined_at: row.get(4)?,
    })
}

fn invite_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Invite> {
    let role: String = row.get(2)?;
    Ok(Invite {
        id: row.get(0)?,
        email: row.get(1)?,
        role: Role::parse(&role).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_by: row.get(3)?,
        created_at: row.get(4)?,
        expires_at: row.get(5)?,
    })
}

// ---------------------------------------------------------------------------
// Worker-side operations
// ---------------------------------------------------------------------------

pub(crate) fn list_members(
    connection: &Connection,
    principal: &Principal,
    organization_id: &str,
) -> Result<Vec<Member>, MemberError> {
    actor_role(connection, principal, organization_id)?;
    let mut statement = connection.prepare(&format!(
        "SELECT {MEMBER_COLUMNS}
         FROM organization_members m JOIN users u ON u.id=m.user_id
         WHERE m.organization_id=?1 LIMIT ?2"
    ))?;
    let mut members = statement
        .query_map(params![organization_id, MAX_MEMBERS], member_row)?
        .collect::<Result<Vec<_>, _>>()?;
    members.sort_by(|a, b| {
        rank(a.role)
            .cmp(&rank(b.role))
            .then(a.joined_at.cmp(&b.joined_at))
            .then(a.email.cmp(&b.email))
    });
    Ok(members)
}

fn rank(role: Role) -> u8 {
    match role {
        Role::Owner => 0,
        Role::Admin => 1,
        Role::Member => 2,
    }
}

pub(crate) fn update_member_role(
    connection: &mut Connection,
    principal: &Principal,
    organization_id: &str,
    user_id: &str,
    new_role: Role,
) -> Result<Member, MemberError> {
    require_session(principal)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actor = actor_role(&transaction, principal, organization_id)?;
    require_manager(actor)?;
    let target = member_role(&transaction, organization_id, user_id)?
        .ok_or(MemberError::Access(AccessError::NotFound))?;
    if !may_change(actor, target, Some(new_role)) {
        return Err(MemberError::Access(AccessError::Forbidden));
    }
    if target != new_role {
        if target == Role::Owner && owner_count(&transaction, organization_id)? <= 1 {
            return Err(MemberError::Conflict(Conflict::LastOwner));
        }
        transaction.execute(
            "UPDATE organization_members SET role=?3 WHERE organization_id=?1 AND user_id=?2",
            params![organization_id, user_id, new_role.as_str()],
        )?;
    }
    let member = read_member(&transaction, organization_id, user_id)?
        .ok_or(MemberError::Access(AccessError::NotFound))?;
    transaction.commit()?;
    if target != new_role {
        tracing::info!(
            target: "hoglet::audit",
            actor = %principal.user_id,
            organization = %organization_id,
            member = %user_id,
            from = target.as_str(),
            to = new_role.as_str(),
            "member role changed"
        );
    }
    Ok(member)
}

pub(crate) fn remove_member(
    connection: &mut Connection,
    principal: &Principal,
    organization_id: &str,
    user_id: &str,
) -> Result<(), MemberError> {
    require_session(principal)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actor = actor_role(&transaction, principal, organization_id)?;
    let leaving = principal.user_id == user_id;
    if !leaving {
        require_manager(actor)?;
    }
    let target = member_role(&transaction, organization_id, user_id)?
        .ok_or(MemberError::Access(AccessError::NotFound))?;
    if !leaving && !may_change(actor, target, None) {
        return Err(MemberError::Access(AccessError::Forbidden));
    }
    if target == Role::Owner && owner_count(&transaction, organization_id)? <= 1 {
        return Err(MemberError::Conflict(Conflict::LastOwner));
    }
    transaction.execute(
        "DELETE FROM organization_members WHERE organization_id=?1 AND user_id=?2",
        params![organization_id, user_id],
    )?;
    let remaining: i64 = transaction.query_row(
        "SELECT count(*) FROM organization_members WHERE user_id=?1",
        [user_id],
        |row| row.get(0),
    )?;
    // Access is re-checked against membership on every request, so removal is
    // immediate either way. Credentials go too: sessions always (someone else
    // removed them), keys and (when leaving) sessions once no organization
    // is left to use them for.
    if !leaving || remaining == 0 {
        transaction.execute("DELETE FROM auth_sessions WHERE user_id=?1", [user_id])?;
    }
    if remaining == 0 {
        transaction.execute("DELETE FROM personal_api_keys WHERE user_id=?1", [user_id])?;
    }
    transaction.commit()?;
    tracing::info!(
        target: "hoglet::audit",
        actor = %principal.user_id,
        organization = %organization_id,
        member = %user_id,
        left = leaving,
        "member removed"
    );
    Ok(())
}

fn purge_expired(connection: &Connection, organization_id: &str, now: i64) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM organization_invites WHERE organization_id=?1 AND expires_at<=?2",
        params![organization_id, now],
    )?;
    Ok(())
}

pub(crate) fn create_invite(
    connection: &mut Connection,
    principal: &Principal,
    organization_id: &str,
    email: &str,
    role: Role,
) -> Result<CreatedInvite, MemberError> {
    require_session(principal)?;
    let email = normalize_email(email)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actor = actor_role(&transaction, principal, organization_id)?;
    require_manager(actor)?;
    if role == Role::Owner && actor != Role::Owner {
        return Err(MemberError::Access(AccessError::Forbidden));
    }
    let already_member: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM organization_members m JOIN users u ON u.id=m.user_id
                       WHERE m.organization_id=?1 AND u.email=?2)",
        params![organization_id, email],
        |row| row.get(0),
    )?;
    if already_member {
        return Err(MemberError::Conflict(Conflict::AlreadyMember));
    }
    let now = Utc::now().timestamp();
    purge_expired(&transaction, organization_id, now)?;
    // Inviting the same address again replaces the old link.
    transaction.execute(
        "DELETE FROM organization_invites WHERE organization_id=?1 AND email=?2",
        params![organization_id, email],
    )?;
    let pending: i64 = transaction.query_row(
        "SELECT count(*) FROM organization_invites WHERE organization_id=?1",
        [organization_id],
        |row| row.get(0),
    )?;
    if pending >= MAX_PENDING_INVITES {
        return Err(MemberError::Conflict(Conflict::TooManyInvites));
    }
    let token = format!("{TOKEN_PREFIX}{}", random_secret());
    let hash = token_hash(&token).ok_or(MemberError::Access(AccessError::Unavailable))?;
    let invite = Invite {
        id: Uuid::new_v4().to_string(),
        email,
        role,
        created_by: principal.user_id.clone(),
        created_at: now,
        expires_at: now + INVITE_TTL_SECONDS,
    };
    transaction.execute(
        "INSERT INTO organization_invites
             (id,organization_id,email,role,token_hash,created_by,created_at,expires_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            invite.id,
            organization_id,
            invite.email,
            role.as_str(),
            hash,
            invite.created_by,
            invite.created_at,
            invite.expires_at
        ],
    )?;
    transaction.commit()?;
    tracing::info!(
        target: "hoglet::audit",
        actor = %principal.user_id,
        organization = %organization_id,
        invite = %invite.id,
        role = role.as_str(),
        "invite created"
    );
    let path = format!("/invite/{token}");
    Ok(CreatedInvite {
        invite,
        token,
        path,
    })
}

pub(crate) fn list_invites(
    connection: &Connection,
    principal: &Principal,
    organization_id: &str,
) -> Result<Vec<Invite>, MemberError> {
    require_manager(actor_role(connection, principal, organization_id)?)?;
    let now = Utc::now().timestamp();
    let mut statement = connection.prepare(
        "SELECT id,email,role,created_by,created_at,expires_at FROM organization_invites
         WHERE organization_id=?1 AND expires_at>?2 ORDER BY created_at DESC,id LIMIT ?3",
    )?;
    Ok(statement
        .query_map(params![organization_id, now, MAX_PENDING_INVITES], invite_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn revoke_invite(
    connection: &Connection,
    principal: &Principal,
    organization_id: &str,
    invite_id: &str,
) -> Result<(), MemberError> {
    require_session(principal)?;
    let actor = actor_role(connection, principal, organization_id)?;
    require_manager(actor)?;
    let role: Option<String> = connection
        .query_row(
            "SELECT role FROM organization_invites WHERE id=?1 AND organization_id=?2",
            params![invite_id, organization_id],
            |row| row.get(0),
        )
        .optional()?;
    let role = Role::parse(&role.ok_or(MemberError::Access(AccessError::NotFound))?)?;
    if role == Role::Owner && actor != Role::Owner {
        return Err(MemberError::Access(AccessError::Forbidden));
    }
    connection.execute(
        "DELETE FROM organization_invites WHERE id=?1 AND organization_id=?2",
        params![invite_id, organization_id],
    )?;
    tracing::info!(
        target: "hoglet::audit",
        actor = %principal.user_id,
        organization = %organization_id,
        invite = %invite_id,
        "invite revoked"
    );
    Ok(())
}

pub(crate) fn invite_lookup(
    connection: &Connection,
    hash: &str,
) -> Result<InviteLookup, MemberError> {
    let now = Utc::now().timestamp();
    let found: Option<(String, String, String, i64)> = connection
        .query_row(
            "SELECT o.name,i.email,i.role,i.expires_at
             FROM organization_invites i JOIN organizations o ON o.id=i.organization_id
             WHERE i.token_hash=?1",
            [hash],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let (organization_name, email, role, expires_at) = found.ok_or(MemberError::InvalidInvite)?;
    if expires_at <= now {
        connection.execute(
            "DELETE FROM organization_invites WHERE token_hash=?1",
            [hash],
        )?;
        return Err(MemberError::InvalidInvite);
    }
    let account: Option<(String, String)> = connection
        .query_row(
            "SELECT id,password_hash FROM users WHERE email=?1",
            [&email],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(InviteLookup {
        preview: InvitePreview {
            organization_name,
            email,
            role: Role::parse(&role)?,
            expires_at,
            account_exists: account.is_some(),
        },
        account,
    })
}

pub(crate) fn accept_invite(
    connection: &mut Connection,
    hash: &str,
    account: AcceptAccount,
) -> Result<SetupResult, MemberError> {
    let now = Utc::now().timestamp();
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found: Option<(String, String, String, String, i64)> = transaction
        .query_row(
            "SELECT id,organization_id,email,role,expires_at FROM organization_invites
             WHERE token_hash=?1",
            [hash],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let (invite_id, organization_id, email, role, expires_at) =
        found.ok_or(MemberError::InvalidInvite)?;
    if expires_at <= now {
        return Err(MemberError::InvalidInvite);
    }
    let role = Role::parse(&role)?;
    let members: i64 = transaction.query_row(
        "SELECT count(*) FROM organization_members WHERE organization_id=?1",
        [&organization_id],
        |row| row.get(0),
    )?;
    if members >= MAX_MEMBERS {
        return Err(MemberError::Conflict(Conflict::TooManyMembers));
    }
    let user_id = match account {
        AcceptAccount::Existing { user_id } => {
            let matches: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND email=?2)",
                params![user_id, email],
                |row| row.get(0),
            )?;
            if !matches {
                return Err(MemberError::InvalidInvite);
            }
            user_id
        }
        AcceptAccount::New {
            name,
            password_hash,
        } => {
            let taken: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE email=?1)",
                [&email],
                |row| row.get(0),
            )?;
            if taken {
                return Err(MemberError::Conflict(Conflict::EmailInUse));
            }
            let user_id = Uuid::new_v4().to_string();
            transaction.execute(
                "INSERT INTO users(id,email,password_hash,name,created_at) VALUES (?1,?2,?3,?4,?5)",
                params![
                    user_id,
                    email,
                    password_hash,
                    name,
                    Utc::now().timestamp_millis()
                ],
            )?;
            user_id
        }
    };
    if member_role(&transaction, &organization_id, &user_id)?.is_some() {
        transaction.execute("DELETE FROM organization_invites WHERE id=?1", [&invite_id])?;
        transaction.commit()?;
        return Err(MemberError::Conflict(Conflict::AlreadyMember));
    }
    transaction.execute(
        "INSERT INTO organization_members(organization_id,user_id,role,joined_at)
         VALUES (?1,?2,?3,?4)",
        params![organization_id, user_id, role.as_str(), now],
    )?;
    transaction.execute("DELETE FROM organization_invites WHERE id=?1", [&invite_id])?;
    transaction.commit()?;
    tracing::info!(
        target: "hoglet::audit",
        actor = %user_id,
        organization = %organization_id,
        invite = %invite_id,
        role = role.as_str(),
        "invite accepted"
    );
    Ok(start_session(connection, &user_id)?)
}

// ---------------------------------------------------------------------------
// Async facade
// ---------------------------------------------------------------------------

impl ProjectAccess {
    async fn member_command<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, MemberError>>) -> Command,
    ) -> Result<T, MemberError> {
        let (response, receive) = oneshot::channel();
        self.tx
            .send(build(response))
            .await
            .map_err(|_| MemberError::Access(AccessError::Unavailable))?;
        receive
            .await
            .map_err(|_| MemberError::Access(AccessError::Unavailable))?
    }

    pub async fn list_members(
        &self,
        principal: &Principal,
        organization_id: &str,
    ) -> Result<Vec<Member>, MemberError> {
        self.member_command(|response| Command::ListMembers {
            principal: principal.clone(),
            organization_id: organization_id.into(),
            response,
        })
        .await
    }

    pub async fn update_member_role(
        &self,
        principal: &Principal,
        organization_id: &str,
        user_id: &str,
        role: Role,
    ) -> Result<Member, MemberError> {
        self.member_command(|response| Command::UpdateMemberRole {
            principal: principal.clone(),
            organization_id: organization_id.into(),
            user_id: user_id.into(),
            role,
            response,
        })
        .await
    }

    pub async fn remove_member(
        &self,
        principal: &Principal,
        organization_id: &str,
        user_id: &str,
    ) -> Result<(), MemberError> {
        self.member_command(|response| Command::RemoveMember {
            principal: principal.clone(),
            organization_id: organization_id.into(),
            user_id: user_id.into(),
            response,
        })
        .await
    }

    pub async fn create_invite(
        &self,
        principal: &Principal,
        organization_id: &str,
        email: &str,
        role: Role,
    ) -> Result<CreatedInvite, MemberError> {
        self.member_command(|response| Command::CreateInvite {
            principal: principal.clone(),
            organization_id: organization_id.into(),
            email: email.into(),
            role,
            response,
        })
        .await
    }

    pub async fn list_invites(
        &self,
        principal: &Principal,
        organization_id: &str,
    ) -> Result<Vec<Invite>, MemberError> {
        self.member_command(|response| Command::ListInvites {
            principal: principal.clone(),
            organization_id: organization_id.into(),
            response,
        })
        .await
    }

    pub async fn revoke_invite(
        &self,
        principal: &Principal,
        organization_id: &str,
        invite_id: &str,
    ) -> Result<(), MemberError> {
        self.member_command(|response| Command::RevokeInvite {
            principal: principal.clone(),
            organization_id: organization_id.into(),
            invite_id: invite_id.into(),
            response,
        })
        .await
    }

    async fn lookup_invite(&self, hash: String) -> Result<InviteLookup, MemberError> {
        self.member_command(|response| Command::InviteLookup {
            token_hash: hash,
            response,
        })
        .await
    }

    /// What an invite link is for. Malformed, unknown, expired and used tokens
    /// all answer `InvalidInvite`.
    pub async fn invite_preview(&self, token: &str) -> Result<InvitePreview, MemberError> {
        let hash = token_hash(token).ok_or(MemberError::InvalidInvite)?;
        Ok(self.lookup_invite(hash).await?.preview)
    }

    /// Join the organization with an invite. A new address gets an account
    /// (`name`, `password` chosen now); an address that already has one must
    /// present its current password, like a sign-in. Opens a session.
    pub async fn accept_invite(
        &self,
        token: &str,
        name: Option<&str>,
        password: &str,
    ) -> Result<SetupResult, MemberError> {
        let hash = token_hash(token).ok_or(MemberError::InvalidInvite)?;
        let lookup = self.lookup_invite(hash.clone()).await?;
        let account = match lookup.account {
            Some((user_id, stored_hash)) => {
                let password = password.to_owned();
                let verified = self
                    .run_hashing(move || verify_password(&stored_hash, &password))
                    .await?;
                verified?;
                AcceptAccount::Existing { user_id }
            }
            None => {
                let name = name.unwrap_or_default().trim();
                validate_name(name).map_err(|_| MemberError::Invalid("Enter your name."))?;
                validate_password(password).map_err(|_| {
                    MemberError::Invalid("The password must be at least 12 characters.")
                })?;
                let password = password.to_owned();
                let password_hash = self.run_hashing(move || hash_password(&password)).await??;
                AcceptAccount::New {
                    name: name.to_owned(),
                    password_hash,
                }
            }
        };
        self.member_command(|response| Command::AcceptInvite {
            token_hash: hash,
            account,
            response,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_matrix() {
        use Role::*;
        for target in [Owner, Admin, Member] {
            for new in [None, Some(Owner), Some(Admin), Some(Member)] {
                assert!(may_change(Owner, target, new));
                assert!(!may_change(Member, target, new));
            }
        }
        assert!(!may_change(Admin, Owner, None));
        assert!(!may_change(Admin, Owner, Some(Member)));
        assert!(!may_change(Admin, Member, Some(Owner)));
        assert!(!may_change(Admin, Admin, Some(Owner)));
        assert!(may_change(Admin, Admin, Some(Member)));
        assert!(may_change(Admin, Member, Some(Admin)));
        assert!(may_change(Admin, Member, None));
    }

    #[test]
    fn token_shape_is_checked_before_any_lookup() {
        let good = format!("hgi_{}", "ab".repeat(32));
        assert!(token_hash(&good).is_some());
        assert_eq!(token_hash(&good), token_hash(&good));
        for bad in [
            "",
            "hgi_",
            "phx_abababababababababababababababababababababababababababababababab",
            &format!("hgi_{}", "ab".repeat(31)),
            &format!("hgi_{}", "zz".repeat(32)),
            &format!("{good}x"),
        ] {
            assert!(token_hash(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn invite_emails_are_normalized_and_strict() {
        assert_eq!(normalize_email("  Ann@Example.COM ").unwrap(), "ann@example.com");
        for bad in ["", "no-at", "@x.co", "a@", "a@b@c", "a b@c.co", "a\u{0}@b.co"] {
            assert!(normalize_email(bad).is_err(), "{bad:?}");
        }
    }
}
