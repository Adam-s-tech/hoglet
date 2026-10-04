//! Team members and invites, end to end against the real composed
//! application: invite → accept → role-enforced reads and writes → role
//! changes → removal → credential revocation, plus last-owner protection,
//! token lifecycle, tenant isolation and throttling.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use hoglet::application::{Application, ApplicationConfig};
use hoglet::security::PeerAddr;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple";
const MEMBER_PASSWORD: &str = "member password long enough";

struct Team {
    application: Application,
    router: Router,
    dir: tempfile::TempDir,
    owner: String,
    org: String,
    project: String,
}

#[derive(Clone, Default)]
struct Auth {
    cookie: Option<String>,
    bearer: Option<String>,
    peer: Option<std::net::IpAddr>,
}

impl Auth {
    fn cookie(cookie: &str) -> Self {
        Self {
            cookie: Some(cookie.to_owned()),
            ..Self::default()
        }
    }
    fn bearer(secret: &str) -> Self {
        Self {
            bearer: Some(secret.to_owned()),
            ..Self::default()
        }
    }
    fn from(mut self, ip: &str) -> Self {
        self.peer = Some(ip.parse().unwrap());
        self
    }
}

async fn call(
    router: &Router,
    method: Method,
    uri: &str,
    auth: &Auth,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = &auth.cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(secret) = &auth.bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {secret}"));
    }
    let mut request = match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .unwrap();
    if let Some(ip) = auth.peer {
        request.extensions_mut().insert(PeerAddr(ip));
    }
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, value)
}

