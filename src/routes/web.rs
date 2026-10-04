//! `GET /api/projects/{project_id}/web/overview` and `/web/breakdown`.
#![allow(clippy::result_large_err)]

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

use crate::contract::common::Interval;
use crate::contract::web::{WebDimension, WebQuery};
use crate::control::ProjectAccess;
use crate::explore::http::{
    ExploreState, RequestId, assign_request_id, authorize, explore_error, invalid, query, respond,
};
use crate::explore::web::{self, DEFAULT_BREAKDOWN_LIMIT};
use crate::explore::{Explorer, filters};

/// The query string of both endpoints. `properties` is URL-encoded JSON.
#[derive(Debug, Deserialize)]
struct RawWebQuery {
    #[serde(default)]
    date_from: Option<String>,
    #[serde(default)]
    date_to: Option<String>,
    #[serde(default)]
    interval: Option<Interval>,
    #[serde(default)]
    properties: Option<String>,
    #[serde(default)]
    dimension: Option<WebDimension>,
    #[serde(default)]
    limit: Option<usize>,
}

impl RawWebQuery {
    fn web_query(&self) -> Result<WebQuery, crate::explore::ExploreError> {
        Ok(WebQuery {
            date_from: self
                .date_from
                .clone()
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "-7d".to_owned()),
            date_to: self.date_to.clone().filter(|text| !text.is_empty()),
            interval: self.interval,
            properties: filters::parse(self.properties.as_deref())?,
        })
    }
}

pub fn router(access: Arc<ProjectAccess>, explorer: Arc<Explorer>) -> Router {
    Router::new()
        .route("/api/projects/{project_id}/web/overview", get(overview))
        .route("/api/projects/{project_id}/web/breakdown", get(breakdown))
        .with_state(ExploreState { access, explorer })
        .layer(middleware::from_fn(assign_request_id))
}

async fn parse(
    state: &ExploreState,
    request_id: &RequestId,
    project_id: &str,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(String, RawWebQuery, WebQuery), Response> {
    let project_id = authorize(&state.access, headers, project_id, request_id).await?;
    let raw: RawWebQuery = query(uri, request_id)?;
    let web_query = raw
        .web_query()
        .map_err(|error| explore_error(error, request_id))?;
    Ok((project_id, raw, web_query))
}

async fn overview(
    State(state): State<ExploreState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let (project_id, _, web_query) =
        match parse(&state, &request_id, &project_id, &headers, &uri).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    respond(&state, &request_id, move |explorer, connection| {
        web::overview(explorer, connection, &project_id, &web_query, Utc::now())
    })
    .await
}

async fn breakdown(
    State(state): State<ExploreState>,
    Extension(request_id): Extension<RequestId>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let (project_id, raw, web_query) =
        match parse(&state, &request_id, &project_id, &headers, &uri).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let Some(dimension) = raw.dimension else {
        return invalid("dimension: required.", &request_id);
    };
    let limit = raw.limit.unwrap_or(DEFAULT_BREAKDOWN_LIMIT);
    respond(&state, &request_id, move |explorer, connection| {
        web::breakdown(
            explorer,
            connection,
            &project_id,
            &web_query,
            dimension,
            limit,
            Utc::now(),
        )
    })
    .await
}
