//! Database services: connection details, public access, password rotation and backups.

use super::{Json, no_content, optional_json};
use crate::database;
use crate::error::{ApiResult, AppError};
use crate::services;
use crate::state::AppState;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use common::api::{CreateBackup, UpdateDatabase};
use common::*;
use std::sync::Arc;
use tokio_util::io::ReaderStream;

/// Connection details of a database service, after a change.
async fn info(state: &AppState, service_id: &str) -> ApiResult<Json<DatabaseInfo>> {
    services::detail(state, service_id)
        .await?
        .database
        .map(Json)
        .ok_or_else(|| AppError::BadRequest("not a database service".into()))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDatabase>,
) -> ApiResult<Json<DatabaseInfo>> {
    let rec = services::record(&state, &id).await?;
    database::set_public_port(&state, &rec, body.public_port).await?;
    info(&state, &id).await
}

pub async fn rotate_password(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<DatabaseInfo>> {
    let rec = services::record(&state, &id).await?;
    database::rotate_password(&state, &rec).await?;
    info(&state, &id).await
}

pub async fn list_backups(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<Backup>>> {
    services::record(&state, &id).await?;
    Ok(Json(state.db.backups(&id).await?))
}

pub async fn create_backup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Backup>> {
    let body: CreateBackup = optional_json(&body)?;
    let rec = services::record(&state, &id).await?;
    Ok(Json(
        database::start_backup(
            &state,
            &rec,
            BackupTrigger::Manual,
            body.storage_id.as_deref(),
        )
        .await?,
    ))
}

async fn backup(state: &AppState, id: &str) -> ApiResult<Backup> {
    state
        .db
        .backup(id)
        .await?
        .ok_or_else(|| AppError::not_found("backup", id))
}

pub async fn get_backup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Backup>> {
    Ok(Json(backup(&state, &id).await?))
}

pub async fn download_backup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let b = backup(&state, &id).await?;
    if b.status != BackupStatus::Done {
        return Err(AppError::BadRequest(format!(
            "backup {} is {}, only finished backups can be downloaded",
            b.file_name,
            b.status.as_str()
        )));
    }
    let file = database::open_backup(&state, &b).await?;
    let headers = [
        (header::CONTENT_TYPE, "application/gzip".to_string()),
        (header::CONTENT_LENGTH, b.size_bytes.to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", b.file_name),
        ),
    ];
    Ok((headers, Body::from_stream(ReaderStream::new(file))).into_response())
}

pub async fn restore_backup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Backup>> {
    let b = backup(&state, &id).await?;
    database::restore(&state, &b).await?;
    Ok(Json(b))
}

pub async fn delete_backup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let b = backup(&state, &id).await?;
    database::delete_backup(&state, &b).await?;
    Ok(no_content())
}

pub async fn get_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<BackupSchedule>> {
    services::record(&state, &id).await?;
    Ok(Json(state.db.backup_schedule(&id).await?.schedule))
}

pub async fn set_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<BackupSchedule>,
) -> ApiResult<Json<BackupSchedule>> {
    let rec = services::record(&state, &id).await?;
    Ok(Json(database::set_schedule(&state, &rec, body).await?))
}
