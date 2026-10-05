//! Authenticated catalog autocomplete over ordered projection state:
//! `GET /api/projects/{project_id}/catalog/{events,properties,values}`.

use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::contract::common::PropertySource;
use crate::control::ProjectAccess;
use crate::explore::http::{RequestId, api_error, assign_request_id, authorize, invalid, query};
use crate::projection_catalog::{MAX_CATALOG_EVENTS, ProjectionCatalog, ProjectionCatalogError};

const DEFAULT_VALUE_LIMIT: usize = 50;

#[derive(Clone)]
struct CatalogState {
    access: Arc<ProjectAccess>,
    catalog: Arc<ProjectionCatalog>,
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default, alias = "prefix")]
    search: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct PropertiesQuery {
    #[serde(default, rename = "type", alias = "source")]
    source: PropertySource,
    #[serde(default, alias = "prefix")]
    search: String,
}

#[derive(Debug, Deserialize)]
struct ValuesQuery {
    #[serde(default)]
    key: String,
    #[serde(default, rename = "type", alias = "source")]
    source: PropertySource,
    #[serde(default, alias = "prefix")]
    search: String,
    #[serde(default)]
    limit: Option<usize>,
}

pub fn router(access: Arc<ProjectAccess>, catalog: Arc<ProjectionCatalog>) -> Router {
    Router::new()
        .route("/api/projects/{project_id}/catalog/events", get(events))
        .route(
            "/api/projects/{project_id}/catalog/properties",
            get(properties),
        )
        .route("/api/projects/{project_id}/catalog/values", get(values))
        .with_state(CatalogState { access, catalog })
        .layer(middleware::from_fn(assign_request_id))
}

async fn events(
    State(state): State<CatalogState>,
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
    let catalog = state.catalog;
    let limit = query.limit.unwrap_or(MAX_CATALOG_EVENTS);
    catalog_result(
        tokio::task::spawn_blocking(move || catalog.event_names(&project_id, &query.search, limit))
            .await,
        &request_id,
    )
}

async fn properties(
    State(state): State<CatalogState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let project_id = match authorize(&state.access, &headers, &project_id, &request_id).await {
        Ok(project_id) => project_id,
        Err(response) => return response,
    };
    let query: PropertiesQuery = match query(&uri, &request_id) {
        Ok(query) => query,
        Err(response) => return response,
    };
    let catalog = state.catalog;
    catalog_result(
        tokio::task::spawn_blocking(move || {
            catalog.property_keys(&project_id, query.source, &query.search)
        })
        .await,
        &request_id,
    )
}

async fn values(
    State(state): State<CatalogState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let project_id = match authorize(&state.access, &headers, &project_id, &request_id).await {
        Ok(project_id) => project_id,
        Err(response) => return response,
    };
    let query = match query::<ValuesQuery>(&uri, &request_id) {
        Ok(query) if !query.key.trim().is_empty() => query,
        Ok(_) => return invalid("key: required.", &request_id),
        Err(response) => return response,
    };
    let catalog = state.catalog;
    let limit = query.limit.unwrap_or(DEFAULT_VALUE_LIMIT);
    catalog_result(
        tokio::task::spawn_blocking(move || {
            catalog.property_values(&project_id, query.source, &query.key, &query.search, limit)
        })
        .await,
        &request_id,
    )
}

fn catalog_result<T: Serialize>(
    result: Result<Result<T, ProjectionCatalogError>, tokio::task::JoinError>,
    request_id: &RequestId,
) -> Response {
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(ProjectionCatalogError::InvalidRequest(field))) => {
            invalid(&format!("{field}: invalid."), request_id)
        }
        Ok(Err(failure)) => {
            tracing::error!(request_id = %request_id.0, error = %failure, "catalog request failed");
            internal(request_id)
        }
        Err(_) => internal(request_id),
    }
}

fn internal(request_id: &RequestId) -> Response {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "The catalog could not be loaded.",
        request_id,
    )
}
