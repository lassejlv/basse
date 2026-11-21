//! Replicas: the identical containers of a service behind the proxy's load balancer.
//!
//! Deploys roll replicas over one batch at a time (see [`crate::deploy`]). This module builds
//! their containers, waits for them to be healthy, puts them behind the proxy or drains them,
//! keeps the service's route container in step, and scales a live service without a deploy.

use crate::db::{ReplicaRow, ServiceRecord};
use crate::docker::{self, ContainerSpec, RunState};
use crate::error::{ApiResult, bad_request, conflict};
use crate::notify::Event;
use crate::state::AppState;
use anyhow::{Context as _, Result, anyhow, bail};
use bollard::Docker;
use chrono::Utc;
use common::notify::NotifyEvent;
use common::*;
use futures_util::future::join_all;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a new replica has to pass its health check.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);
/// Without a health path, how long a new replica must keep running.
pub const STABLE_FOR: Duration = Duration::from_secs(5);
/// Pause after attaching replicas to the proxy, so it has reloaded before others leave it.
pub const PROXY_SETTLE: Duration = Duration::from_secs(2);
/// Pause between taking replicas out of the proxy and stopping them, for requests in flight.
pub const DRAIN: Duration = Duration::from_secs(3);
/// Seconds a drained replica gets to shut down.
const STOP_SECS: i32 = 10;
/// The least memory one more replica is expected to need.
const MIN_REPLICA_MEMORY: u64 = 64 << 20;
/// Share of a server's memory left to the system when counting room for replicas.
const RESERVE_SHARE: f64 = 0.1;
/// Metrics older than this don't describe a replica any more.
const FRESH_SECS: i64 = 30;

/// Whether a service can run more than one replica: databases own their data volume and
/// compose groups name their containers themselves.
pub fn supports_replicas(source: &Source) -> bool {
    match source {
        Source::Database { .. } => false,
        Source::Git { build, .. } => build.method != BuildMethod::Compose,
        Source::Image { .. } => true,
    }
}

/// The name metrics keep a replica's samples under.
pub fn metrics_key(index: u16) -> String {
    format!("#{index}")
}

/// The replica index of a metrics key written by [`metrics_key`].
pub fn index_of_key(key: &str) -> Option<u16> {
    key.strip_prefix('#')?.parse().ok()
}

/// The container of replica `index` running `image` for deployment `number`. Creates the
/// environment network and the service's volumes when they are missing.
#[allow(clippy::too_many_arguments)]
pub async fn container_spec(
    state: &AppState,
    docker: &Docker,
    svc: &Service,
    env: &Environment,
    image: &str,
    deployment_id: &str,
    number: i64,
    index: u16,
) -> Result<ContainerSpec> {
    let db = &state.db;
    let domains = db.domains(&svc.id).await?;
    let volumes = db.volumes(&svc.id).await?;
    let vars = db.env(&svc.id).await?;
    let published = match (db.public_port(&svc.id).await?, svc.port) {
        (Some(host_port), Some(port)) => vec![(format!("{port}/tcp"), host_port)],
        _ => Vec::new(),
    };
    let env_net = docker::env_network(&env.id);
    docker::ensure_network(docker, &env_net).await?;
    let mut labels = docker::upstream_labels(&domains, svc.port);
    labels.insert(brand::label("managed"), "true".into());
    labels.insert(brand::label("service"), svc.id.clone());
    labels.insert(brand::label("deployment"), deployment_id.to_string());
    labels.insert(brand::label("replica"), index.to_string());
    let mut binds = Vec::with_capacity(volumes.len());
    for v in &volumes {
        let name = docker::volume_name(&svc.id, &v.name);
        docker::ensure_volume(docker, &name).await?;
        binds.push(format!("{name}:{}", v.mount_path));
    }
    let mut env_vars: Vec<String> = vars
        .iter()
        .map(|v| format!("{}={}", v.key, v.value))
        .collect();
    env_vars.push(format!("{}={index}", brand::env_var("REPLICA")));
    Ok(ContainerSpec {
        name: docker::container_name(&svc.name, &env.id, number, index),
        image: image.to_string(),
        env: env_vars,
        cmd: svc
            .command
            .clone()
            .map(|c| vec!["sh".into(), "-c".into(), c]),
        labels,
        binds,
        restart: svc.restart,
        networks: vec![(env_net, vec![svc.name.clone()])],
        published,
        publish_ip: None,
        extra_hosts: Vec::new(),
    })
}

