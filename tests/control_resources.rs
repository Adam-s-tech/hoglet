use hoglet::control::{ProjectAccess, SetupRequest};
use hoglet::control_resources::{
    ControlResourceError, ControlResources, DashboardDraft, DashboardTileInput, ImportedInsight,
    InsightDraft, ShareTarget,
};
use hoglet::storage_bootstrap::bootstrap_storage;
use serde_json::{Value, json};

struct Fixture {
    _directory: tempfile::TempDir,
    resources: ControlResources,
    first_project: String,
    second_project: String,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let control_path = directory.path().join("control.db");
    bootstrap_storage(&control_path, directory.path().join("projections.db")).unwrap();

    let (access, runtime) = ProjectAccess::open(control_path.clone()).unwrap();
    let setup = access
        .setup(SetupRequest {
            email: "owner@example.com".into(),
            password: "correct horse battery staple".into(),
            organization_name: "Acme".into(),
            project_name: "First".into(),
            existing_project_token: None,
        })
        .await
        .unwrap();
    let principal = access.validate_session(&setup.session_id).await.unwrap();
    let organization = &setup.workspace.organizations[0];
    let first_project = organization.projects[0].id.clone();
    let second_project = access
        .create_project(&principal, &organization.id, "Second")
        .await
        .unwrap()
        .id;
    drop(access);
    runtime.close();

    Fixture {
        resources: ControlResources::open(&control_path).unwrap(),
        _directory: directory,
        first_project,
        second_project,
    }
}

fn valid_query() -> Value {
    json!({
        "kind": "TrendsQuery",
        "series": [{"event": "$pageview", "math": "total"}],
        "date_range": {"date_from": "2026-08-01", "date_to": "2026-08-08"},
        "interval": "day"
    })
}

fn insight(name: &str) -> InsightDraft {
    InsightDraft {
        name: name.into(),
        description: String::new(),
        query_ir: valid_query(),
    }
}

#[test]
fn resources_refuse_unvalidated_or_wrong_role_databases() {
    let directory = tempfile::tempdir().unwrap();
    let arbitrary = directory.path().join("arbitrary.db");
    rusqlite::Connection::open(&arbitrary).unwrap();
    assert!(matches!(
        ControlResources::open(&arbitrary),
        Err(ControlResourceError::InvalidStorage)
    ));

    let control = directory.path().join("control.db");
    let projections = directory.path().join("projections.db");
    bootstrap_storage(&control, &projections).unwrap();
    assert!(matches!(
        ControlResources::open(&projections),
        Err(ControlResourceError::InvalidStorage)
    ));
}

#[tokio::test]
async fn direct_keys_and_ids_cannot_cross_project_resource_scopes() {
    let fixture = fixture().await;
    let store = &fixture.resources;
    let first = &fixture.first_project;
    let second = &fixture.second_project;

    let saved = store
        .create_insight(first, "user-1", &insight("Conversion"))
        .unwrap();
    assert!(matches!(
        store.get_insight(second, &saved.id),
        Err(ControlResourceError::NotFound)
    ));
    assert!(matches!(
        store.update_insight(second, &saved.id, &insight("Stolen")),
        Err(ControlResourceError::NotFound)
    ));
    assert!(matches!(
        store.delete_insight(second, &saved.id),
        Err(ControlResourceError::NotFound)
    ));
    assert_eq!(
        store.get_insight(first, &saved.id).unwrap().name,
        "Conversion"
    );

    let dashboard = store
        .create_dashboard(
            first,
            "user-1",
            &DashboardDraft {
                name: "Product".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        store.get_dashboard(second, &dashboard.id),
        Err(ControlResourceError::NotFound)
    ));
    assert!(matches!(
        store.update_dashboard(
            second,
            &dashboard.id,
            &DashboardDraft {
                name: "Stolen".into(),
            }
        ),
        Err(ControlResourceError::NotFound)
    ));
    assert!(matches!(
        store.delete_dashboard(second, &dashboard.id),
        Err(ControlResourceError::NotFound)
    ));

    let share = store
        .create_share(first, ShareTarget::Dashboard, &dashboard.id, "user-1", None)
        .unwrap();
    assert!(matches!(
        store.create_share(
            second,
            ShareTarget::Dashboard,
            &dashboard.id,
            "user-1",
            None
        ),
        Err(ControlResourceError::NotFound)
    ));
    assert!(matches!(
        store.get_share(second, &share.id),
        Err(ControlResourceError::NotFound)
    ));
    assert!(matches!(
        store.update_share_expiry(second, &share.id, Some(1_800_000_000)),
        Err(ControlResourceError::NotFound)
    ));
    assert!(matches!(
        store.delete_share(second, &share.id),
        Err(ControlResourceError::NotFound)
    ));
    assert_eq!(
        store.get_share(first, &share.id).unwrap().token,
        share.token
    );
    assert_eq!(
        store
            .authorize_share_token(&share.token, share.created_at)
            .unwrap()
            .project_id,
        *first
    );
    store
        .update_share_expiry(first, &share.id, Some(share.created_at + 1))
        .unwrap();
    assert!(matches!(
        store.authorize_share_token(&share.token, share.created_at + 1),
        Err(ControlResourceError::NotFound)
    ));
}

