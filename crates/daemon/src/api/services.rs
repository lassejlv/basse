//! Services and their env vars, domains, volumes, logs and stats.

use super::{Json, Query, ndjson, no_content, optional_json};
use crate::db::new_id;
use crate::deploy::{self, Plan};
use crate::docker;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::services::{self, Power};
use crate::state::AppState;
use crate::stats;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use bollard::query_parameters::LogsOptions;
use chrono::Utc;
use common::api::{CreateDomain, CreateService, CreateVolume, DeployRequest, UpdateService};
use common::*;
use futures_util::StreamExt as _;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateService>,
) -> ApiResult<Json<ServiceDetail>> {
    let deploy_now = body.deploy;
    let rec = services::create(&state, body).await?;
    if deploy_now {
        deploy::enqueue(&state, &rec, "api", Plan::Source).await?;
    }
    Ok(Json(services::detail(&state, &rec.service.id).await?))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ServiceDetail>> {
    Ok(Json(services::detail(&state, &id).await?))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateService>,
) -> ApiResult<Json<ServiceDetail>> {
    services::update(&state, &id, body).await?;
    Ok(Json(services::detail(&state, &id).await?))
}

#[derive(Deserialize)]
pub struct DeleteServiceQuery {
    #[serde(default)]
    volumes: bool,
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<DeleteServiceQuery>,
) -> ApiResult<StatusCode> {
    let rec = services::record(&state, &id).await?;
    services::delete(&state, &rec, q.volumes).await?;
    Ok(no_content())
}

pub async fn deploy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Deployment>> {
    let req: DeployRequest = optional_json(&body)?;
    let trigger = req
        .trigger
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "api".into());
    if trigger.len() > 32
        || !trigger
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bad_request!("invalid trigger '{trigger}'");
    }
    let rec = services::record(&state, &id).await?;
    Ok(Json(
        deploy::enqueue(&state, &rec, &trigger, Plan::Source).await?,
    ))
}

