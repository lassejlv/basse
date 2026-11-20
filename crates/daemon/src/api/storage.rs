//! Storage connections: S3-compatible buckets backups can be kept in.

use super::secrets::secret_of_kind;
use super::{Json, no_content};
use crate::backup_store::BackupStore;
use crate::db::new_id;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use common::api::{CreateStorage, UpdateStorage};
use common::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<StorageConnection>>> {
    Ok(Json(state.db.storages().await?))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<StorageConnection>> {
    Ok(Json(storage(&state, &id).await?))
}

async fn storage(state: &AppState, id: &str) -> ApiResult<StorageConnection> {
    state
        .db
        .storage(id)
        .await?
        .ok_or_else(|| AppError::not_found("storage connection", id))
}

/// Normalizes and checks every field; the endpoint must be an http(s) URL.
async fn validate(state: &AppState, s: &mut StorageConnection) -> ApiResult<()> {
    validate_name(&s.name).map_err(AppError::BadRequest)?;
    s.endpoint = s.endpoint.trim().trim_end_matches('/').to_string();
    let host = s
        .endpoint
        .strip_prefix("https://")
        .or_else(|| s.endpoint.strip_prefix("http://"));
    if host.is_none_or(|h| h.is_empty() || h.chars().any(char::is_whitespace)) {
        bad_request!(
            "invalid endpoint '{}': use a URL such as https://s3.eu-west-1.amazonaws.com",
            s.endpoint
        );
    }
    s.region = s.region.trim().to_string();
    if s.region.is_empty() {
        bad_request!("region must not be empty (use auto for Cloudflare R2)");
    }
    s.bucket = s.bucket.trim().to_string();
    if s.bucket.is_empty() || s.bucket.contains(['/', ' ']) {
        bad_request!("invalid bucket '{}'", s.bucket);
    }
    s.path_prefix = s.path_prefix.trim().trim_matches('/').to_string();
    if s.path_prefix
        .split('/')
        .any(|part| part == "." || part == ".." || (part.is_empty() && !s.path_prefix.is_empty()))
    {
        bad_request!("invalid path prefix '{}'", s.path_prefix);
    }
    secret_of_kind(state, &s.credentials_secret_id, SecretKind::S3Credentials).await?;
    Ok(())
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateStorage>,
) -> ApiResult<Json<StorageConnection>> {
    let mut conn = StorageConnection {
        id: new_id(),
        name: body.name,
        endpoint: body.endpoint,
        region: body.region,
        bucket: body.bucket,
        path_prefix: body.path_prefix,
        path_style: body.path_style,
        credentials_secret_id: body.credentials_secret_id,
        created_at: Utc::now(),
        last_test: None,
    };
    validate(&state, &mut conn).await?;
    if state.db.storage_name_taken(&conn.name).await? {
        conflict!("a storage connection named '{}' already exists", conn.name);
    }
    state.db.save_storage(&conn).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Storage {} added", conn.name),
            Some(location(&conn)),
        )
        .await;
    Ok(Json(conn))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateStorage>,
) -> ApiResult<Json<StorageConnection>> {
    let old = storage(&state, &id).await?;
    let mut conn = old.clone();
    let UpdateStorage {
        name,
        endpoint,
        region,
        bucket,
        path_prefix,
        path_style,
        credentials_secret_id,
    } = body;
    conn.name = name.unwrap_or(conn.name);
    conn.endpoint = endpoint.unwrap_or(conn.endpoint);
    conn.region = region.unwrap_or(conn.region);
    conn.bucket = bucket.unwrap_or(conn.bucket);
    conn.path_prefix = path_prefix.unwrap_or(conn.path_prefix);
    conn.path_style = path_style.unwrap_or(conn.path_style);
    conn.credentials_secret_id = credentials_secret_id.unwrap_or(conn.credentials_secret_id);
    validate(&state, &mut conn).await?;
    if conn.name != old.name && state.db.storage_name_taken(&conn.name).await? {
        conflict!("a storage connection named '{}' already exists", conn.name);
    }
    let moved = (&conn.endpoint, &conn.bucket, &conn.path_prefix)
        != (&old.endpoint, &old.bucket, &old.path_prefix);
    let (backups, _) = state.db.storage_usage(&id).await?;
    if moved && backups > 0 {
        conflict!(
            "storage '{}' holds {backups} backup(s); endpoint, bucket and prefix can only change \
             while it is empty, or the backups would be lost",
            old.name
        );
    }
    if conn != old {
        conn.last_test = None;
    }
    state.db.save_storage(&conn).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Storage {} updated", conn.name),
            Some(location(&conn)),
        )
        .await;
    Ok(Json(conn))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let conn = storage(&state, &id).await?;
    let (backups, schedules) = state.db.storage_usage(&id).await?;
    if backups > 0 || schedules > 0 {
        conflict!(
            "storage '{}' still holds {backups} backup(s) and is the target of {schedules} \
             schedule(s); delete those backups and point the schedules elsewhere first",
            conn.name
        );
    }
    state.db.delete_storage(&id).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Storage {} removed", conn.name),
            None,
        )
        .await;
    Ok(no_content())
}

/// Puts, reads back and deletes a small probe object, and stores the outcome on the connection.
pub async fn test(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<StorageTest>> {
    let mut conn = storage(&state, &id).await?;
    let started = Instant::now();
    let outcome = probe(&state, &conn).await;
    let result = StorageTest {
        ok: outcome.is_ok(),
        at: Utc::now(),
        latency_ms: outcome
            .is_ok()
            .then(|| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
        error: outcome.err().map(|e| format!("{e:#}")),
    };
    conn.last_test = Some(result.clone());
    state.db.save_storage(&conn).await?;
    let (level, message) = if result.ok {
        (
            ActivityLevel::Success,
            format!("Storage {} works", conn.name),
        )
    } else {
        (
            ActivityLevel::Error,
            format!("Storage {} test failed", conn.name),
        )
    };
    state
        .activity(
            level,
            message,
            result
                .error
                .clone()
                .or_else(|| result.latency_ms.map(|ms| format!("{ms} ms"))),
        )
        .await;
    Ok(Json(result))
}

async fn probe(state: &AppState, conn: &StorageConnection) -> anyhow::Result<()> {
    let store = BackupStore::for_connection(conn, &state.secrets)
        .await?
        .with_timeout(PROBE_TIMEOUT);
    let key = format!(".probe/{}", new_id());
    let body = format!(
        "{} storage probe {}\n",
        brand::NAME,
        Utc::now().to_rfc3339()
    );
    let staged = state
        .dirs
        .backups_dir()
        .join(".staging")
        .join(format!("probe-{}", new_id()));
    tokio::fs::create_dir_all(staged.parent().expect("staging has a parent")).await?;
    tokio::fs::write(&staged, &body).await?;
    let put = store.put(&key, &staged).await;
    let _ = tokio::fs::remove_file(&staged).await;
    put?;
    let mut read = Vec::new();
    let got = async {
        let mut reader = store.get(&key).await?;
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut read).await?;
        anyhow::Ok(())
    }
    .await;
    let deleted = store.delete(&key).await;
    got?;
    if read != body.as_bytes() {
        anyhow::bail!("the probe object read back differs from what was written");
    }
    deleted
}

/// `bucket/prefix at endpoint`, for activity rows and messages.
pub fn location(conn: &StorageConnection) -> String {
    let path = if conn.path_prefix.is_empty() {
        conn.bucket.clone()
    } else {
        format!("{}/{}", conn.bucket, conn.path_prefix)
    };
    format!("{path} at {}", conn.endpoint)
}
