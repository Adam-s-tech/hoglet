//! `GET /api/projects/{project_id}/persons[/{person_id}[/events]]`.

use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{OriginalUri, Path, State},
    http::HeaderMap,
    middleware,
    response::Response,
    routing::get,
};
use chrono::Utc;
use serde::Deserialize;

use crate::control::ProjectAccess;
use crate::explore::Explorer;
use crate::explore::events::DEFAULT_FEED_LIMIT;
use crate::explore::http::{
    ExploreState, MAX_PATH_ID, RequestId, assign_request_id, authorize, explore_error, not_found,
    parse_before, query, respond,
};
use crate::explore::persons;

const DEFAULT_PERSON_LIMIT: usize = 50;

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub fn router(access: Arc<ProjectAccess>, explorer: Arc<Explorer>) -> Router {
    Router::new()
        .route("/api/projects/{project_id}/persons", get(list))
        .route(
            "/api/projects/{project_id}/persons/{person_id}",
            get(detail),
        )
        .route(
            "/api/projects/{project_id}/persons/{person_id}/events",
            get(events),
        )
        .with_state(ExploreState { access, explorer })
        .layer(middleware::from_fn(assign_request_id))
}

async fn list(
    State(state): State<ExploreState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let project_id = match authorize(&state.access, &headers, &project_id, &request_id).await {
        Ok(project_id) => project_id,
        Err(response) => return response,
    };
    let query: ListQuery = match query(&uri, &request_id) {
        Ok(query) => query,
        Err(response) => return response,
    };
    let cursor = match query.cursor.as_deref().filter(|text| !text.is_empty()) {
        None => None,
        Some(text) => match persons::decode_cursor(text) {
            Ok(cursor) => Some(cursor),
            Err(error) => return explore_error(error, &request_id),
        },
    };
    let limit = query.limit.unwrap_or(DEFAULT_PERSON_LIMIT);
    respond(&state, &request_id, move |explorer, _| {
        persons::list(
            explorer.persons(),
            &project_id,
            query.search.as_deref(),
            cursor.as_ref(),
            limit,
        )
    })
    .await
}

async fn detail(
    State(state): State<ExploreState>,
    Extension(request_id): Extension<RequestId>,
    Path((project_id, person_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let project_id = match authorize(&state.access, &headers, &project_id, &request_id).await {
        Ok(project_id) => project_id,
        Err(response) => return response,
    };
    if person_id.is_empty() || person_id.len() > MAX_PATH_ID {
        return not_found(&request_id);
    }
    respond(&state, &request_id, move |explorer, connection| {
        persons::detail(explorer, connection, &project_id, &person_id)
    })
    .await
}

async fn events(
    State(state): State<ExploreState>,
    Extension(request_id): Extension<RequestId>,
    Path((project_id, person_id)): Path<(String, String)>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let project_id = match authorize(&state.access, &headers, &project_id, &request_id).await {
        Ok(project_id) => project_id,
        Err(response) => return response,
    };
    if person_id.is_empty() || person_id.len() > MAX_PATH_ID {
        return not_found(&request_id);
    }
    let query: EventsQuery = match query(&uri, &request_id) {
        Ok(query) => query,
        Err(response) => return response,
    };
    let before = match parse_before(query.before.as_deref(), &request_id) {
        Ok(before) => before,
        Err(response) => return response,
    };
    let limit = query.limit.unwrap_or(DEFAULT_FEED_LIMIT);
    respond(&state, &request_id, move |explorer, connection| {
        persons::person_events(
            explorer,
            connection,
            &project_id,
            &person_id,
            before,
            limit,
            Utc::now(),
        )
    })
    .await
}
