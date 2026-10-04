//! Operator tools for accounts: `hoglet user list` and
//! `hoglet user reset-password`. They open the data directory directly, so a
//! locked-out owner can be recovered with shell access and nothing else.
//!
//! Resetting refuses while a server holds the data directory (the WAL writer
//! lock), so it can never race a running instance.

use std::fs::OpenOptions;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::control::{hash_password, validate_password};

/// The lock a running server holds for the lifetime of its WAL writer.
const SERVER_LOCK: &str = "wal/.writer.lock";

#[derive(Debug)]
pub enum UserAdminError {
    /// No `control.db` at the data directory.
    NoData,
    /// A server is running on this data directory.
    ServerRunning,
    UnknownUser,
    WeakPassword,
    Storage(String),
}

impl std::fmt::Display for UserAdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoData => f.write_str("no Hoglet data found there (control.db is missing)"),
            Self::ServerRunning => f.write_str(
                "a Hoglet server is running on this data directory; stop it first, then retry",
            ),
            Self::UnknownUser => f.write_str("no account with that email"),
            Self::WeakPassword => f.write_str("the password must be 12 to 1024 characters"),
            Self::Storage(message) => write!(f, "could not use the account database: {message}"),
        }
    }
}

impl std::error::Error for UserAdminError {}

impl From<rusqlite::Error> for UserAdminError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    pub email: String,
    pub name: String,
    /// `(organization name, role)`.
    pub memberships: Vec<(String, String)>,
}

fn open(data_dir: &Path, flags: OpenFlags) -> Result<Connection, UserAdminError> {
    let path = data_dir.join("control.db");
    if !path.is_file() {
        return Err(UserAdminError::NoData);
    }
    let connection = Connection::open_with_flags(&path, flags)?;
    let application_id: i64 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if application_id != crate::storage_bootstrap::CONTROL_APPLICATION_ID {
        return Err(UserAdminError::Storage(
            "control.db is not a Hoglet control database".into(),
        ));
    }
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(connection)
}

/// Every account with its organizations and roles. Read-only; safe while the
/// server runs.
pub fn list_users(data_dir: &Path) -> Result<Vec<UserSummary>, UserAdminError> {
    let connection = open(data_dir, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut users =
        connection.prepare("SELECT id,email,name FROM users ORDER BY created_at,email")?;
    let mut memberships = connection.prepare(
        "SELECT o.name,m.role FROM organization_members m
         JOIN organizations o ON o.id=m.organization_id
         WHERE m.user_id=?1 ORDER BY o.created_at,o.id",
    )?;
    let rows = users
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, email, name) in rows {
        let memberships = memberships
            .query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        out.push(UserSummary {
            email,
            name,
            memberships,
        });
    }
    Ok(out)
}

/// Set a new password for `email` and end all of that account's sessions.
/// Personal API keys are left alone (revoke them in the dashboard).
/// Returns the number of sessions ended.
pub fn reset_password(
    data_dir: &Path,
    email: &str,
    new_password: &str,
) -> Result<usize, UserAdminError> {
    validate_password(new_password).map_err(|_| UserAdminError::WeakPassword)?;
    // Hold the server lock for the whole operation. A missing lock file means
    // no server has ever written here.
    let lock_path = data_dir.join(SERVER_LOCK);
    let _lock = if lock_path.is_file() {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| UserAdminError::Storage(error.to_string()))?;
        match lock.try_lock() {
            Ok(()) => Some(lock),
            Err(std::fs::TryLockError::WouldBlock) => return Err(UserAdminError::ServerRunning),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(UserAdminError::Storage(error.to_string()));
            }
        }
    } else {
        None
    };
    let mut connection = open(
        data_dir,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let email = email.trim().to_ascii_lowercase();
    let user_id: String = connection
        .query_row("SELECT id FROM users WHERE email=?1", [&email], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or(UserAdminError::UnknownUser)?;
    let hash = hash_password(new_password).map_err(|_| UserAdminError::WeakPassword)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "UPDATE users SET password_hash=?2 WHERE id=?1",
        rusqlite::params![user_id, hash],
    )?;
    let ended = transaction.execute("DELETE FROM auth_sessions WHERE user_id=?1", [&user_id])?;
    transaction.commit()?;
    Ok(ended)
}
