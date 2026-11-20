//! Daemon-wide reads: host info, the overview tree and the activity feed.

use super::{Json, Query};
use crate::docker;
use crate::error::ApiResult;
use crate::host;
use crate::services::with_states;
use crate::state::AppState;
use axum::extract::State;
use common::*;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

pub async fn info(State(state): State<Arc<AppState>>) -> ApiResult<Json<DaemonInfo>> {
    let sample = state.stats.host();
    let proxy = match state.db.local_server().await? {
        Some(server) => match state.docker.get(&server).await {
            Ok(d) => tokio::time::timeout(Duration::from_secs(5), docker::proxy_status(&d))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or(server.proxy),
            Err(_) => server.proxy,
        },
        None => ProxyStatus::Missing,
    };
    let ssh_public_key = std::fs::read_to_string(state.dirs.ssh_public_key())
        .ok()
        .map(|k| k.trim().to_string());
    Ok(Json(DaemonInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        hostname: host::hostname(),
        public_ip: host::public_ip(),
        cpu_percent: sample.cpu_percent,
        mem_used: sample.mem_used,
        mem_total: sample.mem_total,
        disk_used: sample.disk_used,
        disk_total: sample.disk_total,
        cpu_history: sample.cpu_history.into_iter().collect(),
        proxy,
        ssh_public_key,
    }))
}

pub async fn tree(State(state): State<Arc<AppState>>) -> ApiResult<Json<Tree>> {
    let services = with_states(&state, state.db.services().await?).await;
    let environments = state.db.environments().await?;
    let projects = state
        .db
        .projects()
        .await?
        .into_iter()
        .map(|project| {
            let environments = environments
                .iter()
                .filter(|e| e.project_id == project.id)
                .map(|environment| EnvironmentNode {
                    services: services
                        .iter()
                        .filter(|s| s.environment_id == environment.id)
                        .cloned()
                        .collect(),
                    environment: environment.clone(),
                })
                .collect();
            ProjectNode {
                project,
                environments,
            }
        })
        .collect();
    Ok(Json(Tree { projects }))
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    limit: Option<usize>,
}

pub async fn activity(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ActivityQuery>,
) -> ApiResult<Json<Vec<Activity>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 1000);
    Ok(Json(state.db.activity(limit).await?))
}