/// How a new replica's health check ended.
pub enum Health {
    Passed(String),
    /// A remote image without `wget` cannot be probed from inside.
    Skipped,
}

/// Waits until a new replica answers its health path, or keeps running for [`STABLE_FOR`]
/// without one. `waiting` gets a line on every failed probe.
pub async fn wait_healthy(
    state: &AppState,
    docker: &Docker,
    server: &Server,
    svc: &Service,
    env: &Environment,
    container: &str,
    waiting: impl Fn(String),
) -> Result<Health> {
    let (Some(path), Some(port)) = (svc.health_path.as_deref(), svc.port) else {
        let deadline = Instant::now() + STABLE_FOR;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_secs(1)).await;
            ensure_running(docker, container).await?;
        }
        return Ok(Health::Passed(format!(
            "running for {}s",
            STABLE_FOR.as_secs()
        )));
    };
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    let mut last_error = String::from("no response");
    loop {
        let inspected = ensure_running(docker, container).await?;
        if server.kind == ServerKind::Local {
            if let Some(ip) = docker::container_ip(&inspected, &docker::env_network(&env.id)) {
                match state
                    .http
                    .get(format!("http://{ip}:{port}{path}"))
                    .send()
                    .await
                {
                    Ok(res) if res.status().is_success() || res.status().is_redirection() => {
                        return Ok(Health::Passed(format!(
                            "GET {path} -> {}",
                            res.status().as_u16()
                        )));
                    }
                    Ok(res) => last_error = format!("HTTP {}", res.status()),
                    Err(e) => last_error = describe_reqwest_error(&e),
                }
            }
        } else {
            let url = format!("http://127.0.0.1:{port}{path}");
            let cmd = ["wget", "-q", "-O", "/dev/null", "-T", "3", &url]
                .map(String::from)
                .to_vec();
            match docker::exec(docker, container, cmd).await? {
                0 => return Ok(Health::Passed(format!("GET {path} -> ok"))),
                126 | 127 => return Ok(Health::Skipped),
                code => last_error = format!("wget exited with {code}"),
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "health check GET {path} on port {port} failed for {}s: {last_error}",
                HEALTH_TIMEOUT.as_secs()
            );
        }
        waiting(format!(
            "Waiting for GET {path} on port {port}: {last_error}"
        ));
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Fails unless the container is running and has not restarted.
pub async fn ensure_running(
    docker: &Docker,
    container: &str,
) -> Result<bollard::models::ContainerInspectResponse> {
    let inspected = docker::inspect(docker, container)
        .await?
        .context("the container disappeared")?;
    match RunState::from_inspect(&inspected) {
        RunState::Running if inspected.restart_count.unwrap_or(0) == 0 => Ok(inspected),
        RunState::Running | RunState::Restarting => bail!("the container is crash-looping"),
        RunState::Exited { code } => {
            bail!(
                "the container exited with code {}",
                code.map_or("?".into(), |c| c.to_string())
            )
        }
        RunState::Other => bail!("the container is not running"),
    }
}

fn describe_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection refused".into()
    } else {
        e.to_string()
    }
}

/// Puts a replica behind the proxy.
pub async fn route(docker: &Docker, service_id: &str, container: &str) -> Result<()> {
    docker::connect_to_proxy(docker, container, &docker::proxy_alias(service_id)).await
}

