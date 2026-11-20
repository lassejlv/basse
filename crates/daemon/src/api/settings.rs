//! The instance itself: settings, token rotation, the local proxy, pruning, releases and updates.

use super::{Json, optional_json, servers};
use crate::config;
use crate::docker;
use crate::error::{ApiResult, AppError};
use crate::state::AppState;
use crate::{prune, update};
use axum::body::Bytes;
use axum::extract::State;
use common::api::{RotatedToken, UpdateRequest, UpdateSettings};
use common::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DOCKER_TIMEOUT: Duration = Duration::from_secs(10);
const RELEASE_CACHE: Duration = Duration::from_secs(3600);

pub async fn get(State(state): State<Arc<AppState>>) -> ApiResult<Json<InstanceSettings>> {
    Ok(Json(settings(&state).await?))
}

async fn settings(state: &AppState) -> ApiResult<InstanceSettings> {
    let config = state.config();
    let local = state.db.local_server().await?;
    let docker = match &local {
        Some(server) => state.docker.get(server).await.ok(),
        None => None,
    };
    let (mut proxy, mut docker_version, mut reclaimable) = (None, None, None);
    if let Some(d) = &docker {
        let deployments = deployments_by_service(state).await?;
        proxy = tokio::time::timeout(DOCKER_TIMEOUT, docker::proxy_status(d))
            .await
            .ok()
            .and_then(Result::ok);
        docker_version = tokio::time::timeout(DOCKER_TIMEOUT, d.version())
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|v| v.version);
        reclaimable = tokio::time::timeout(DOCKER_TIMEOUT, prune::reclaimable(d, &deployments))
            .await
            .ok()
            .and_then(Result::ok);
    }
    let root = state.dirs.root().to_path_buf();
    let data_dir_size = tokio::task::spawn_blocking(move || dir_size(&root))
        .await
        .unwrap_or(0);
    Ok(InstanceSettings {
        version: env!("CARGO_PKG_VERSION").to_string(),
        hostname: crate::host::hostname(),
        public_ip: config.public_ip(),
        public_ip_override: config.public_ip_override.clone(),
        listen: state.listen.clone(),
        data_dir: state.dirs.root().display().to_string(),
        data_dir_size,
        manage_proxy: config.manage_proxy,
        proxy: proxy
            .or(local.as_ref().map(|s| s.proxy))
            .unwrap_or_default(),
        proxy_image: docker::PROXY_IMAGE.to_string(),
        docker_version: docker_version.or(local.and_then(|s| s.docker_version)),
        ssh_public_key: std::fs::read_to_string(state.dirs.ssh_public_key())
            .ok()
            .map(|k| k.trim().to_string()),
        reclaimable,
        supervised: update::supervised(),
    })
}

/// Total size of the files under `path`, without following symlinks.
fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(m) if m.is_dir() => dir_size(&entry.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

async fn deployments_by_service(state: &AppState) -> ApiResult<HashMap<Id, Vec<Deployment>>> {
    let mut all = HashMap::new();
    for rec in state.db.services().await? {
        let deployments = state.db.deployments(&rec.service.id, usize::MAX).await?;
        all.insert(rec.service.id, deployments);
    }
    Ok(all)
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateSettings>,
) -> ApiResult<Json<InstanceSettings>> {
    let (config, was_managing) = state.update_config(|c| {
        let was = c.manage_proxy;
        c.apply(&body).map_err(AppError::BadRequest)?;
        Ok(was)
    })?;
    if let Some(ip) = &body.public_ip_override {
        state
            .activity(
                ActivityLevel::Info,
                "Public IP changed".into(),
                Some(
                    config
                        .public_ip_override
                        .clone()
                        .unwrap_or_else(|| format!("auto-detected (was set to '{ip}')")),
                ),
            )
            .await;
    }
    if let Some(on) = body.manage_proxy {
        state
            .activity(
                ActivityLevel::Info,
                format!("Proxy auto-manage turned {}", if on { "on" } else { "off" }),
                None,
            )
            .await;
    }
    if config.manage_proxy
        && !was_managing
        && let Some(local) = state.db.local_server().await?
        && local.proxy != ProxyStatus::Running
    {
        let state = state.clone();
        tokio::spawn(async move { servers::ensure_local_proxy(&state, local).await });
    }
    Ok(Json(settings(&state).await?))
}

pub async fn rotate_token(State(state): State<Arc<AppState>>) -> ApiResult<Json<RotatedToken>> {
    let token = config::generate_token();
    state.update_config(|c| {
        c.token_sha256 = hex::encode(config::hash_token(&token));
        Ok(())
    })?;
    state
        .activity(ActivityLevel::Info, "API token rotated".into(), None)
        .await;
    Ok(Json(RotatedToken { token }))
}

pub async fn restart_proxy(State(state): State<Arc<AppState>>) -> ApiResult<Json<Server>> {
    let local = state
        .db
        .local_server()
        .await?
        .ok_or_else(|| AppError::NotFound("no local server is registered".into()))?;
    Ok(Json(servers::install_proxy(&state, local).await?))
}

/// Prunes the local server and every other server that answers; an unreachable remote server is
/// skipped rather than failing the whole prune.
pub async fn prune(State(state): State<Arc<AppState>>) -> ApiResult<Json<PruneResult>> {
    let deployments = deployments_by_service(&state).await?;
    let mut total = PruneResult {
        reclaimed_bytes: 0,
        removed_images: Vec::new(),
    };
    for server in state.db.servers().await? {
        let local = server.kind == ServerKind::Local;
        let result = async {
            let docker = tokio::time::timeout(DOCKER_TIMEOUT, state.docker.get(&server))
                .await
                .map_err(|_| anyhow::anyhow!("timed out connecting"))??;
            prune::prune(&docker, &deployments).await
        }
        .await;
        match result {
            Ok(r) => {
                total.reclaimed_bytes += r.reclaimed_bytes;
                total.removed_images.extend(r.removed_images);
            }
            Err(e) if local => return Err(e.into()),
            Err(e) => tracing::warn!(server = %server.name, "prune skipped: {e:#}"),
        }
    }
    state
        .activity(
            ActivityLevel::Success,
            format!("Pruned {} images", total.removed_images.len()),
            Some(format!("{} MB freed", total.reclaimed_bytes / 1_000_000)),
        )
        .await;
    Ok(Json(total))
}

pub async fn version(State(state): State<Arc<AppState>>) -> Json<VersionCheck> {
    let cached = state
        .latest_release
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .filter(|(at, _)| at.elapsed() < RELEASE_CACHE);
    if let Some((_, check)) = cached {
        return Json(check);
    }
    let latest = tokio::time::timeout(Duration::from_secs(10), async {
        release::Releases::new()?.latest_tag().await
    })
    .await
    .ok()
    .and_then(|r| {
        r.map_err(|e| tracing::warn!("release check failed: {e:#}"))
            .ok()
    });
    let current = env!("CARGO_PKG_VERSION");
    let check = VersionCheck {
        current: current.to_string(),
        update_available: latest
            .as_deref()
            .is_some_and(|l| release::is_newer(l, current)),
        latest,
    };
    if check.latest.is_some() {
        *state
            .latest_release
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), check.clone()));
    }
    Json(check)
}

pub async fn self_update(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> ApiResult<Json<UpdateResult>> {
    let req: UpdateRequest = optional_json(&body)?;
    Ok(Json(update::run(&state, req).await?))
}
