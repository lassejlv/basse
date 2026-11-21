//! Replicas: the identical containers of a service behind the proxy's load balancer.
//!
//! Deploys roll replicas over one batch at a time (see [`crate::deploy`]). This module builds
//! their containers, waits for them to be healthy, puts them behind the proxy or drains them,
//! keeps the service's route container in step, and scales a live service without a deploy.

use crate::db::{ReplicaRow, ServiceRecord};
use crate::docker::{self, ContainerSpec, RunState};
use crate::error::{ApiResult, bad_request, conflict};
use crate::notify::Event;
use crate::placement::{self, Slot};
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

/// Whether a deployed service routes through a route container (see [`docker::route_labels`]);
/// compose groups label their own containers.
pub fn has_route(source: &Source) -> bool {
    !matches!(source, Source::Git { build, .. } if build.method == BuildMethod::Compose)
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

/// Stops replicas gracefully while they are still behind the proxy. They get SIGTERM and
/// `STOP_SECS` to finish requests in flight; new requests that reach one meanwhile are refused
/// at once and the proxy retries them on another replica. Leaving the proxy network first
/// would be worse: the requests in flight would lose their connection without a reset and hang
/// until the visitor gave up. Containers that are already gone are skipped.
pub async fn drain(docker: &Docker, containers: &[String]) -> Result<()> {
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

/// Brings the service's route container on `server_id` in line with its domains, port, policy
/// and health path (see [`docker::route_labels`]). Compose groups route themselves.
pub async fn apply_route(
    state: &AppState,
    docker: &Docker,
    svc: &Service,
    server_id: &str,
) -> Result<()> {
    if !has_route(&svc.source) {
        return Ok(());
    }
    let domains = state.db.domains(&svc.id).await?;
    let extras = route_extras(state, svc, server_id).await?;
    let labels = docker::route_labels(
        &domains,
        svc.port,
        svc.scaling.lb_policy,
        svc.health_path.as_deref(),
        &extras,
    );
    docker::ensure_route(docker, &svc.id, labels).await
}

/// Brings the service's route in line on every server that runs one of its replicas.
pub async fn apply_routes(state: &AppState, rec: &ServiceRecord) -> Result<()> {
    let servers: Vec<Id> = if rec.replicas.is_empty() {
        vec![rec.service.server_id.clone()]
    } else {
        rec.replica_servers()
            .into_iter()
            .filter(|s| rec.replicas.iter().any(|r| &r.server_id == s))
            .collect()
    };
    for server_id in servers {
        let (_, docker) = connect(state, &server_id).await?;
        apply_route(state, &docker, &rec.service, &server_id).await?;
    }
    Ok(())
}

/// Removes the service's route from a server that runs none of its replicas any more.
pub async fn remove_route(docker: &Docker, service_id: &str) -> Result<()> {
    docker::ensure_route(docker, service_id, HashMap::new()).await
}

/// What a server's route adds for a service that may run on several servers or sits behind a
/// managed load balancer: the health endpoint load balancers probe and, on every server but
/// the one that obtains certificates, a relay of certificate challenges to that one. Without
/// the relay, Let's Encrypt's challenge lands on whichever server the DNS or load balancer
/// picks and issuance fails on the others.
async fn route_extras(
    state: &AppState,
    svc: &Service,
    server_id: &str,
) -> Result<docker::RouteExtras> {
    let servers = svc.scaling.placement.servers_or(&svc.server_id);
    let lb = state.db.service_lb(&svc.id).await?.is_some();
    if servers.len() < 2 && !lb {
        return Ok(docker::RouteExtras::default());
    }
    let issuer = certificate_server(svc);
    let acme_relay = if issuer == server_id {
        None
    } else {
        match state.db.server(&issuer).await? {
            Some(server) => state.server_ips(&server).await.0.map(|ip| ip.to_string()),
            None => None,
        }
    };
    Ok(docker::RouteExtras {
        health: true,
        acme_relay,
    })
}

/// The server that obtains a multi-server service's certificates: its own server when replicas
/// may run there, else the first server of its placement.
pub fn certificate_server(svc: &Service) -> Id {
    let servers = svc.scaling.placement.servers_or(&svc.server_id);
    if servers.contains(&svc.server_id) {
        svc.server_id.clone()
    } else {
        servers[0].clone()
    }
}

/// Docker connections to the servers a service touches. Servers that can't be reached are left
/// out, with the reason.
pub struct Fleet {
    servers: HashMap<Id, (Server, Docker)>,
    pub unreachable: HashMap<Id, String>,
}

/// How long connecting to one server may take before it counts as unreachable.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

impl Fleet {
    pub async fn connect<'a>(state: &AppState, ids: impl IntoIterator<Item = &'a str>) -> Self {
        let mut unique: Vec<&str> = Vec::new();
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        let results = join_all(unique.into_iter().map(|id| async move {
            let connected = tokio::time::timeout(CONNECT_TIMEOUT, connect(state, id))
                .await
                .unwrap_or_else(|_| Err(anyhow!("timed out connecting")));
            (id.to_string(), connected)
        }))
        .await;
        let mut fleet = Self {
            servers: HashMap::new(),
            unreachable: HashMap::new(),
        };
        for (id, result) in results {
            match result {
                Ok(pair) => {
                    fleet.servers.insert(id, pair);
                }
                Err(e) => {
                    fleet.unreachable.insert(id, format!("{e:#}"));
                }
            }
        }
        fleet
    }

    /// Every reachable server's engine.
    pub fn dockers(&self) -> HashMap<Id, Docker> {
        self.servers
            .iter()
            .map(|(id, (_, d))| (id.clone(), d.clone()))
            .collect()
    }

    pub fn docker(&self, id: &str) -> Option<&Docker> {
        self.servers.get(id).map(|(_, d)| d)
    }

    pub fn server(&self, id: &str) -> Option<&Server> {
        self.servers.get(id).map(|(s, _)| s)
    }

    /// The server and its engine, or why it can't be reached.
    pub fn get(&self, id: &str) -> Result<(&Server, &Docker)> {
        match self.servers.get(id) {
            Some((s, d)) => Ok((s, d)),
            None => Err(anyhow!(
                "cannot reach server {id}: {}",
                self.unreachable
                    .get(id)
                    .map_or("it was deleted", String::as_str)
            )),
        }
    }

    pub fn name(&self, id: &str) -> String {
        self.server(id)
            .map_or_else(|| id.to_string(), |s| s.name.clone())
    }

    /// Makes sure `image` is on `server_id`: an image the daemon built is copied from a server
    /// that has it, any other is pulled.
    pub async fn ensure_image(
        &self,
        server_id: &str,
        image: &str,
        log: impl Fn(String),
    ) -> Result<()> {
        let (server, docker) = self.get(server_id)?;
        if docker::image_exists(docker, image).await? {
            return Ok(());
        }
        if image.starts_with(&format!("{}/", brand::PREFIX)) {
            for (id, (from_server, from)) in &self.servers {
                if id == server_id || !docker::image_exists(from, image).await.unwrap_or(false) {
                    continue;
                }
                log(format!(
                    "Copying image {image} from {} to {}",
                    from_server.name, server.name
                ));
                let bytes = docker::copy_image(from, docker, image).await?;
                log(format!("Copied {} to {}", human_bytes(bytes), server.name));
                return Ok(());
            }
            bail!("image {image} is on none of the service's servers; deploy from source instead");
        }
        log(format!("Pulling {image} on {}", server.name));
        docker::pull_image(docker, image, |_| {}).await
    }
}

/// Each replica of a service with its container, health and newest usage.
pub async fn describe(state: &AppState, rec: &ServiceRecord) -> Vec<Replica> {
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
    let fleet = Fleet::connect(state, rec.replicas.iter().map(|r| r.server_id.as_str())).await;
    let inspected = join_all(rec.replicas.iter().map(|r| {
        let fleet = &fleet;
        async move {
            let docker = fleet.docker(&r.server_id)?;
            docker::inspect(docker, &r.container_id)
                .await
                .ok()
                .flatten()
        }
    }))
    .await;
    rec.replicas
        .iter()
        .zip(inspected)
        .map(|(r, inspected)| {
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

/// Room for more replicas of a service on each server it may run on, by memory. Fails only
/// when no server could be measured.
pub async fn capacity(state: &AppState, rec: &ServiceRecord) -> Result<ScaleCapacity> {
    let svc = &rec.service;
    let since = Utc::now() - chrono::TimeDelta::seconds(FRESH_SECS);
    let per_replica = state
        .db
        .latest_samples(&svc.id, since)
        .await?
        .into_iter()
        .map(|(_, _, mem)| mem)
        .max()
        .unwrap_or(0)
        .max(MIN_REPLICA_MEMORY);
    let allowed = svc.scaling.placement.servers_or(&svc.server_id);
    let servers: Vec<ServerCapacity> = join_all(
        allowed
            .iter()
            .map(|id| server_capacity(state, rec, id, per_replica)),
    )
    .await;
    if let Some(problem) = servers
        .iter()
        .all(|s| s.problem.is_some())
        .then(|| servers[0].problem.clone())
        .flatten()
    {
        bail!("{problem}");
    }
    let measured = || servers.iter().filter(|s| s.problem.is_none());
    Ok(ScaleCapacity {
        server_id: svc.server_id.clone(),
        mem_total: measured().map(|s| s.mem_total).sum(),
        mem_free: measured().map(|s| s.mem_free).sum(),
        per_replica,
        fits_more: measured()
            .map(|s| s.fits_more)
            .fold(0u32, u32::saturating_add),
        servers,
    })
}

async fn server_capacity(
    state: &AppState,
    rec: &ServiceRecord,
    server_id: &str,
    per_replica: u64,
) -> ServerCapacity {
    let replicas = rec
        .replicas
        .iter()
        .filter(|r| r.server_id == server_id)
        .count();
    let measured = async {
        let (server, docker) = tokio::time::timeout(CONNECT_TIMEOUT, connect(state, server_id))
            .await
            .unwrap_or_else(|_| Err(anyhow!("timed out connecting")))?;
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
        anyhow::Ok((total, free.saturating_sub(reserve)))
    };
    let (mem_total, mem_free, problem) = match measured.await {
        Ok((total, free)) => (total, free, None),
        Err(e) => (0, 0, Some(format!("{e:#}"))),
    };
    ServerCapacity {
        server_id: server_id.to_string(),
        mem_total,
        mem_free,
        fits_more: u32::try_from(mem_free / per_replica).unwrap_or(u32::MAX),
        replicas: u16::try_from(replicas).unwrap_or(u16::MAX),
        problem,
    }
}

/// The servers a service may place replicas on, each with the most replicas it can hold.
/// Servers that can't be reached are left out; when memory can't be read at all, every server
/// of the placement counts as roomy, so placement never blocks on a measurement.
pub async fn slots(state: &AppState, rec: &ServiceRecord) -> Vec<Slot> {
    let svc = &rec.service;
    let unbounded = || {
        svc.scaling
            .placement
            .servers_or(&svc.server_id)
            .into_iter()
            .map(|server_id| Slot {
                server_id,
                room: u16::MAX,
            })
            .collect()
    };
    match capacity(state, rec).await {
        Ok(cap) => cap
            .servers
            .iter()
            .filter(|s| s.problem.is_none())
            .map(|s| Slot {
                server_id: s.server_id.clone(),
                room: s
                    .replicas
                    .saturating_add(u16::try_from(s.fits_more).unwrap_or(u16::MAX)),
            })
            .collect(),
        Err(e) => {
            tracing::warn!("cannot check memory for {}: {e:#}", svc.name);
            unbounded()
        }
    }
}

/// Where each replica index goes for a service running `n` replicas, keeping the current ones
/// in place where the placement allows.
pub async fn plan(state: &AppState, rec: &ServiceRecord, n: u16) -> Vec<Id> {
    let current: Vec<(u16, Id)> = rec
        .replicas
        .iter()
        .map(|r| (r.index, r.server_id.clone()))
        .collect();
    let slots = slots(state, rec).await;
    placement::plan(n, &slots, rec.service.scaling.placement.strategy, &current)
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
/// a shared writable volume without the opt-in, or more than its servers' memory fits.
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
        check_volumes(state, svc, replicas).await?;
    }
    let current = rec.replicas.len().max(1);
    let more = usize::from(replicas).saturating_sub(current);
    if more > 0 && rec.service.live_deployment_id.is_some() {
        match capacity(state, rec).await {
            Ok(cap) if more > cap.fits_more as usize => {
                let place = if cap.servers.len() > 1 {
                    format!("memory on its {} servers", cap.servers.len())
                } else {
                    "server memory".into()
                };
                conflict!(
                    "{place} fits {} more replica{} of '{}' (~{} each, {} free), not {more}",
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

/// Several replicas on a writable volume means several processes writing the same files,
/// which most apps don't expect, so it is refused unless the service opted in. Replicas on
/// different servers can't share a volume at all; they need `volumes_per_server`, which gives
/// each server its own copy.
pub async fn check_volumes(state: &AppState, svc: &Service, replicas: u16) -> ApiResult<()> {
    if replicas <= 1 {
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
    let plural = if names.len() == 1 { "" } else { "s" };
    let placement = &svc.scaling.placement;
    if placement.is_multi_server() && !placement.volumes_per_server {
        conflict!(
            "'{}' mounts the volume{plural} {}, which replicas on different servers can't share; set volumes_per_server to give each server its own copy, or place it on one server",
            svc.name,
            names.join(", ")
        );
    }
    let servers = if placement.volumes_per_server && placement.strategy == PlacementStrategy::Spread
    {
        u16::try_from(placement.servers.len().max(1)).unwrap_or(u16::MAX)
    } else {
        1
    };
    if replicas.div_ceil(servers) > 1 && !svc.scaling.shared_volume_ok {
        conflict!(
            "'{}' mounts the volume{plural} {}, which every replica on a server would write at once; set shared_volume_ok to allow it",
            svc.name,
            names.join(", ")
        );
    }
    Ok(())
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

/// Adds or drains replicas until the service runs its replica count where its placement says,
/// in the background.
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
        state.lb_kick();
    });
}

/// Brings the live replica set to the service's replica count and placement under the service's
/// deploy lock. New replicas start and pass their health check before any old one drains, so a
/// replica moving to another server serves on both for a moment instead of on neither. A
/// queued deploy rolls out the new count itself, so this leaves busy services alone.
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
    let target = plan(state, &rec, want).await;
    if target.is_empty() {
        bail!("none of the servers of '{}' can be reached", svc.name);
    }
    let adds: Vec<(u16, Id)> = target
        .iter()
        .enumerate()
        .map(|(i, s)| (u16::try_from(i).unwrap_or(u16::MAX), s.clone()))
        .filter(|(i, s)| {
            !rec.replicas
                .iter()
                .any(|r| r.index == *i && &r.server_id == s)
        })
        .collect();
    let surplus: Vec<&ReplicaRow> = rec.replicas.iter().filter(|r| r.index >= want).collect();
    if adds.is_empty() && surplus.is_empty() {
        return Ok(());
    }
    let fleet = Fleet::connect(
        state,
        std::iter::once(svc.server_id.as_str())
            .chain(target.iter().map(String::as_str))
            .chain(rec.replicas.iter().map(|r| r.server_id.as_str())),
    )
    .await;
    let started = Instant::now();
    let mut problems = Vec::new();
    let mut leaving: Vec<(Id, String)> = Vec::new();
    for r in &surplus {
        state.db.delete_replica(&svc.id, r.index).await?;
        leaving.push((r.server_id.clone(), r.container_id.clone()));
    }
    if !adds.is_empty() {
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
        let routed =
            !docker::upstream_labels(&state.db.domains(&svc.id).await?, svc.port).is_empty();
        let mut ready: Vec<&str> = Vec::new();
        for (_, server_id) in &adds {
            if ready.contains(&server_id.as_str()) {
                continue;
            }
            let prepared = async {
                fleet
                    .ensure_image(server_id, &image, |line| tracing::info!("{line}"))
                    .await?;
                let (_, docker) = fleet.get(server_id)?;
                apply_route(state, docker, svc, server_id).await
            };
            match prepared.await {
                Ok(()) => ready.push(server_id),
                Err(e) => problems.push(format!("{}: {e:#}", fleet.name(server_id))),
            }
        }
        let added = join_all(
            adds.iter()
                .filter(|(_, s)| ready.contains(&s.as_str()))
                .map(|(index, server_id)| {
                    let (fleet, env, live, image) = (&fleet, &env, &live, &image);
                    async move {
                        let (server, docker) =
                            fleet.get(server_id).map_err(|e| format!("{e:#}"))?;
                        add_replica(state, docker, server, svc, env, live, image, *index, routed)
                            .await
                            .map(|()| *index)
                            .map_err(|e| format!("replica #{index} on {}: {e:#}", server.name))
                    }
                }),
        )
        .await;
        for result in added {
            match result {
                Ok(index) => {
                    if let Some(old) = rec
                        .replicas
                        .iter()
                        .find(|r| r.index == index && r.index < want)
                    {
                        leaving.push((old.server_id.clone(), old.container_id.clone()));
                    }
                }
                Err(e) => problems.push(e),
            }
        }
    }
    for (server_id, container) in &leaving {
        let Some(docker) = fleet.docker(server_id) else {
            problems.push(format!(
                "cannot drain a replica on {}: unreachable",
                fleet.name(server_id)
            ));
            continue;
        };
        drain(docker, std::slice::from_ref(container)).await?;
        if let Err(e) = docker::remove_container(docker, container).await {
            problems.push(format!("{e:#}"));
        }
    }
    let after = state.db.service(service_id).await?;
    let now = after.as_ref().map_or(0, |r| r.replicas.len());
    if let Some(after) = &after {
        for server_id in rec.replica_servers() {
            let still = after.replicas.iter().any(|r| r.server_id == server_id);
            if !still
                && let Some(docker) = fleet.docker(&server_id)
                && let Err(e) = remove_route(docker, &svc.id).await
            {
                problems.push(format!("{e:#}"));
            }
        }
    }
    let from = rec.replicas.len();
    let servers = after.as_ref().map_or(0, |r| {
        let mut s: Vec<&str> = r.replicas.iter().map(|r| r.server_id.as_str()).collect();
        s.sort_unstable();
        s.dedup();
        s.len()
    });
    let on = if servers > 1 {
        format!(" on {servers} servers")
    } else {
        String::new()
    };
    let detail = format!(
        "{from} → {now}{on} · {reason} · {}s",
        started.elapsed().as_secs()
    );
    if problems.is_empty() {
        state
            .service_activity(
                svc,
                ActivityLevel::Success,
                format!(
                    "Scaled to {now} replica{}{on}",
                    if now == 1 { "" } else { "s" }
                ),
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
                .detail(format!("{from} → {now} replicas{on}"))
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
            tokio::time::sleep(PROXY_SETTLE).await;
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