#[tokio::test]
async fn dashboard_tiles_only_accept_insights_from_the_same_project_transactionally() {
    let fixture = fixture().await;
    let store = &fixture.resources;
    let first = &fixture.first_project;
    let second = &fixture.second_project;

    let first_insight = store
        .create_insight(first, "user-1", &insight("First insight"))
        .unwrap();
    let second_insight = store
        .create_insight(second, "user-1", &insight("Second insight"))
        .unwrap();
    let dashboard = store
        .create_dashboard(
            first,
            "user-1",
            &DashboardDraft {
                name: "Overview".into(),
            },
        )
        .unwrap();

    let first_tile = DashboardTileInput {
        insight_id: first_insight.id.clone(),
        x: 0,
        y: 0,
        w: 6,
        h: 4,
    };
    store
        .replace_dashboard_tiles(first, &dashboard.id, std::slice::from_ref(&first_tile))
        .unwrap();

    let cross_project = DashboardTileInput {
        insight_id: second_insight.id,
        x: 6,
        y: 0,
        w: 6,
        h: 4,
    };
    assert!(matches!(
        store.replace_dashboard_tiles(first, &dashboard.id, &[cross_project]),
        Err(ControlResourceError::NotFound)
    ));

    let unchanged = store.get_dashboard(first, &dashboard.id).unwrap();
    assert_eq!(
        unchanged.tiles.len(),
        1,
        "failed replacement must roll back"
    );
    assert_eq!(unchanged.tiles[0].insight_id, first_insight.id);
    assert_eq!(
        unchanged.tiles[0].insight.as_ref().unwrap().project_id,
        *first
    );
}

#[tokio::test]
async fn normal_insight_writes_reject_unsupported_ir_but_explicit_import_preserves_it() {
    let fixture = fixture().await;
    let store = &fixture.resources;
    let project = &fixture.first_project;

    // The legacy IR shape is not an `InsightQuery`; neither is a query over
    // the engine's limits.
    let unsupported_ir = json!({
        "kind": "Funnels",
        "series": [{"event": {"type": "name", "value": "pageview"}, "math": {"type": "total"}}],
        "range": {"from": "2026-08-01T00:00:00Z", "to": "2026-08-08T00:00:00Z"}
    });
    let mut over_limit = valid_query();
    over_limit["series"] = Value::Array(vec![json!({"event": "$pageview"}); 21]);
    assert!(matches!(
        store.create_insight(
            project,
            "user-1",
            &InsightDraft {
                name: "Too many".into(),
                description: String::new(),
                query_ir: over_limit,
            }
        ),
        Err(ControlResourceError::InvalidQuery { .. })
    ));
    let draft = InsightDraft {
        name: "Legacy funnel".into(),
        description: "Imported for reference".into(),
        query_ir: unsupported_ir.clone(),
    };
    assert!(matches!(
        store.create_insight(project, "user-1", &draft),
        Err(ControlResourceError::InvalidQuery { .. })
    ));

    let imported = store
        .import_insight(
            project,
            ImportedInsight {
                id: "legacy-insight-1".into(),
                name: draft.name,
                description: draft.description,
                query_ir: unsupported_ir,
                created_by: "legacy-user".into(),
                created_at: 100,
                updated_at: 200,
            },
        )
        .unwrap();
    assert_eq!(imported.id, "legacy-insight-1");
    assert_eq!(imported.query_ir["kind"], "Funnels");

    let encoded = serde_json::to_value(imported).unwrap();
    assert_eq!(encoded["project_id"], project.as_str());
    assert!(encoded.get("token").is_none());
}
