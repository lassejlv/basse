//! Deployments: history, logs, cancel and rollback.

use super::{Json, Query, ndjson, ndjson_line};
use crate::deploy;
use crate::error::{ApiResult, AppError};
use crate::services;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use common::*;
use futures_util::StreamExt as _;
use serde::Deserialize;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;

#[derive(Deserialize)]
pub struct ListQuery {
    limit: Option<usize>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Deployment>>> {
    services::record(&state, &id)?;
    let limit = q.limit.unwrap_or(20).clamp(1, 500);
    Ok(Json(state.db.deployments(&id, limit)?))
}

fn find(state: &AppState, id: &str) -> ApiResult<Deployment> {
    state
        .db
        .deployment(id)?
        .ok_or_else(|| AppError::not_found("deployment", id))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Deployment>> {
    Ok(Json(find(&state, &id)?))
}

#[derive(Deserialize)]
pub struct LogsQuery {
    #[serde(default)]
    follow: bool,
}

pub async fn logs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Response> {
    find(&state, &id)?;
    if let Some(log) = state.deploys.log(&id) {
        let (history, rx) = log.subscribe();
        drop(log);
        let history = futures_util::stream::iter(history);
        if !q.follow {
            return Ok(ndjson(history));
        }
        let live = BroadcastStream::new(rx).filter_map(|item| async move { item.ok() });
        return Ok(ndjson(history.chain(live)));
    }
    let bytes = match tokio::fs::read(state.dirs.deployment_log(&id)).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let line = LogLine {
                ts: None,
                stream: LogStream::System,
                line: "no log was recorded".into(),
            };
            ndjson_line(&line).to_vec()
        }
        Err(e) => return Err(e.into()),
    };
    Ok((
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from(bytes),
    )
        .into_response())
}

pub async fn cancel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Deployment>> {
    Ok(Json(deploy::cancel(&state, &id).await?))
}

pub async fn rollback(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Deployment>> {
    let target = find(&state, &id)?;
    Ok(Json(deploy::rollback(&state, &target)?))
}
