//! Offline account recovery: `hoglet user list` / `reset-password` logic.

use hoglet::application::{Application, ApplicationConfig};
use hoglet::control::{AccessError, LoginRequest, SetupRequest};
use hoglet::user_admin::{UserAdminError, list_users, reset_password};

const OLD: &str = "correct horse battery staple";
const NEW: &str = "a brand new password!";

async fn started(dir: &std::path::Path) -> Application {
    let application = Application::prepare(ApplicationConfig::new(dir)).await.unwrap();
    application.mark_ready();
    application
}

#[tokio::test]
async fn reset_password_recovers_a_locked_out_owner_only_while_the_server_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let application = started(dir.path()).await;
    let access = application.access();
    let setup = access
        .setup(SetupRequest {
            email: "owner@example.com".into(),
            password: OLD.into(),
            organization_name: "Acme".into(),
            project_name: "Site".into(),
            existing_project_token: None,
        })
        .await
        .unwrap();

    // Listing is read-only and works against a running server.
    let users = list_users(dir.path()).unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].email, "owner@example.com");
    assert_eq!(users[0].memberships, vec![("Acme".to_owned(), "owner".to_owned())]);

    // Refused while the server holds the data directory; nothing changed.
    assert!(matches!(
        reset_password(dir.path(), "owner@example.com", NEW),
        Err(UserAdminError::ServerRunning)
    ));
    assert!(
        access
            .login(LoginRequest {
                email: "owner@example.com".into(),
                password: OLD.into()
            })
            .await
            .is_ok()
    );

    drop(access);
    application.shutdown().await.unwrap();

    // Stopped: weak passwords and unknown accounts are refused, a good reset works.
    assert!(matches!(
        reset_password(dir.path(), "owner@example.com", "short"),
        Err(UserAdminError::WeakPassword)
    ));
    assert!(matches!(
        reset_password(dir.path(), "nobody@example.com", NEW),
        Err(UserAdminError::UnknownUser)
    ));
    let ended = reset_password(dir.path(), " Owner@Example.com ", NEW).unwrap();
    assert!(ended >= 1);

    let application = started(dir.path()).await;
    let access = application.access();
    // The old session and password are dead; the new password signs in.
    assert!(matches!(
        access.validate_session(&setup.session_id).await,
        Err(AccessError::Unauthorized)
    ));
    assert!(matches!(
        access
            .login(LoginRequest {
                email: "owner@example.com".into(),
                password: OLD.into()
            })
            .await,
        Err(AccessError::InvalidCredentials)
    ));
    let login = access
        .login(LoginRequest {
            email: "owner@example.com".into(),
            password: NEW.into(),
        })
        .await
        .unwrap();
    assert_eq!(login.workspace.organizations[0].role.as_str(), "owner");
    drop(access);
    application.shutdown().await.unwrap();
}

#[test]
fn a_directory_without_data_is_reported_not_created() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(list_users(dir.path()), Err(UserAdminError::NoData)));
    assert!(matches!(
        reset_password(dir.path(), "a@b.co", NEW),
        Err(UserAdminError::NoData)
    ));
    assert!(!dir.path().join("control.db").exists());
}
