//! `POST /api/projects/{project_id}/demo` — fill a project with the demo
//! dataset so a fresh install shows a living product in one click.
//!
//! Events take the real durable path (WAL → publication → lake), in bounded
//! batches, so the demo proves the pipeline instead of bypassing it.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;

use crate::control::ProjectAccess;
use crate::sink::{AuthorizedEventBatch, EventSink};

/// Events per WAL record while seeding.
const DEMO_BATCH: usize = 500;

#[derive(Clone)]
struct DemoState {
    access: Arc<ProjectAccess>,
    sink: Arc<dyn EventSink>,
}

pub fn router(access: Arc<ProjectAccess>, sink: Arc<dyn EventSink>) -> Router {
    Router::new()
        .route("/api/projects/{project_id}/demo", post(seed))
        .with_state(DemoState { access, sink })
}

async fn seed(
    State(state): State<DemoState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let project = match crate::routes::guard::authorize(
        &state.access,
        &headers,
        &project_id,
        crate::routes::guard::Intent::Write,
    )
    .await
    {
        Ok(project) => project,
        Err(response) => return response,
    };
    match seed_project(state.sink.as_ref(), &project.project_id, &project.capture_token).await {
        Ok(events) => Json(json!({"events": events})).into_response(),
        Err(SeedError) => crate::routes::guard::error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "The demo data could not be stored; try again.",
            &crate::routes::guard::request_id(&headers),
        ),
    }
}

/// The demo dataset could not be generated or stored.
#[derive(Debug)]
pub struct SeedError;

/// Generate and durably append the demo dataset. Returns the event count.
pub async fn seed_project(
    sink: &dyn EventSink,
    project_id: &str,
    token: &str,
) -> Result<usize, SeedError> {
    let config = crate::demo::DemoConfig::new(token, chrono::Utc::now());
    let events = tokio::task::spawn_blocking(move || crate::demo::generate(&config))
        .await
        .map_err(|_| SeedError)?;
    let total = events.len();
    let mut bindings = BTreeMap::new();
    bindings.insert(token.to_owned(), project_id.to_owned());
    for chunk in events.chunks(DEMO_BATCH) {
        let batch = AuthorizedEventBatch {
            events: chunk.to_vec(),
            project_ids_by_token: bindings.clone(),
            historical_migration: true,
        };
        // Back off and retry while the pipeline sheds load.
        let mut attempts = 0;
        loop {
            match sink.append(batch.clone()).await {
                Ok(()) => break,
                Err(crate::sink::SinkError::Retryable) if attempts < 200 => {
                    attempts += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(_) => return Err(SeedError),
            }
        }
    }
    Ok(total)
}