/// Takes replicas out of the proxy, waits for requests in flight, then stops them. Containers
/// that are already gone are skipped.
pub async fn drain(docker: &Docker, containers: &[String]) -> Result<()> {
    if containers.is_empty() {
        return Ok(());
    }
    for id in containers {
        docker::disconnect_from_proxy(docker, id).await?;
    }
    tokio::time::sleep(DRAIN).await;
    for id in containers {
        match docker::stop_container(docker, id, STOP_SECS).await {
            Ok(()) => {}
            Err(e) if is_gone(&e) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn is_gone(e: &anyhow::Error) -> bool {
    e.downcast_ref::<bollard::errors::Error>()
        .is_some_and(docker::is_not_found)
}

/// Brings the service's route container in line with its domains, port, policy and health
/// path (see [`docker::route_labels`]). Compose groups route themselves.
pub async fn apply_route(state: &AppState, docker: &Docker, svc: &Service) -> Result<()> {
    if !supports_replicas(&svc.source) && !matches!(svc.source, Source::Database { .. }) {
        return Ok(());
    }
    let domains = state.db.domains(&svc.id).await?;
    let labels = docker::route_labels(
        &domains,
        svc.port,
        svc.scaling.lb_policy,
        svc.health_path.as_deref(),
    );
    docker::ensure_route(docker, &svc.id, labels).await
}

/// Each replica of a service with its container, health and newest usage.
pub async fn describe(state: &AppState, docker: &Docker, rec: &ServiceRecord) -> Vec<Replica> {
    if rec.replicas.is_empty() {
        return Vec::new();
    }
    let since = Utc::now() - chrono::TimeDelta::seconds(FRESH_SECS);
    let usage: HashMap<String, (f64, u64)> = state
        .db
        .latest_samples(&rec.service.id, since)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(key, cpu, mem)| (key, (cpu, mem)))
        .collect();
    let inspected = join_all(
        rec.replicas
            .iter()
            .map(|r| docker::inspect(docker, &r.container_id)),
    )
    .await;
    rec.replicas
        .iter()
        .zip(inspected)
        .map(|(r, inspected)| {
            let inspected = inspected.ok().flatten();
            let health = match &inspected {
                None => ReplicaHealth::Down,
                Some(c) => health_of(c),
            };
            let container = inspected.as_ref().map(docker::container_info);
            let routed = container
                .as_ref()
                .is_some_and(|c| c.networks.iter().any(|n| n == brand::PROXY_NETWORK));
            let usage = (health != ReplicaHealth::Down)
                .then(|| usage.get(&metrics_key(r.index)))
                .flatten();
            Replica {
                index: r.index,
                server_id: r.server_id.clone(),
                deployment_id: Some(r.deployment_id.clone()),
                container,
                health,
                routed,
                cpu_percent: usage.map(|u| u.0),
                mem_used: usage.map(|u| u.1),
            }
        })
        .collect()
}

fn health_of(c: &bollard::models::ContainerInspectResponse) -> ReplicaHealth {
    use bollard::models::HealthStatusEnum;
    let docker_health = c
        .state
        .as_ref()
        .and_then(|s| s.health.as_ref())
        .and_then(|h| h.status);
    match RunState::from_inspect(c) {
        RunState::Running if docker_health == Some(HealthStatusEnum::UNHEALTHY) => {
            ReplicaHealth::Unhealthy
        }
        RunState::Running => ReplicaHealth::Healthy,
        RunState::Restarting => ReplicaHealth::Unhealthy,
        _ => ReplicaHealth::Down,
    }
}

/// Room for more replicas of a service on its server, by memory.
pub async fn capacity(state: &AppState, rec: &ServiceRecord) -> Result<ScaleCapacity> {
    let svc = &rec.service;
    let (server, docker) = connect(state, &svc.server_id).await?;
    let total = docker
        .info()
        .await
        .context("cannot read the server's memory")?
        .mem_total
        .and_then(|m| u64::try_from(m).ok())
        .unwrap_or(0);
    let since = Utc::now() - chrono::TimeDelta::seconds(FRESH_SECS);
    let free = if server.kind == ServerKind::Local && state.stats.host().mem_total > 0 {
        let host = state.stats.host();
        host.mem_total.saturating_sub(host.mem_used)
    } else {
        total.saturating_sub(state.db.memory_in_use(&server.id, since).await?)
    };
    let reserve = (total as f64 * RESERVE_SHARE) as u64;
    let free = free.saturating_sub(reserve);
    let per_replica = state
        .db
        .latest_samples(&svc.id, since)
        .await?
        .into_iter()
        .map(|(_, _, mem)| mem)
        .max()
        .unwrap_or(0)
        .max(MIN_REPLICA_MEMORY);
    Ok(ScaleCapacity {
        server_id: server.id,
        mem_total: total,
        mem_free: free,
        per_replica,
        fits_more: u32::try_from(free / per_replica).unwrap_or(u32::MAX),
    })
}

async fn connect(state: &AppState, server_id: &str) -> Result<(Server, Docker)> {
    let server = state
        .db
        .server(server_id)
        .await?
        .context("the server was deleted")?;
    let docker = state.docker.get(&server).await?;
    Ok((server, docker))
}

/// `1.2 GB`, `85 MB`.
pub fn human_bytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{mb:.0} MB")
    }
}

