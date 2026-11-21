//! Servers: registration, health checks and proxy installation.

use super::secrets::secret_of_kind;
use super::{Json, Query, no_content};
use crate::db::new_id;
use crate::docker;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::notify::Event;
use crate::preflight;
use crate::services;
use crate::ssh::Unreachable;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse as _, Response};
use chrono::Utc;
use common::api::{CreateServer, UpdateServer};
use common::notify::NotifyEvent;
use common::*;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// Long enough for the preflight, the tunnel and a ping, each bounded on its own.
const CHECK_TIMEOUT: Duration = Duration::from_secs(45);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

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
    if let Some(id) = &body.ssh_key_secret_id {
        secret_of_kind(&state, id, SecretKind::SshKey).await?;
    }
    let server = Server {
        id: new_id(),
        name: body.name,
        kind: ServerKind::Ssh,
        host: Some(host.to_string()),
        user: Some(body.user),
        port: Some(body.port),
        ssh_key_secret_id: body.ssh_key_secret_id,
        status: ServerStatus::Unknown,
        proxy: ProxyStatus::Missing,
        docker_version: None,
        last_checked_at: None,
        created_at: Utc::now(),
        checks: Vec::new(),
        problem: None,
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

/// Switches the key a server connects with, then reconnects and checks it.
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateServer>,
) -> ApiResult<Json<Server>> {
    let mut server = state.server(&id).await?;
    let Some(key) = body.ssh_key_secret_id else {
        return Ok(Json(server));
    };
    if server.kind == ServerKind::Local {
        bad_request!("the local server is reached without SSH");
    }
    let key = Some(key).filter(|k| !k.is_empty());
    let label = match &key {
        Some(key) => secret_of_kind(&state, key, SecretKind::SshKey).await?.name,
        None => "the daemon's own key".to_string(),
    };
    state.db.set_server_key(&id, key.as_deref()).await?;
    state.docker.forget(&id);
    server.ssh_key_secret_id = key;
    state
        .activity(
            ActivityLevel::Info,
            format!("Server {} now connects with {label}", server.name),
            None,
        )
        .await;
    Ok(Json(check_server(&state, server).await?))
}

#[derive(Deserialize)]
pub struct DeleteServerQuery {
    #[serde(default)]
    purge: bool,
    #[serde(default)]
    volumes: bool,
}

/// Forgets a server. With `purge`, first deletes the services assigned to it and removes every
/// container, network and (with `volumes`) volume the daemon created there.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<DeleteServerQuery>,
) -> ApiResult<Response> {
    let server = state.server(&id).await?;
    if server.kind == ServerKind::Local {
        bad_request!("the local server cannot be removed");
    }
    if !q.purge && state.db.server_in_use(&id).await? {
        conflict!(
            "server '{}' still runs services; move or delete them first, or purge it",
            server.name
        );
    }
    let cleanup = if q.purge {
        Some(purge(&state, &server, q.volumes).await?)
    } else {
        None
    };
    state.db.delete_server(&id).await?;
    state.docker.forget(&id);
    state
        .activity(
            ActivityLevel::Info,
            format!("Server {} removed", server.name),
            cleanup.as_ref().map(|c| {
                format!(
                    "{} containers, {} networks, {} volumes removed",
                    c.containers, c.networks, c.volumes
                )
            }),
        )
        .await;
    Ok(match cleanup {
        Some(c) => Json(c).into_response(),
        None => no_content().into_response(),
    })
}

async fn purge(state: &Arc<AppState>, server: &Server, volumes: bool) -> ApiResult<ServerCleanup> {
    let docker = match tokio::time::timeout(CHECK_TIMEOUT, state.docker.get(server)).await {
        Ok(Ok(docker)) => docker,
        Ok(Err(e)) => {
            return Err(AppError::Conflict(format!(
                "cannot reach {}: {e:#}",
                server.name
            )));
        }
        Err(_) => {
            return Err(AppError::Conflict(format!(
                "cannot reach {}: timed out",
                server.name
            )));
        }
    };
    let on_server: Vec<_> = state
        .db
        .services()
        .await?
        .into_iter()
        .filter(|rec| rec.service.server_id == server.id)
        .collect();
    for rec in &on_server {
        services::delete(state, rec, volumes).await?;
    }
    let mut cleanup = docker::purge(&docker, volumes).await?;
    cleanup.services = on_server.len();
    Ok(cleanup)
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
    let server = state.server(&id).await?;
    Ok(Json(install_proxy(&state, server).await?))
}

