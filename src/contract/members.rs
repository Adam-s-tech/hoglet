//! Team members and invites (organization scope).
//!
//! Roles are per organization: `owner` (everything, including owners),
//! `admin` (projects, flags, keys, members except owners) and `member`
//! (read-only). Invites need no email server: the creator receives a
//! one-time link, shown once and stored only as a hash.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::control::Role;

/// One person in an organization.
/// `GET /api/organizations/{org_id}/members` → `Vec<Member>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct Member {
    pub user_id: String,
    pub name: String,
    pub email: String,
    pub role: Role,
    /// Unix seconds.
    #[ts(type = "number")]
    pub joined_at: i64,
}

/// A pending invite. The token itself is never listed.
/// `GET /api/organizations/{org_id}/invites` → `Vec<Invite>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct Invite {
    pub id: String,
    pub email: String,
    pub role: Role,
    /// User id of the member who created it.
    pub created_by: String,
    #[ts(type = "number")]
    pub created_at: i64,
    /// Unix seconds; 7 days after creation.
    #[ts(type = "number")]
    pub expires_at: i64,
}

/// `POST /api/organizations/{org_id}/invites`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct CreateInviteRequest {
    pub email: String,
    pub role: Role,
}

/// Answer to `POST …/invites` (201). The only time the token is visible.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct CreatedInvite {
    pub invite: Invite,
    /// One-time secret. Single use, expires with the invite.
    pub token: String,
    /// Dashboard path the invitee opens: `/invite/{token}`. Prefix it with the
    /// address people use to reach Hoglet.
    pub path: String,
}

/// `PATCH /api/organizations/{org_id}/members/{user_id}`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct UpdateMemberRequest {
    pub role: Role,
}

/// `POST /api/invites/preview`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct InviteTokenRequest {
    pub token: String,
}

/// What the invitee sees before accepting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct InvitePreview {
    pub organization_name: String,
    pub email: String,
    pub role: Role,
    #[ts(type = "number")]
    pub expires_at: i64,
    /// An account with this email exists: accepting means signing in.
    pub account_exists: bool,
}

/// `POST /api/invites/accept`. `name` is used only when the account is
/// created; `password` is the new password (at least 12 characters) or, for
/// an existing account, its current one. Answers with the invitee's
/// `Workspace` and a session cookie, like login.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../web/src/types/")]
pub struct AcceptInviteRequest {
    pub token: String,
    #[serde(default)]
    pub name: Option<String>,
    pub password: String,
}
