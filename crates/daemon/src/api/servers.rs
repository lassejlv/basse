//! Servers: registration, health checks and proxy installation.

use super::{Json, no_content};
use crate::db::new_id;
use crate::docker;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use common::api::CreateServer;
use common::*;
use std::sync::Arc;
use std::time::Duration;

const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<Server>>> {
    Ok(Json(state.db.servers().await?))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateServer>,
) -> ApiResult<Json<Server>> {
    validate_name(&body.name).map_err(AppError::BadRequest)?;
    let host = body.host.trim();
    if host.is_empty() || host.starts_with('-') || host.chars().any(char::is_whitespace) {
        bad_request!("invalid host '{host}'");
    }
    if body.user.is_empty() || body.user.starts_with('-') || body.user.contains(['@', ' ']) {
        bad_request!("invalid SSH user '{}'", body.user);
    }
    if body.port == 0 {
        bad_request!("port must be between 1 and 65535");
    }
    if state.db.server_name_taken(&body.name).await? {
        conflict!("a server named '{}' already exists", body.name);
    }
    let server = Server {
        id: new_id(),
        name: body.name,
        kind: ServerKind::Ssh,
        host: Some(host.to_string()),
        user: Some(body.user),
        port: Some(body.port),
        status: ServerStatus::Unknown,
        proxy: ProxyStatus::Missing,
        docker_version: None,
        last_checked_at: None,
        created_at: Utc::now(),
    };
    state.db.insert_server(&server).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Server {} added", server.name),
            server.host.clone(),
        )
        .await;
    Ok(Json(server))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let server = state.server(&id).await?;
    if server.kind == ServerKind::Local {
        bad_request!("the local server cannot be removed");
    }
    if state.db.server_in_use(&id).await? {
        conflict!(
            "server '{}' still runs services; move or delete them first",
            server.name
        );
    }
    state.db.delete_server(&id).await?;
    state.docker.forget(&id);
    state
        .activity(
            ActivityLevel::Info,
            format!("Server {} removed", server.name),
            None,
        )
        .await;
    Ok(no_content())
}

pub async fn check(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Server>> {
    let server = state.server(&id).await?;
    Ok(Json(check_server(&state, server).await?))
}

pub async fn proxy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Server>> {
    let (server, docker) = state.docker_for(&id).await?;
    if let Err(e) = docker::install_proxy(&docker).await {
        state
            .activity(
                ActivityLevel::Error,
                format!("Proxy setup on {} failed", server.name),
                Some(format!("{e:#}")),
            )
            .await;
        return Err(e.into());
    }
    state
        .activity(
            ActivityLevel::Success,
            format!("Proxy installed on {}", server.name),
            Some(docker::PROXY_IMAGE.to_string()),
        )
        .await;
    Ok(Json(check_server(&state, server).await?))
}

/// Pings Docker on the server and stores status, engine version and proxy status.
pub async fn check_server(state: &AppState, mut server: Server) -> anyhow::Result<Server> {
    let probe = async {
        let docker = state.docker.get(&server).await?;
        let version = docker.version().await?;
        let proxy = docker::proxy_status(&docker).await?;
        anyhow::Ok((version.version, proxy))
    };
    match tokio::time::timeout(CHECK_TIMEOUT, probe).await {
        Ok(Ok((version, proxy))) => {
            server.status = ServerStatus::Online;
            server.docker_version = version;
            server.proxy = proxy;
        }
        Ok(Err(e)) => {
            tracing::warn!(server = %server.name, "server check failed: {e:#}");
            server.status = ServerStatus::Offline;
            state.docker.forget(&server.id);
        }
        Err(_) => {
            tracing::warn!(server = %server.name, "server check timed out");
            server.status = ServerStatus::Offline;
            state.docker.forget(&server.id);
        }
    }
    server.last_checked_at = Some(Utc::now());
    state.db.update_server_check(&server).await?;
    Ok(server)
}