/// Refuses a replica count the service cannot run: out of bounds, a database or compose group,
/// a shared writable volume without the opt-in, or more than the server's memory fits.
pub async fn check_count(state: &AppState, rec: &ServiceRecord, replicas: u16) -> ApiResult<()> {
    let svc = &rec.service;
    if replicas == 0 || replicas > MAX_REPLICAS {
        bad_request!("replicas must be between 1 and {MAX_REPLICAS}");
    }
    if replicas > 1 {
        if !supports_replicas(&svc.source) {
            let what = if matches!(svc.source, Source::Database { .. }) {
                "databases"
            } else {
                "compose services"
            };
            bad_request!("{what} run one replica; replicas are not supported for them yet");
        }
        check_volumes(state, svc).await?;
    }
    let current = rec.replicas.len().max(1);
    let more = usize::from(replicas).saturating_sub(current);
    if more > 0 && rec.service.live_deployment_id.is_some() {
        match capacity(state, rec).await {
            Ok(cap) if more > cap.fits_more as usize => {
                conflict!(
                    "server memory fits {} more replica{} of '{}' (~{} each, {} free), not {more}",
                    cap.fits_more,
                    if cap.fits_more == 1 { "" } else { "s" },
                    svc.name,
                    human_bytes(cap.per_replica),
                    human_bytes(cap.mem_free)
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("cannot check memory for {}: {e:#}", svc.name),
        }
    }
    Ok(())
}

/// More than one replica on a writable volume means several processes writing the same files,
/// which most apps don't expect. Refused unless the service opted in.
pub async fn check_volumes(state: &AppState, svc: &Service) -> ApiResult<()> {
    if svc.scaling.shared_volume_ok {
        return Ok(());
    }
    let volumes = state.db.volumes(&svc.id).await?;
    if volumes.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = volumes
        .iter()
        .map(|v| format!("{} ({})", v.name, v.mount_path))
        .collect();
    conflict!(
        "'{}' mounts the volume{} {}, which every replica would write at once; set shared_volume_ok to allow it",
        svc.name,
        if names.len() == 1 { "" } else { "s" },
        names.join(", ")
    );
}

/// Sets the replica count and, on a live service, adds or drains replicas in the background.
pub async fn scale(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    replicas: u16,
    reason: String,
) -> ApiResult<()> {
    check_count(state, rec, replicas).await?;
    state
        .db
        .set_replica_count(&rec.service.id, replicas)
        .await?;
    spawn_reconcile(state, &rec.service.id, reason);
    Ok(())
}

/// Adds or drains replicas until the service runs its replica count, in the background.
pub fn spawn_reconcile(state: &Arc<AppState>, service_id: &str, reason: String) {
    let state = state.clone();
    let id = service_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = reconcile(&state, &id, &reason).await {
            tracing::warn!(service = %id, "scaling failed: {e:#}");
            if let Ok(Some(rec)) = state.db.service(&id).await {
                state
                    .service_activity(
                        &rec.service,
                        ActivityLevel::Error,
                        "Scaling failed".into(),
                        Some(format!("{e:#}")),
                    )
                    .await;
            }
        }
    });
}

