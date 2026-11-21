//! Instance backups over the API. Restoring is the `<daemon> restore` command, not a route.

use super::{Json, no_content};
use crate::error::{ApiResult, AppError, bad_request};
use crate::instance;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use common::api::{CreateInstanceBackup, UpdateInstanceBackupSchedule};
use common::*;
use std::sync::Arc;
use tokio_util::io::ReaderStream;
use zeroize::Zeroizing;

pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<InstanceBackup>>> {
    Ok(Json(state.db.instance_backups().await?))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateInstanceBackup>,
) -> ApiResult<Json<InstanceBackup>> {
    let CreateInstanceBackup {
        storage_id,
        passphrase,
    } = body;
    Ok(Json(
        instance::start(
            &state,
            BackupTrigger::Manual,
            storage_id.as_deref(),
            Zeroizing::new(passphrase),
        )
        .await?,
    ))
}

async fn backup(state: &AppState, id: &str) -> ApiResult<InstanceBackup> {
    state
        .db
        .instance_backup(id)
        .await?
        .ok_or_else(|| AppError::not_found("instance backup", id))
}

pub async fn download(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let b = backup(&state, &id).await?;
    if b.status != BackupStatus::Done {
        bad_request!(
            "instance backup {} is {}, only finished backups can be downloaded",
            b.file_name,
            b.status.as_str()
        );
    }
    let file = instance::open(&state, &b).await?;
    let headers = [
        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        (header::CONTENT_LENGTH, b.size_bytes.to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", b.file_name),
        ),
    ];
    Ok((headers, Body::from_stream(ReaderStream::new(file))).into_response())
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let b = backup(&state, &id).await?;
    instance::delete(&state, &b).await?;
    Ok(no_content())
}

pub async fn get_schedule(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<InstanceBackupSchedule>> {
    Ok(Json(instance::schedule(&state).await?))
}

pub async fn set_schedule(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateInstanceBackupSchedule>,
) -> ApiResult<Json<InstanceBackupSchedule>> {
    Ok(Json(instance::set_schedule(&state, body).await?))
}