fn session_cookie(headers: &axum::http::HeaderMap) -> String {
    headers
        .get(header::SET_COOKIE)
        .expect("a session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

impl Team {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let application = Application::prepare(ApplicationConfig::new(dir.path()))
            .await
            .unwrap();
        application.mark_ready();
        let router = application.router();
        let (status, headers, workspace) = call(
            &router,
            Method::POST,
            "/api/auth/setup",
            &Auth::default(),
            Some(json!({
                "email": "owner@example.com", "password": PASSWORD,
                "organization_name": "Acme", "project_name": "Site"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        Self {
            owner: session_cookie(&headers),
            org: workspace["organizations"][0]["id"].as_str().unwrap().into(),
            project: workspace["organizations"][0]["projects"][0]["id"]
                .as_str()
                .unwrap()
                .into(),
            application,
            router,
            dir,
        }
    }

    async fn finish(self) {
        drop(self.router);
        self.application.shutdown().await.unwrap();
    }

    fn owner(&self) -> Auth {
        Auth::cookie(&self.owner)
    }

    async fn api(
        &self,
        method: Method,
        uri: &str,
        auth: &Auth,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (status, _, value) = call(&self.router, method, uri, auth, body).await;
        (status, value)
    }

    fn members_uri(&self) -> String {
        format!("/api/organizations/{}/members", self.org)
    }

    fn invites_uri(&self) -> String {
        format!("/api/organizations/{}/invites", self.org)
    }

    fn project_uri(&self, tail: &str) -> String {
        format!("/api/projects/{}/{tail}", self.project)
    }

    /// Create an invite as `by`; returns `(status, body)`.
    async fn invite(&self, by: &Auth, email: &str, role: &str) -> (StatusCode, Value) {
        self.api(
            Method::POST,
            &self.invites_uri(),
            by,
            Some(json!({"email": email, "role": role})),
        )
        .await
    }

    /// Invite and accept as a brand-new person; returns their session auth and user id.
    async fn join(&self, by: &Auth, email: &str, role: &str) -> (Auth, String) {
        let (status, created) = self.invite(by, email, role).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let token = created["token"].as_str().unwrap();
        let (status, headers, workspace) = call(
            &self.router,
            Method::POST,
            "/api/invites/accept",
            &Auth::default(),
            Some(json!({"token": token, "name": email, "password": MEMBER_PASSWORD})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{workspace}");
        (
            Auth::cookie(&session_cookie(&headers)),
            workspace["user"]["id"].as_str().unwrap().into(),
        )
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.path().join("control.db")).unwrap()
    }

    async fn role_of(&self, user: &str) -> Option<String> {
        let (_, members) = self
            .api(Method::GET, &self.members_uri(), &self.owner(), None)
            .await;
        members
            .as_array()
            .unwrap()
            .iter()
            .find(|member| member["user_id"] == user)
            .map(|member| member["role"].as_str().unwrap().to_owned())
    }
}

fn insight_body() -> Value {
    json!({"name": "SQL", "query_ir": {"kind": "SqlQuery", "query": "select 1 as x"}})
}

#[tokio::test]
async fn invite_accept_then_roles_gate_every_write() {
    let team = Team::start().await;
    let owner = team.owner();

    // Objects created by the owner, so members can be refused changing them.
    let (status, insight) = team
        .api(Method::POST, &team.project_uri("insights"), &owner, Some(insight_body()))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{insight}");
    let (_, dashboard) = team
        .api(Method::POST, &team.project_uri("dashboards"), &owner, Some(json!({"name": "D"})))
        .await;
    let (status, flag) = team
        .api(Method::POST, &team.project_uri("feature_flags"), &owner, Some(json!({"key": "beta"})))
        .await;
    assert!(status.is_success(), "{flag}");
    let (_, share) = team
        .api(
            Method::POST,
            &team.project_uri("shares"),
            &owner,
            Some(json!({"object_type": "insight", "object_id": insight["id"]})),
        )
        .await;

    let (member, member_id) = team.join(&owner, "Ann@Example.com", "member").await;

    // Joined with the invited role; the workspace shows it.
    let (_, me) = team.api(Method::GET, "/api/auth/me", &member, None).await;
    assert_eq!(me["user"]["email"], "ann@example.com");
    assert_eq!(me["organizations"][0]["role"], "member");
    assert_eq!(me["organizations"][0]["projects"][0]["id"], team.project.as_str());

    // Reads work.
    for tail in ["insights", "dashboards", "shares", "feature_flags", "persons", "events", "status", "forwarding"] {
        let (status, body) = team
            .api(Method::GET, &team.project_uri(tail), &member, None)
            .await;
        assert_eq!(status, StatusCode::OK, "member read {tail}: {body}");
    }
    let (status, _) = team.api(Method::GET, &team.members_uri(), &member, None).await;
    assert_eq!(status, StatusCode::OK);

    // Every write is refused with 403, whatever it targets.
    let insight_id = insight["id"].as_str().unwrap();
    let dashboard_id = dashboard["id"].as_str().unwrap();
    let share_id = share["id"].as_str().unwrap();
    let flag_id = flag["id"].as_i64().unwrap();
    let writes: Vec<(Method, String, Option<Value>)> = vec![
        (Method::POST, team.project_uri("insights"), Some(insight_body())),
        (Method::PUT, team.project_uri(&format!("insights/{insight_id}")), Some(insight_body())),
        (Method::DELETE, team.project_uri(&format!("insights/{insight_id}")), None),
        (Method::POST, team.project_uri("dashboards"), Some(json!({"name": "x"}))),
        (Method::PUT, team.project_uri(&format!("dashboards/{dashboard_id}")), Some(json!({"name": "y"}))),
        (Method::PUT, team.project_uri(&format!("dashboards/{dashboard_id}/tiles")), Some(json!([]))),
        (Method::DELETE, team.project_uri(&format!("dashboards/{dashboard_id}")), None),
        (Method::POST, team.project_uri("shares"), Some(json!({"object_type": "insight", "object_id": insight_id}))),
        (Method::DELETE, team.project_uri(&format!("shares/{share_id}")), None),
        (Method::POST, team.project_uri("feature_flags"), Some(json!({"key": "nope"}))),
        (Method::PATCH, team.project_uri(&format!("feature_flags/{flag_id}")), Some(json!({"active": false}))),
        (Method::DELETE, team.project_uri(&format!("feature_flags/{flag_id}")), None),
        (Method::PUT, team.project_uri("forwarding"), Some(json!({"enabled": false, "host": "", "posthog_token": ""}))),
        (Method::POST, team.project_uri("demo"), None),
        (Method::POST, team.project_uri("persons/someone/erase"), None),
        (Method::POST, format!("/api/organizations/{}/projects", team.org), Some(json!({"name": "P2"}))),
        (Method::POST, team.invites_uri(), Some(json!({"email": "x@example.com", "role": "member"}))),
        (Method::GET, team.invites_uri(), None),
        (Method::PATCH, format!("{}/{member_id}", team.members_uri()), Some(json!({"role": "admin"}))),
    ];
    for (method, uri, body) in &writes {
        let (status, body) = team.api(method.clone(), uri, &member, body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "member {method} {uri} -> {body}");
    }
    // Removing another member is also refused; the owner is still there.
    let (owner_id, _) = {
        let (_, members) = team.api(Method::GET, &team.members_uri(), &owner, None).await;
        let owner_row = members.as_array().unwrap().iter().find(|m| m["role"] == "owner").unwrap().clone();
        (owner_row["user_id"].as_str().unwrap().to_owned(), ())
    };
    let (status, _) = team
        .api(Method::DELETE, &format!("{}/{owner_id}", team.members_uri()), &member, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Nothing changed.
    let (_, flags) = team.api(Method::GET, &team.project_uri("feature_flags"), &owner, None).await;
    assert_eq!(flags.as_array().unwrap().len(), 1);
    let (_, insights) = team.api(Method::GET, &team.project_uri("insights"), &owner, None).await;
    assert_eq!(insights.as_array().unwrap().len(), 1);

    // A member's own keys are bounded by the member role: a write-scoped key
    // is still refused, and a key cannot manage people at all.
    let (status, key) = team
        .api(Method::POST, "/api/auth/keys", &member, Some(json!({"name": "ci", "scope": "write"})))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let key_secret = key["secret"].as_str().unwrap().to_owned();
    let key_auth = Auth::bearer(&key_secret);
    let (status, _) = team
        .api(Method::POST, &team.project_uri("feature_flags"), &key_auth, Some(json!({"key": "viakey"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = team.api(Method::GET, &team.project_uri("insights"), &key_auth, None).await;
    assert_eq!(status, StatusCode::OK);

    // Promote to admin: writes now work; the key follows the role.
    let (status, promoted) = team
        .api(Method::PATCH, &format!("{}/{member_id}", team.members_uri()), &owner, Some(json!({"role": "admin"})))
        .await;
    assert_eq!(status, StatusCode::OK, "{promoted}");
    assert_eq!(promoted["role"], "admin");
    let (status, body) = team
        .api(Method::POST, &team.project_uri("feature_flags"), &member, Some(json!({"key": "by_admin"})))
        .await;
    assert!(status.is_success(), "{body}");
    let (status, _) = team
        .api(Method::POST, &team.project_uri("feature_flags"), &key_auth, Some(json!({"key": "by_admin_key"})))
        .await;
    assert!(status.is_success());

    // Demote: writes are refused again immediately.
    let (status, _) = team
        .api(Method::PATCH, &format!("{}/{member_id}", team.members_uri()), &owner, Some(json!({"role": "member"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = team
        .api(Method::POST, &team.project_uri("feature_flags"), &member, Some(json!({"key": "again"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Remove: sessions and personal keys stop working at once.
    let (status, _) = team
        .api(Method::DELETE, &format!("{}/{member_id}", team.members_uri()), &owner, None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(team.role_of(&member_id).await, None);
    let (status, _) = team.api(Method::GET, "/api/auth/me", &member, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = team.api(Method::GET, &team.project_uri("insights"), &key_auth, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let remaining: i64 = team
        .db()
        .query_row("SELECT count(*) FROM personal_api_keys WHERE user_id=?1", [&member_id], |r| r.get(0))
        .unwrap();
    assert_eq!(remaining, 0);

    team.finish().await;
}

#[tokio::test]
async fn admins_manage_members_but_never_owners() {
    let team = Team::start().await;
    let owner = team.owner();
    let (admin, admin_id) = team.join(&owner, "admin@example.com", "admin").await;
    let (_member, member_id) = team.join(&owner, "m@example.com", "member").await;
    let (_, members) = team.api(Method::GET, &team.members_uri(), &owner, None).await;
    let owner_id = members[0]["user_id"].as_str().unwrap().to_owned();
    assert_eq!(members[0]["role"], "owner");
    assert_eq!(members.as_array().unwrap().len(), 3);

    // Admin invites members and admins, not owners.
    assert_eq!(team.invite(&admin, "new1@example.com", "member").await.0, StatusCode::CREATED);
    assert_eq!(team.invite(&admin, "new2@example.com", "admin").await.0, StatusCode::CREATED);
    assert_eq!(team.invite(&admin, "new3@example.com", "owner").await.0, StatusCode::FORBIDDEN);
    // An owner invite made by the owner cannot be revoked by an admin.
    let (_, owner_invite) = team.invite(&owner, "boss@example.com", "owner").await;
    let (status, _) = team
        .api(Method::DELETE, &format!("{}/{}", team.invites_uri(), owner_invite["invite"]["id"].as_str().unwrap()), &admin, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin can change a member's role up to admin, never to owner, and never touch the owner.
    let member_uri = format!("{}/{member_id}", team.members_uri());
    let patch = |role: &str| Some(json!({ "role": role }));
    assert_eq!(team.api(Method::PATCH, &member_uri, &admin, patch("admin")).await.0, StatusCode::OK);
    assert_eq!(team.api(Method::PATCH, &member_uri, &admin, patch("owner")).await.0, StatusCode::FORBIDDEN);
    let owner_uri = format!("{}/{owner_id}", team.members_uri());
    assert_eq!(team.api(Method::PATCH, &owner_uri, &admin, patch("member")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(team.api(Method::DELETE, &owner_uri, &admin, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(team.role_of(&owner_id).await.as_deref(), Some("owner"));
    // Admin cannot promote themselves.
    let own_uri = format!("{}/{admin_id}", team.members_uri());
    assert_eq!(team.api(Method::PATCH, &own_uri, &admin, patch("owner")).await.0, StatusCode::FORBIDDEN);
    // Admin removes a member.
    assert_eq!(team.api(Method::DELETE, &member_uri, &admin, None).await.0, StatusCode::NO_CONTENT);
    // Unknown member id: not found, not a crash.
    let ghost = format!("{}/{}", team.members_uri(), uuid::Uuid::new_v4());
    assert_eq!(team.api(Method::DELETE, &ghost, &owner, None).await.0, StatusCode::NOT_FOUND);

    team.finish().await;
}

#[tokio::test]
async fn an_organization_always_keeps_an_owner() {
    let team = Team::start().await;
    let owner = team.owner();
    let (_, members) = team.api(Method::GET, &team.members_uri(), &owner, None).await;
    let owner_id = members[0]["user_id"].as_str().unwrap().to_owned();
    let uri = format!("{}/{owner_id}", team.members_uri());

    let (status, body) = team.api(Method::PATCH, &uri, &owner, Some(json!({"role": "admin"}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "last_owner");
    let (status, body) = team.api(Method::DELETE, &uri, &owner, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "last_owner");

    // With a second owner, the first may step down or leave; the second then cannot.
    let (second, second_id) = team.join(&owner, "second@example.com", "owner").await;
    assert_eq!(team.api(Method::PATCH, &uri, &owner, Some(json!({"role": "admin"}))).await.0, StatusCode::OK);
    let second_uri = format!("{}/{second_id}", team.members_uri());
    assert_eq!(team.api(Method::PATCH, &second_uri, &second, Some(json!({"role": "member"}))).await.0, StatusCode::CONFLICT);
    assert_eq!(team.api(Method::DELETE, &second_uri, &second, None).await.0, StatusCode::CONFLICT);
    // The demoted first owner (now admin) cannot touch the remaining owner.
    assert_eq!(team.api(Method::DELETE, &second_uri, &owner, None).await.0, StatusCode::FORBIDDEN);

    team.finish().await;
}

#[tokio::test]
async fn members_may_leave_and_lose_their_credentials() {
    let team = Team::start().await;
    let (member, member_id) = team.join(&team.owner(), "leaver@example.com", "member").await;
    let uri = format!("{}/{member_id}", team.members_uri());
    assert_eq!(team.api(Method::DELETE, &uri, &member, None).await.0, StatusCode::NO_CONTENT);
    // Their account has no organization left, so the session is gone too.
    assert_eq!(team.api(Method::GET, "/api/auth/me", &member, None).await.0, StatusCode::UNAUTHORIZED);
    team.finish().await;
}

#[tokio::test]
async fn invite_tokens_are_hashed_single_use_expiring_and_revocable() {
    let team = Team::start().await;
    let owner = team.owner();

    let (status, created) = team.invite(&owner, "new@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let token = created["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("hgi_") && token.len() == 68);
    assert_eq!(created["path"], format!("/invite/{token}"));
    assert_eq!(
        created["invite"]["expires_at"].as_i64().unwrap() - created["invite"]["created_at"].as_i64().unwrap(),
        7 * 24 * 3600
    );

    // Stored only as a hash; listed without the token.
    let stored: String = team
        .db()
        .query_row("SELECT token_hash FROM organization_invites", [], |r| r.get(0))
        .unwrap();
    assert_ne!(stored, token);
    assert!(!stored.contains(&token));
    assert_eq!(stored.len(), 64);
    let (_, listed) = team.api(Method::GET, &team.invites_uri(), &owner, None).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains(&token));
    assert_eq!(listed[0]["email"], "new@example.com");

    // Preview: org, role, and that no account exists. No session needed.
    let anon = Auth::default();
    let (status, preview) = team
        .api(Method::POST, "/api/invites/preview", &anon, Some(json!({"token": token})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["organization_name"], "Acme");
    assert_eq!(preview["role"], "member");
    assert_eq!(preview["account_exists"], false);

    // Re-inviting the same address replaces the link.
    let (_, again) = team.invite(&owner, "NEW@example.com", "admin").await;
    let token2 = again["token"].as_str().unwrap().to_owned();
    assert_ne!(token, token2);
    let (status, _) = team
        .api(Method::POST, "/api/invites/preview", &anon, Some(json!({"token": token})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, listed) = team.api(Method::GET, &team.invites_uri(), &owner, None).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);

    // Accept: weak password and missing name are refused without burning the invite.
    let accept = |name: Value, password: &str| {
        Some(json!({"token": token2, "name": name, "password": password}))
    };
    let (status, body) = team
        .api(Method::POST, "/api/invites/accept", &anon, accept(json!("New Person"), "short"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("12"));
    let (status, _) = team
        .api(Method::POST, "/api/invites/accept", &anon, accept(Value::Null, MEMBER_PASSWORD))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, headers, workspace) = call(
        &team.router,
        Method::POST,
        "/api/invites/accept",
        &anon,
        accept(json!("New Person"), MEMBER_PASSWORD),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{workspace}");
    assert_eq!(workspace["user"]["email"], "new@example.com");
    assert_eq!(workspace["organizations"][0]["role"], "admin");
    let cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));

    // Used: gone for preview and accept, and they can sign in with the password.
    for uri in ["/api/invites/preview", "/api/invites/accept"] {
        let (status, body) = team
            .api(Method::POST, uri, &anon, Some(json!({"token": token2, "name": "x", "password": MEMBER_PASSWORD})))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(body["error"]["code"], "invite_invalid");
    }
    let (status, _) = team
        .api(Method::POST, "/api/auth/login", &anon, Some(json!({"email": "new@example.com", "password": MEMBER_PASSWORD})))
        .await;
    assert_eq!(status, StatusCode::OK);

    // Already a member: a fresh invite is a conflict.
    let (status, body) = team.invite(&owner, "new@example.com", "member").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "already_member");

    // Revoked.
    let (_, third) = team.invite(&owner, "gone@example.com", "member").await;
    let third_token = third["token"].as_str().unwrap().to_owned();
    let revoke = format!("{}/{}", team.invites_uri(), third["invite"]["id"].as_str().unwrap());
    assert_eq!(team.api(Method::DELETE, &revoke, &owner, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(team.api(Method::DELETE, &revoke, &owner, None).await.0, StatusCode::NOT_FOUND);
    let (status, _) = team
        .api(Method::POST, "/api/invites/accept", &anon, Some(json!({"token": third_token, "name": "x", "password": MEMBER_PASSWORD})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Expired (clock moved past 7 days).
    let (_, fourth) = team.invite(&owner, "late@example.com", "member").await;
    let fourth_token = fourth["token"].as_str().unwrap().to_owned();
    team.db()
        .execute("UPDATE organization_invites SET expires_at=?1", [chrono::Utc::now().timestamp() - 1])
        .unwrap();
    let (status, _) = team
        .api(Method::POST, "/api/invites/accept", &anon, Some(json!({"token": fourth_token, "name": "x", "password": MEMBER_PASSWORD})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, listed) = team.api(Method::GET, &team.invites_uri(), &owner, None).await;
    assert!(listed.as_array().unwrap().is_empty());

    // Garbage tokens are just "invalid", with no parsing surprises.
    for bad in ["", "hgi_", "hgi_zz", "../../etc/passwd", &"a".repeat(5000)] {
        let (status, _) = team
            .api(Method::POST, "/api/invites/preview", &anon, Some(json!({"token": bad})))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{bad:?}");
    }
    let (status, _) = team
        .api(Method::POST, "/api/invites/preview", &anon, Some(json!({"nope": 1})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    team.finish().await;
}

#[tokio::test]
async fn invite_input_and_limits() {
    let team = Team::start().await;
    let owner = team.owner();
    for bad in ["", "no-at-sign", "a b@example.com", "x@"] {
        assert_eq!(team.invite(&owner, bad, "member").await.0, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    let (status, _) = team
        .api(Method::POST, &team.invites_uri(), &owner, Some(json!({"email": "a@example.com", "role": "superuser"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A bounded number of pending invites.
    for index in 0..100 {
        let (status, body) = team.invite(&owner, &format!("p{index}@example.com"), "member").await;
        assert_eq!(status, StatusCode::CREATED, "{index}: {body}");
    }
    let (status, body) = team.invite(&owner, "p100@example.com", "member").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "too_many_invites");
    // Re-issuing an existing address still works at the limit.
    assert_eq!(team.invite(&owner, "p5@example.com", "member").await.0, StatusCode::CREATED);

    team.finish().await;
}

#[tokio::test]
async fn organizations_are_isolated_from_each_other() {
    let team = Team::start().await;
    let owner = team.owner();
    // `outsider` joins Acme, creates their own organization, then leaves Acme.
    let (outsider, outsider_id) = team.join(&owner, "outsider@example.com", "admin").await;
    let (status, other_org) = team
        .api(Method::POST, "/api/organizations", &outsider, Some(json!({"name": "Other"})))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let other_org_id = other_org["id"].as_str().unwrap().to_owned();
    let (_, other_project) = team
        .api(
            Method::POST,
            &format!("/api/organizations/{other_org_id}/projects"),
            &outsider,
            Some(json!({"name": "Theirs"})),
        )
        .await;
    let other_project_id = other_project["id"].as_str().unwrap().to_owned();
    assert_eq!(
        team.api(Method::DELETE, &format!("{}/{outsider_id}", team.members_uri()), &owner, None).await.0,
        StatusCode::NO_CONTENT
    );
    // Removing from one organization does not log them out of another.
    let (outsider2, _) = {
        let (status, headers, _) = call(
            &team.router,
            Method::POST,
            "/api/auth/login",
            &Auth::default(),
            Some(json!({"email": "outsider@example.com", "password": MEMBER_PASSWORD})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        (Auth::cookie(&session_cookie(&headers)), ())
    };
    let (_, me) = team.api(Method::GET, "/api/auth/me", &outsider2, None).await;
    assert_eq!(me["organizations"].as_array().unwrap().len(), 1);
    assert_eq!(me["organizations"][0]["name"], "Other");

    // The outsider sees nothing of Acme.
    let acme_invite = team.invite(&owner, "keep@example.com", "member").await.1;
    for (method, uri) in [
        (Method::GET, team.members_uri()),
        (Method::GET, team.invites_uri()),
        (Method::GET, team.project_uri("insights")),
        (Method::GET, team.project_uri("persons")),
    ] {
        let (status, _) = team.api(method.clone(), &uri, &outsider2, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    assert_eq!(team.invite(&outsider2, "evil@example.com", "member").await.0, StatusCode::FORBIDDEN);
    let revoke = format!("{}/{}", team.invites_uri(), acme_invite["invite"]["id"].as_str().unwrap());
    assert_eq!(team.api(Method::DELETE, &revoke, &outsider2, None).await.0, StatusCode::FORBIDDEN);

    // And Acme's owner sees nothing of Other, even through the invite and member routes.
    let other_members = format!("/api/organizations/{other_org_id}/members");
    assert_eq!(team.api(Method::GET, &other_members, &owner, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        team.api(
            Method::POST,
            &format!("/api/organizations/{other_org_id}/invites"),
            &owner,
            Some(json!({"email": "x@example.com", "role": "member"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        team.api(Method::GET, &format!("/api/projects/{other_project_id}/insights"), &owner, None).await.0,
        StatusCode::FORBIDDEN
    );
    // A member id from another organization is not addressable through this one.
    let acme_owner_uri = format!("{}/{outsider_id}", other_members);
    assert_eq!(team.api(Method::DELETE, &acme_owner_uri, &owner, None).await.0, StatusCode::FORBIDDEN);
    // Malformed organization id.
    assert_eq!(team.api(Method::GET, "/api/organizations/nope/members", &owner, None).await.0, StatusCode::NOT_FOUND);

    team.finish().await;
}

#[tokio::test]
async fn existing_accounts_sign_in_to_accept_and_share_the_login_throttle() {
    let team = Team::start().await;
    let owner = team.owner();
    // `guest` exists (member of Acme) and is invited to a second organization.
    let (_guest, _) = team.join(&owner, "guest@example.com", "member").await;
    let (_, second) = team
        .api(Method::POST, "/api/organizations", &owner, Some(json!({"name": "Second"})))
        .await;
    let second_org = second["id"].as_str().unwrap();
    let (status, created) = team
        .api(
            Method::POST,
            &format!("/api/organizations/{second_org}/invites"),
            &owner,
            Some(json!({"email": "guest@example.com", "role": "admin"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = created["token"].as_str().unwrap().to_owned();
    let anon = Auth::default();

    let (_, preview) = team
        .api(Method::POST, "/api/invites/preview", &anon, Some(json!({"token": token})))
        .await;
    assert_eq!(preview["account_exists"], true);

    // Wrong password: 401, invite intact. Five failures, then the email is delayed.
    for _ in 0..5 {
        let (status, _) = team
            .api(Method::POST, "/api/invites/accept", &anon, Some(json!({"token": token, "password": "definitely not it"})))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, headers, _) = call(
        &team.router,
        Method::POST,
        "/api/invites/accept",
        &anon,
        Some(json!({"token": token, "password": MEMBER_PASSWORD})),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.contains_key(header::RETRY_AFTER));
    // The same address is delayed at the login form: one shared throttle.
    let (status, _) = team
        .api(Method::POST, "/api/auth/login", &anon, Some(json!({"email": "guest@example.com", "password": MEMBER_PASSWORD})))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // After the delay, the right password joins with the invited role and a fresh session.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (status, headers, workspace) = call(
        &team.router,
        Method::POST,
        "/api/invites/accept",
        &anon,
        Some(json!({"token": token, "password": MEMBER_PASSWORD})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{workspace}");
    let orgs = workspace["organizations"].as_array().unwrap();
    assert_eq!(orgs.len(), 2);
    assert!(orgs.iter().any(|o| o["name"] == "Second" && o["role"] == "admin"));
    assert!(headers.contains_key(header::SET_COOKIE));

    team.finish().await;
}

#[tokio::test]
async fn guessing_invite_tokens_is_throttled_per_source() {
    let team = Team::start().await;
    let guesser = Auth::default().from("203.0.113.50");
    let bogus = format!("hgi_{}", "0".repeat(64));
    for attempt in 0..5 {
        let (status, _) = team
            .api(Method::POST, "/api/invites/preview", &guesser, Some(json!({"token": bogus})))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "attempt {attempt}");
    }
    let (status, _, _) = call(
        &team.router,
        Method::POST,
        "/api/invites/preview",
        &guesser,
        Some(json!({"token": bogus})),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // Another address, and a real invitee, are not affected.
    let (_, created) = team.invite(&team.owner(), "fine@example.com", "member").await;
    let other = Auth::default().from("203.0.113.51");
    let (status, _) = team
        .api(Method::POST, "/api/invites/preview", &other, Some(json!({"token": created["token"]})))
        .await;
    assert_eq!(status, StatusCode::OK);
    team.finish().await;
}

#[tokio::test]
async fn team_routes_need_a_session_and_keys_cannot_manage_people() {
    let team = Team::start().await;
    let owner = team.owner();
    let anon = Auth::default();
    for (method, uri) in [
        (Method::GET, team.members_uri()),
        (Method::GET, team.invites_uri()),
        (Method::POST, team.invites_uri()),
        (Method::PATCH, format!("{}/{}", team.members_uri(), uuid::Uuid::new_v4())),
        (Method::DELETE, format!("{}/{}", team.members_uri(), uuid::Uuid::new_v4())),
        (Method::DELETE, format!("{}/{}", team.invites_uri(), uuid::Uuid::new_v4())),
    ] {
        let (status, _) = team.api(method.clone(), &uri, &anon, Some(json!({"email": "a@b.co", "role": "member"}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
    // An owner's write-scoped key reads the team but cannot mint invites or change roles.
    let (_, key) = team
        .api(Method::POST, "/api/auth/keys", &owner, Some(json!({"name": "ci", "scope": "write"})))
        .await;
    let key = Auth::bearer(key["secret"].as_str().unwrap());
    assert_eq!(team.api(Method::GET, &team.members_uri(), &key, None).await.0, StatusCode::OK);
    assert_eq!(team.api(Method::GET, &team.invites_uri(), &key, None).await.0, StatusCode::OK);
    assert_eq!(team.invite(&key, "k@example.com", "member").await.0, StatusCode::FORBIDDEN);
    let (_, members) = team.api(Method::GET, &team.members_uri(), &owner, None).await;
    let owner_id = members[0]["user_id"].as_str().unwrap();
    assert_eq!(
        team.api(Method::PATCH, &format!("{}/{owner_id}", team.members_uri()), &key, Some(json!({"role": "member"}))).await.0,
        StatusCode::FORBIDDEN
    );
    team.finish().await;
}