/// Installs Docker on a server where the last check found it missing and the SSH user may
/// install it, then checks the server again.
pub async fn install_docker(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Server>> {
    let server = state.server(&id).await?;
    if server.kind == ServerKind::Local {
        bad_request!("the local server runs the daemon's own Docker");
    }
    let server = check_server(&state, server).await?;
    match &server.problem {
        None => conflict!("docker already runs on {}", server.name),
        Some(p) if !p.can_install_docker => {
            conflict!("cannot install docker on {}: {}", server.name, p.summary)
        }
        Some(_) => {}
    }
    state
        .activity(
            ActivityLevel::Info,
            format!("Installing Docker on {}", server.name),
            Some("get.docker.com".into()),
        )
        .await;
    if let Err(e) = state
        .docker
        .run_script(&server, preflight::INSTALL_SCRIPT, INSTALL_TIMEOUT)
        .await
    {
        let message = format!("{e:#}");
        state
            .activity(
                ActivityLevel::Error,
                format!("Docker install on {} failed", server.name),
                Some(message.clone()),
            )
            .await;
        return Err(AppError::Conflict(format!(
            "cannot install docker on {}: {message}",
            server.name
        )));
    }
    state.docker.forget(&id);
    let server = check_server(&state, server).await?;
    state
        .activity(
            ActivityLevel::Success,
            format!("Docker installed on {}", server.name),
            server.docker_version.clone(),
        )
        .await;
    Ok(Json(server))
}

/// Installs or reinstalls the proxy on a server and records the outcome.
pub async fn install_proxy(state: &AppState, server: Server) -> ApiResult<Server> {
    let docker = state.docker.get(&server).await?;
    let config = state.proxy_config(&server).await;
    let installed = async {
        let pages = state.proxy_pages(&server.id).await?;
        docker::install_proxy(&docker, &config, pages).await
    };
    if let Err(e) = installed.await {
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
    Ok(check_server(state, server).await?)
}

/// Installs the proxy on this machine so domains work without a manual step.
/// Retries a few times because a fresh server may still be pulling images or settling its network.
pub async fn ensure_local_proxy(state: &AppState, server: Server) {
    const ATTEMPTS: u32 = 5;
    for attempt in 1..=ATTEMPTS {
        tracing::info!(
            "installing the proxy on {} (attempt {attempt})",
            server.name
        );
        let result = async {
            let docker = state.docker.get(&server).await?;
            let pages = state.proxy_pages(&server.id).await?;
            docker::install_proxy(&docker, &state.proxy_config(&server).await, pages).await
        }
        .await;
        let e = match result {
            Ok(()) => {
                state
                    .activity(
                        ActivityLevel::Success,
                        format!("Proxy installed on {}", server.name),
                        Some(docker::PROXY_IMAGE.to_string()),
                    )
                    .await;
                let _ = check_server(state, server).await;
                return;
            }
            Err(e) => e,
        };
        tracing::warn!("proxy setup failed: {e:#}");
        if attempt == ATTEMPTS {
            state
                .activity(
                    ActivityLevel::Error,
                    format!("Proxy setup on {} failed", server.name),
                    Some(format!("{e:#}")),
                )
                .await;
            return;
        }
        tokio::time::sleep(Duration::from_secs(u64::from(attempt) * 10)).await;
    }
}

/// Recreates a running proxy whose configuration is out of date: after an upgrade that changed
/// it, or when the instance domain moved behind Cloudflare's proxy or out from behind it. An
/// up-to-date proxy gets this daemon's pages written into it.
pub async fn sync_proxy(state: &AppState, server: Server) -> ApiResult<()> {
    let docker = state.docker.get(&server).await?;
    let Some(now) = docker::proxy_labels_now(&docker).await? else {
        return Ok(());
    };
    if now == docker::proxy_container_labels(&state.proxy_config(&server).await) {
        docker::write_proxy_pages(&docker, state.proxy_pages(&server.id).await?).await?;
        return Ok(());
    }
    tracing::info!("the proxy on {} is out of date; recreating it", server.name);
    install_proxy(state, server).await?;
    Ok(())
}

/// Pings Docker on the server and stores status, engine version and proxy status.
pub async fn check_server(state: &AppState, mut server: Server) -> anyhow::Result<Server> {
    let before = server.status;
    let mut failure = None;
    let probe = async {
        let docker = state.docker.get(&server).await?;
        let version = docker.version().await?;
        let proxy = docker::proxy_status(&docker).await?;
        anyhow::Ok((version.version, proxy))
    };
    let outcome = match tokio::time::timeout(CHECK_TIMEOUT, probe).await {
        Ok(outcome) => outcome,
        Err(_) => Err(anyhow::anyhow!(
            "no answer within {}s",
            CHECK_TIMEOUT.as_secs()
        )),
    };
    match outcome {
        Ok((version, proxy)) => {
            server.status = ServerStatus::Online;
            server.docker_version = version;
            server.proxy = proxy;
            server.checks = state.docker.checks(&server.id);
            server.problem = None;
        }
        Err(e) => {
            tracing::warn!(server = %server.name, "server check failed: {e:#}");
            failure = Some(format!("{e:#}"));
            server.status = ServerStatus::Offline;
            (server.checks, server.problem) = match e.downcast_ref::<Unreachable>() {
                Some(u) => (u.checks.clone(), Some(u.problem.clone())),
                None => (
                    Vec::new(),
                    Some(ServerProblem {
                        summary: e.to_string(),
                        fix: None,
                        detail: format!("{e:#}"),
                        can_install_docker: false,
                    }),
                ),
            };
            state.docker.forget(&server.id);
        }
    }
    server.last_checked_at = Some(Utc::now());
    state.db.update_server_check(&server).await?;
    let event = |kind| {
        let mut e = Event::new(kind).about(server.name.clone(), server.id.clone());
        if let Some(host) = &server.host {
            e = e.field("Host", host.clone());
        }
        e
    };
    if before != server.status {
        match server.status {
            ServerStatus::Online => state.notify(event(NotifyEvent::ServerConnected).detail(
                format!("Docker {}", server.docker_version.as_deref().unwrap_or("?")),
            )),
            ServerStatus::Offline => state.notify(
                event(NotifyEvent::ServerUnreachable)
                    .detail(failure.unwrap_or_default())
                    .hint(format!("{} server check {}", brand::CLI_BIN, server.name)),
            ),
            ServerStatus::Unknown => {}
        }
    }
    Ok(server)
}