/// Brings the live replica set to the service's replica count under the service's deploy lock.
/// A queued deploy rolls out the new count itself, so this leaves busy services alone.
pub async fn reconcile(state: &Arc<AppState>, service_id: &str, reason: &str) -> Result<()> {
    let lock = state.deploys.lock_for(service_id);
    let _guard = lock.lock().await;
    let Some(rec) = state.db.service(service_id).await? else {
        return Ok(());
    };
    let svc = &rec.service;
    if rec.busy.is_some()
        || rec.stopped
        || svc.live_deployment_id.is_none()
        || !supports_replicas(&svc.source)
    {
        return Ok(());
    }
    let want = svc.replicas.max(1);
    let have = rec.replicas.len();
    if have == usize::from(want) && rec.replicas.iter().all(|r| r.index < want) {
        return Ok(());
    }
    let (server, docker) = connect(state, &svc.server_id).await?;
    let started = Instant::now();
    let mut problems = Vec::new();
    let removing: Vec<&ReplicaRow> = rec.replicas.iter().filter(|r| r.index >= want).collect();
    if !removing.is_empty() {
        for r in &removing {
            state.db.delete_replica(&svc.id, r.index).await?;
        }
        let ids: Vec<String> = removing.iter().map(|r| r.container_id.clone()).collect();
        drain(&docker, &ids).await?;
        for id in &ids {
            if let Err(e) = docker::remove_container(&docker, id).await {
                problems.push(format!("{e:#}"));
            }
        }
    }
    let missing: Vec<u16> = (0..want)
        .filter(|i| !rec.replicas.iter().any(|r| r.index == *i))
        .collect();
    if !missing.is_empty() {
        let live = state
            .db
            .live_deployment(&svc.id)
            .await?
            .context("the service has no live deployment")?;
        let image = live
            .image
            .clone()
            .context("the live deployment has no image")?;
        let env = state
            .db
            .environment(&svc.environment_id)
            .await?
            .context("the environment was deleted")?;
        apply_route(state, &docker, svc).await?;
        let routed =
            !docker::upstream_labels(&state.db.domains(&svc.id).await?, svc.port).is_empty();
        let added = join_all(missing.iter().map(|&index| {
            let (docker, server, env, live, image) = (&docker, &server, &env, &live, &image);
            async move {
                add_replica(state, docker, server, svc, env, live, image, index, routed)
                    .await
                    .map_err(|e| format!("replica #{index}: {e:#}"))
            }
        }))
        .await;
        for result in added {
            if let Err(e) = result {
                problems.push(e);
            }
        }
    }
    let now = state
        .db
        .service(service_id)
        .await?
        .map_or(0, |r| r.replicas.len());
    let from = have;
    let detail = format!(
        "{from} → {now} · {reason} · {}s",
        started.elapsed().as_secs()
    );
    if problems.is_empty() {
        state
            .service_activity(
                svc,
                ActivityLevel::Success,
                format!("Scaled to {now} replica{}", if now == 1 { "" } else { "s" }),
                Some(detail),
            )
            .await;
    } else {
        state
            .service_activity(
                svc,
                ActivityLevel::Error,
                format!("Scaled to {now} of {want} replicas"),
                Some(problems.join("; ")),
            )
            .await;
    }
    state
        .notify_service(
            svc,
            Event::new(NotifyEvent::ServiceScaled)
                .detail(format!("{from} → {now} replicas"))
                .field("Reason", reason)
                .hint(format!("{} service ps {}", brand::CLI_BIN, svc.name)),
        )
        .await;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(problems.join("; ")))
    }
}

#[allow(clippy::too_many_arguments)]
async fn add_replica(
    state: &AppState,
    docker: &Docker,
    server: &Server,
    svc: &Service,
    env: &Environment,
    live: &Deployment,
    image: &str,
    index: u16,
    routed: bool,
) -> Result<()> {
    let spec = container_spec(state, docker, svc, env, image, &live.id, live.number, index).await?;
    docker::remove_container(docker, &spec.name).await?;
    let id = docker::create_container(docker, &spec).await?;
    let result = async {
        docker::start_container(docker, &id).await?;
        wait_healthy(state, docker, server, svc, env, &id, |_| {}).await?;
        if routed {
            route(docker, &svc.id, &id).await?;
        }
        state
            .db
            .insert_replica(
                &svc.id,
                &ReplicaRow {
                    index,
                    server_id: server.id.clone(),
                    deployment_id: live.id.clone(),
                    container_id: id.clone(),
                },
            )
            .await
    }
    .await;
    if result.is_err() {
        let _ = docker::remove_container(docker, &id).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replica_metrics_keys_round_trip() {
        assert_eq!(metrics_key(3), "#3");
        assert_eq!(index_of_key("#3"), Some(3));
        assert_eq!(index_of_key(""), None);
        assert_eq!(index_of_key("web"), None);
    }

    #[test]
    fn only_single_container_apps_scale() {
        assert!(supports_replicas(&Source::Image {
            image: "nginx".into()
        }));
        assert!(!supports_replicas(&Source::Database {
            kind: DatabaseKind::Postgres,
            version: "18".into()
        }));
        let git = |method| Source::Git {
            repo: "r".into(),
            branch: "main".into(),
            dockerfile: "Dockerfile".into(),
            context: ".".into(),
            root: String::new(),
            watch_paths: true,
            source_id: None,
            auto_deploy: true,
            build: Build {
                method,
                ..Default::default()
            },
        };
        assert!(supports_replicas(&git(BuildMethod::Railpack)));
        assert!(supports_replicas(&git(BuildMethod::Dockerfile)));
        assert!(!supports_replicas(&git(BuildMethod::Compose)));
    }

    #[test]
    fn byte_sizes_read_naturally() {
        assert_eq!(human_bytes(85_000_000), "85 MB");
        assert_eq!(human_bytes(1_234_000_000), "1.2 GB");
    }
}