pub async fn restart(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ServiceDetail>> {
    services::power(&state, &id, Power::Restart).await?;
    Ok(Json(services::detail(&state, &id).await?))
}

pub async fn stop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ServiceDetail>> {
    services::power(&state, &id, Power::Stop).await?;
    Ok(Json(services::detail(&state, &id).await?))
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ServiceDetail>> {
    services::power(&state, &id, Power::Start).await?;
    Ok(Json(services::detail(&state, &id).await?))
}

#[derive(Deserialize)]
pub struct LogsQuery {
    tail: Option<usize>,
    #[serde(default)]
    follow: bool,
    /// A compose service of a compose build.
    container: Option<String>,
}

pub async fn logs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Response> {
    let rec = services::record(&state, &id).await?;
    let missing = || {
        AppError::NotFound(format!(
            "'{}' has no container to read logs from",
            rec.service.name
        ))
    };
    let (_, docker) = state.docker_for(&rec.service.server_id).await?;
    let container = match &q.container {
        Some(name) => services::compose_services(&docker, &id)
            .await?
            .into_iter()
            .find(|c| c.name == *name)
            .and_then(|c| c.container)
            .map(|c| c.id)
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "'{}' has no compose service '{name}' running",
                    rec.service.name
                ))
            })?,
        None => rec.live_container.clone().ok_or_else(missing)?,
    };
    if docker::inspect(&docker, &container).await?.is_none() {
        return Err(missing());
    }
    let options = LogsOptions {
        follow: q.follow,
        stdout: true,
        stderr: true,
        timestamps: true,
        tail: q.tail.unwrap_or(200).to_string(),
        ..Default::default()
    };
    let (tx, rx) = mpsc::channel::<LogLine>(512);
    tokio::spawn(async move {
        let mut stream = docker.logs(&container, Some(options));
        loop {
            tokio::select! {
                () = tx.closed() => break,
                item = stream.next() => match item {
                    Some(Ok(chunk)) => {
                        for line in docker::log_lines(&chunk) {
                            if tx.send(line).await.is_err() {
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let line = LogLine { ts: Some(Utc::now()), stream: LogStream::System, line: format!("log stream ended: {e}") };
                        let _ = tx.send(line).await;
                        break;
                    }
                    None => break,
                },
            }
        }
    });
    Ok(ndjson(ReceiverStream::new(rx)))
}

pub async fn stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ServiceStats>> {
    if let Some(s) = state.stats.service(&id) {
        return Ok(Json(s));
    }
    let rec = services::record(&state, &id).await?;
    let sampled = match &rec.live_container {
        Some(container) => match state.docker_for(&rec.service.server_id).await {
            Ok((_, docker)) => stats::sample_now(&docker, container).await,
            Err(_) => None,
        },
        None => None,
    };
    Ok(Json(sampled.unwrap_or(ServiceStats {
        cpu_percent: 0.0,
        mem_used: 0,
        mem_limit: 0,
        cpu_history: Vec::new(),
        mem_history: Vec::new(),
    })))
}

pub async fn set_env(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(vars): Json<Vec<EnvVar>>,
) -> ApiResult<Json<Vec<EnvVar>>> {
    let rec = services::record(&state, &id).await?;
    services::validate_env(&vars)?;
    state.db.replace_env(&id, &vars).await?;
    state
        .service_activity(
            &rec.service,
            ActivityLevel::Info,
            "Environment variables updated".into(),
            Some(format!("{} variables, applied on next deploy", vars.len())),
        )
        .await;
    Ok(Json(state.db.env(&id).await?))
}

pub async fn add_domain(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<CreateDomain>,
) -> ApiResult<Json<Domain>> {
    let rec = services::record(&state, &id).await?;
    let host = services::validate_host(&body.host)?;
    if body.port == Some(0) {
        bad_request!("port must be between 1 and 65535");
    }
    let compose = matches!(&rec.service.source, Source::Git { build, .. } if build.method == BuildMethod::Compose);
    let compose_service = body
        .compose_service
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    match (compose, &compose_service) {
        (true, None) => bad_request!(
            "'{}' is a compose build: say which compose service the domain routes to",
            rec.service.name
        ),
        (false, Some(_)) => bad_request!(
            "'{}' is not a compose build, so the domain cannot name a compose service",
            rec.service.name
        ),
        _ => {}
    }
    if state.db.domain_taken(&host).await? {
        conflict!("domain '{host}' is already in use");
    }
    let domain = Domain {
        id: new_id(),
        service_id: id,
        host,
        port: body.port,
        tls: body.tls,
        compose_service,
        created_at: Utc::now(),
    };
    state.db.insert_domain(&domain).await?;
    state
        .service_activity(
            &rec.service,
            ActivityLevel::Info,
            "Domain added".into(),
            Some(domain.host.clone()),
        )
        .await;
    deploy::reconfigure(&state, &rec).await?;
    Ok(Json(domain))
}

pub async fn delete_domain(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let domain = state
        .db
        .domain(&id)
        .await?
        .ok_or_else(|| AppError::not_found("domain", &id))?;
    let rec = services::record(&state, &domain.service_id).await?;
    state.db.delete_domain(&id).await?;
    state
        .service_activity(
            &rec.service,
            ActivityLevel::Info,
            "Domain removed".into(),
            Some(domain.host),
        )
        .await;
    deploy::reconfigure(&state, &rec).await?;
    Ok(no_content())
}

pub async fn add_volume(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<CreateVolume>,
) -> ApiResult<Json<Volume>> {
    let rec = services::record(&state, &id).await?;
    services::validate_volume(&body.name, &body.mount_path)?;
    if state.db.volume_name_taken(&id, &body.name).await? {
        conflict!(
            "'{}' already has a volume named '{}'",
            rec.service.name,
            body.name
        );
    }
    if state
        .db
        .volumes(&id)
        .await?
        .iter()
        .any(|v| v.mount_path == body.mount_path)
    {
        conflict!("another volume is already mounted at {}", body.mount_path);
    }
    let volume = Volume {
        id: new_id(),
        service_id: id,
        name: body.name,
        mount_path: body.mount_path,
        created_at: Utc::now(),
    };
    state.db.insert_volume(&volume).await?;
    state
        .service_activity(
            &rec.service,
            ActivityLevel::Info,
            "Volume added".into(),
            Some(format!(
                "{} at {}, mounted on next deploy",
                volume.name, volume.mount_path
            )),
        )
        .await;
    Ok(Json(volume))
}

#[derive(Deserialize)]
pub struct DeleteVolumeQuery {
    #[serde(default)]
    purge: bool,
}

pub async fn delete_volume(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<DeleteVolumeQuery>,
) -> ApiResult<StatusCode> {
    let volume = state
        .db
        .volume(&id)
        .await?
        .ok_or_else(|| AppError::not_found("volume", &id))?;
    let rec = services::record(&state, &volume.service_id).await?;
    if q.purge {
        let (_, docker) = state.docker_for(&rec.service.server_id).await?;
        let name = docker::volume_name(&rec.service.id, &volume.name);
        if let Err(e) = docker::remove_volume(&docker, &name).await {
            conflict!(
                "cannot purge volume '{}' while a container uses it; redeploy first ({e:#})",
                volume.name
            );
        }
    }
    state.db.delete_volume(&id).await?;
    let detail = if q.purge { "data purged" } else { "data kept" };
    state
        .service_activity(
            &rec.service,
            ActivityLevel::Info,
            format!("Volume {} removed", volume.name),
            Some(detail.into()),
        )
        .await;
    Ok(no_content())
}
