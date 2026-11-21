//! Services and their env vars, domains, volumes, logs, stats and metrics.

use super::{Json, Query, ndjson, no_content, optional_json};
use crate::db::new_id;
use crate::deploy::{self, Plan};
use crate::docker;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::metrics;
use crate::replicas;
use crate::services::{self, Power};
use crate::state::AppState;
use crate::stats;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use bollard::query_parameters::LogsOptions;
use chrono::Utc;
use common::api::{
    CreateDomain, CreateService, CreateVolume, DeployRequest, ScaleRequest, UpdateService,
};
use common::metrics::{MetricsRange, ServiceMetrics};
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
    /// A replica by index.
    replica: Option<u16>,
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
        None => match q.replica {
            Some(index) => rec
                .replica_containers()
                .into_iter()
                .find(|(i, _)| *i == index)
                .map(|(_, id)| id)
                .ok_or_else(|| {
                    AppError::NotFound(format!("'{}' has no replica #{index}", rec.service.name))
                })?,
            None => rec.live_container.clone().ok_or_else(missing)?,
        },
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

/// The Overview's figures: the last hour of raw samples, summed over the service's containers.
pub async fn stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ServiceStats>> {
    let rec = services::record(&state, &id).await?;
    let hour = MetricsRange::Hour;
    let from = Utc::now() - chrono::TimeDelta::seconds(hour.secs());
    let buckets = state
        .db
        .metric_buckets(&id, hour.step_secs(), from, None)
        .await?;
    let points = metrics::combine(&buckets);
    if let Some(last) = points.last() {
        return Ok(Json(ServiceStats {
            cpu_percent: last.cpu as f32,
            mem_used: last.mem,
            mem_limit: last.mem_limit,
            cpu_history: points.iter().map(|p| p.cpu as f32).collect(),
            mem_history: points.iter().map(|p| p.mem).collect(),
        }));
    }
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

#[derive(Deserialize)]
pub struct MetricsQuery {
    range: Option<String>,
    container: Option<String>,
    replica: Option<u16>,
}

pub async fn metrics(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<MetricsQuery>,
) -> ApiResult<Json<ServiceMetrics>> {
    let range = match q.range.as_deref() {
        None => MetricsRange::default(),
        Some(r) => match r.parse() {
            Ok(r) => r,
            Err(e) => bad_request!("{e}"),
        },
    };
    let rec = services::record(&state, &id).await?;
    let container = q.container.as_deref().filter(|c| !c.is_empty());
    if container.is_some() && !metrics::is_compose(&rec.service.source) {
        bad_request!("only compose services have containers to pick");
    }
    if container.is_some() && q.replica.is_some() {
        bad_request!("pick a container or a replica, not both");
    }
    Ok(Json(
        metrics::service_metrics(&state, &id, range, container, q.replica, Utc::now()).await?,
    ))
}

pub async fn scale(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ScaleRequest>,
) -> ApiResult<Json<ServiceDetail>> {
    let rec = services::record(&state, &id).await?;
    if rec.service.scaling.autoscale.enabled {
        conflict!(
            "autoscaling manages the replicas of '{}'; turn it off first",
            rec.service.name
        );
    }
    replicas::scale(&state, &rec, body.replicas, "manual".into()).await?;
    Ok(Json(services::detail(&state, &id).await?))
}

pub async fn autoscale(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Autoscale>,
) -> ApiResult<Json<ServiceDetail>> {
    body.validate().map_err(AppError::BadRequest)?;
    let rec = services::record(&state, &id).await?;
    let mut svc = rec.service.clone();
    if body.enabled && body.max > 1 {
        if !replicas::supports_replicas(&svc.source) {
            bad_request!("databases and compose services run one replica");
        }
        replicas::check_volumes(&state, &svc).await?;
    }
    svc.scaling.autoscale = body.clone();
    state.db.update_service(&svc).await?;
    let enabled = if body.enabled { "on" } else { "off" };
    state
        .service_activity(
            &svc,
            ActivityLevel::Info,
            format!("Autoscaling {enabled}"),
            body.enabled.then(|| body.summary()),
        )
        .await;
    if body.enabled {
        let bounded = svc.replicas.clamp(body.min, body.max);
        if bounded != svc.replicas {
            state.db.set_replica_count(&id, bounded).await?;
            replicas::spawn_reconcile(
                &state,
                &id,
                format!("autoscale: within {}–{}", body.min, body.max),
            );
        }
    }
    Ok(Json(services::detail(&state, &id).await?))
}

pub async fn capacity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ScaleCapacity>> {
    let rec = services::record(&state, &id).await?;
    Ok(Json(replicas::capacity(&state, &rec).await?))
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
    let svc = &rec.service;
    if (svc.replicas > 1 || svc.scaling.autoscale.enabled && svc.scaling.autoscale.max > 1)
        && !svc.scaling.shared_volume_ok
    {
        conflict!(
            "'{}' runs several replicas, which would all write this volume; set shared_volume_ok or scale to 1 first",
            svc.name
        );
    }
    let volume = Volume {
        id: new_id(),
        service_id: id,
        name: body.name,
        mount_path: body.mount_path,
        created_at: Utc::now(),
        compose: None,
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
        if let Err(e) = docker::remove_volume(&docker, &volume.docker_name()).await {
            let fix = if volume.compose.is_some() {
                "stop the service first"
            } else {
                "redeploy first"
            };
            conflict!(
                "cannot purge volume '{}' while a container uses it; {fix} ({e:#})",
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
