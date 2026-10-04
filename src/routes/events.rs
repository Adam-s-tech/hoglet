//! `GET /api/projects/{project_id}/events` — the live event feed.

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
use crate::explore::events::{self, DEFAULT_FEED_LIMIT, FeedQuery, MAX_EVENT_NAME};
use crate::explore::http::{
    ExploreState, MAX_PATH_ID, RequestId, assign_request_id, authorize, explore_error, invalid,
    parse_before, query, respond,
};
use crate::explore::{ExploreError, Explorer, filters, persons};
use crate::persons::MAX_DISTINCT_IDS_PER_CALL;

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    person_id: Option<String>,
    #[serde(default)]
    distinct_id: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    /// URL-encoded JSON array of event-property `PropertyFilter`s.
    #[serde(default)]
    properties: Option<String>,
}

pub fn router(access: Arc<ProjectAccess>, explorer: Arc<Explorer>) -> Router {
    Router::new()
        .route("/api/projects/{project_id}/events", get(list))
        .with_state(ExploreState { access, explorer })
        .layer(middleware::from_fn(assign_request_id))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
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
    let query: EventsQuery = match query(&uri, &request_id) {
        Ok(query) => query,
        Err(response) => return response,
    };
    let event = non_empty(query.event);
    let person_id = non_empty(query.person_id);
    let distinct_id = non_empty(query.distinct_id);
    if event
        .as_ref()
        .is_some_and(|name| name.len() > MAX_EVENT_NAME)
        || person_id.as_ref().is_some_and(|id| id.len() > MAX_PATH_ID)
        || distinct_id
            .as_ref()
            .is_some_and(|id| id.len() > MAX_PATH_ID)
    {
        return invalid("A filter value is too long.", &request_id);
    }
    let before = match parse_before(query.before.as_deref(), &request_id) {
        Ok(before) => before,
        Err(response) => return response,
    };
    let filters = match filters::parse(query.properties.as_deref()) {
        Ok(filters) => filters,
        Err(error) => return explore_error(error, &request_id),
    };
    let limit = query.limit.unwrap_or(DEFAULT_FEED_LIMIT);
    respond(&state, &request_id, move |explorer, connection| {
        let distinct_ids = match person_id {
            Some(person_id) => {
                let ids = match persons::find(explorer.persons(), &project_id, &person_id) {
                    Ok(person) => explorer.persons().distinct_ids_of(
                        &project_id,
                        &person.id,
                        MAX_DISTINCT_IDS_PER_CALL,
                    )?,
                    // An unknown person has no events.
                    Err(ExploreError::NotFound) => Vec::new(),
                    Err(error) => return Err(error),
                };
                Some(match distinct_id {
                    Some(distinct_id) => ids.into_iter().filter(|id| *id == distinct_id).collect(),
                    None => ids,
                })
            }
            None => distinct_id.map(|id| vec![id]),
        };
        events::feed(
            explorer,
            connection,
            &project_id,
            &FeedQuery {
                event,
                distinct_ids,
                before,
                limit,
                filters,
            },
            Utc::now(),
        )
    })
    .await
}
