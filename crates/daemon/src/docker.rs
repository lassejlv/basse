//! Docker access: a client per server plus container, network, image and proxy helpers.

use crate::proxy_pages;
use crate::secrets::Secrets;
use crate::ssh::{SshPaths, Tunnel};
use anyhow::{Context as _, Result, bail};
use bollard::container::LogOutput;
use bollard::errors::Error as DockerError;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, ContainerSummary, ContainerSummaryStateEnum,
    EndpointSettings, HostConfig, NetworkConnectRequest, NetworkCreateRequest, NetworkingConfig,
    PortBinding, RestartPolicy as DockerRestart, RestartPolicyNameEnum, VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, DownloadFromContainerOptions, ImportImageOptions,
    ListContainersOptions, ListNetworksOptions, ListVolumesOptions, RemoveContainerOptions,
    RemoveVolumeOptions, StopContainerOptions, UploadToContainerOptions,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use common::{
    ContainerInfo, Domain, Id, LbPolicy, LogLine, LogStream, PreflightCheck, ProxyStatus,
    RestartPolicy, Server, ServerCleanup, ServerKind, brand,
};
use futures_util::StreamExt as _;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, RwLock};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};

/// Reverse proxy image run on every server.
pub const PROXY_IMAGE: &str = "lucaslorentz/caddy-docker-proxy:2.12-alpine";
/// Small image for helper containers that copy files into or between volumes.
pub const HELPER_IMAGE: &str = "busybox:1.37";
const DOCKER_TIMEOUT_SECS: u64 = 120;

/// One Docker client per server, created on first use. Remote servers are reached through an
/// SSH tunnel to their Docker socket.
pub struct DockerPool {
    clients: RwLock<HashMap<Id, Docker>>,
    tunnels: Mutex<HashMap<Id, Tunnel>>,
    /// The preflight checklist from when each tunnel opened.
    checks: Mutex<HashMap<Id, Vec<PreflightCheck>>>,
    connecting: tokio::sync::Mutex<()>,
    ssh: SshPaths,
    secrets: Secrets,
}

impl DockerPool {
    pub fn new(ssh: SshPaths, secrets: Secrets) -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
            checks: Mutex::new(HashMap::new()),
            connecting: tokio::sync::Mutex::new(()),
            ssh,
            secrets,
        }
    }

    pub async fn get(&self, server: &Server) -> Result<Docker> {
        if let Some(docker) = self.cached(server) {
            return Ok(docker);
        }
        let docker = match server.kind {
            ServerKind::Local => Docker::connect_with_defaults()
                .context("cannot connect to the local Docker socket")?,
            ServerKind::Ssh => {
                let _connecting = self.connecting.lock().await;
                if let Some(docker) = self.cached(server) {
                    return Ok(docker);
                }
                let (private_key, key_name) = match &server.ssh_key_secret_id {
                    Some(id) => (
                        Some(self.secrets.ssh_private_key(id).await?),
                        self.secrets.name(id).await,
                    ),
                    None => (None, None),
                };
                let (tunnel, checks) = Tunnel::connect(
                    &self.ssh,
                    server,
                    private_key.as_ref().map(|k| k.as_str()),
                    key_name.as_deref(),
                )
                .await?;
                lock(&self.checks).insert(server.id.clone(), checks);
                let docker = Docker::connect_with_unix(
                    &tunnel.socket.to_string_lossy(),
                    DOCKER_TIMEOUT_SECS,
                    API_DEFAULT_VERSION,
                )?;
                lock(&self.tunnels).insert(server.id.clone(), tunnel);
                docker
            }
        };
        self.clients
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(server.id.clone(), docker.clone());
        Ok(docker)
    }

    /// The cached client, if its SSH tunnel (for remote servers) is still up.
    fn cached(&self, server: &Server) -> Option<Docker> {
        let docker = self
            .clients
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&server.id)
            .cloned()?;
        if server.kind == ServerKind::Ssh
            && !lock(&self.tunnels)
                .get_mut(&server.id)
                .is_some_and(Tunnel::is_alive)
        {
            self.forget(&server.id);
            return None;
        }
        Some(docker)
    }

    /// Environment that points the `docker` CLI at the same engine as [`Self::get`]: the SSH
    /// tunnel's socket for remote servers, reopening the tunnel when a failed check closed it,
    /// and the default socket for the local one. Without a tunnel the CLI would silently use the
    /// local engine, so that is an error.
    pub async fn cli_env(&self, server: &Server) -> Result<Vec<(String, String)>> {
        if server.kind == ServerKind::Local {
            return Ok(Vec::new());
        }
        self.get(server).await?;
        let socket = lock(&self.tunnels)
            .get(&server.id)
            .map(|t| t.socket.clone())
            .with_context(|| format!("the SSH tunnel to server {} closed", server.name))?;
        Ok(vec![(
            "DOCKER_HOST".to_string(),
            format!("unix://{}", socket.display()),
        )])
    }

    /// Drops the cached client and closes its tunnel, e.g. after the server was deleted or a
    /// check failed.
    pub fn forget(&self, server_id: &str) {
        self.clients
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(server_id);
        lock(&self.tunnels).remove(server_id);
    }

    /// Runs a POSIX sh script on a remote server over SSH with its key.
    pub async fn run_script(
        &self,
        server: &Server,
        script: &str,
        timeout: std::time::Duration,
    ) -> Result<String> {
        let private_key = match &server.ssh_key_secret_id {
            Some(id) => Some(self.secrets.ssh_private_key(id).await?),
            None => None,
        };
        crate::ssh::run(
            &self.ssh,
            server,
            private_key.as_ref().map(|k| k.as_str()),
            script,
            timeout,
        )
        .await
    }

    /// The preflight checklist from when the server's tunnel opened; empty for the local server.
    pub fn checks(&self, server_id: &str) -> Vec<PreflightCheck> {
        lock(&self.checks)
            .get(server_id)
            .cloned()
            .unwrap_or_default()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// Names

/// Name of one replica's container: the deployment number it runs and its replica index.
pub fn container_name(service_name: &str, env_id: &str, number: i64, replica: u16) -> String {
    let short: String = env_id.chars().take(6).collect();
    format!(
        "{}-{service_name}-{short}-{number}-r{replica}",
        brand::PREFIX
    )
}

/// The container holding a service's proxy route; see [`route_labels`].
pub fn route_container(service_id: &str) -> String {
    format!("{}-route-{service_id}", brand::PREFIX)
}

pub fn env_network(env_id: &str) -> String {
    format!("{}-env-{env_id}", brand::PREFIX)
}

pub fn volume_name(service_id: &str, volume: &str) -> String {
    format!("{}-{service_id}-{volume}", brand::PREFIX)
}

pub fn build_tag(service_id: &str, number: i64) -> String {
    format!("{}/{service_id}:{number}", brand::PREFIX)
}

pub fn proxy_volume() -> String {
    format!("{}-caddy-data", brand::PREFIX)
}

/// Adds `:latest` when an image reference has neither tag nor digest.
pub fn normalize_image(image: &str) -> String {
    let image = image.trim();
    let last_segment = image.rsplit('/').next().unwrap_or(image);
    if image.contains('@') || last_segment.contains(':') {
        image.to_string()
    } else {
        format!("{image}:latest")
    }
}

/// DNS name of a service on the proxy network. The proxy resolves it on every new upstream
/// connection, so traffic follows whichever containers currently hold the alias.
pub fn proxy_alias(service_id: &str) -> String {
    format!("{}-{service_id}", brand::PREFIX)
}

/// The domains that route somewhere, with the container port they route to. Domains without a
/// port (none on the domain and none on the service) are skipped.
fn routable(domains: &[Domain], service_port: Option<u16>) -> impl Iterator<Item = (String, u16)> {
    domains.iter().filter_map(move |d| {
        let port = d.port.or(service_port)?;
        let site = if d.tls {
            d.host.clone()
        } else {
            format!("http://{}", d.host)
        };
        Some((site, port))
    })
}

/// Labels a replica carries so the proxy adds its address on the proxy network to the
/// upstreams of each of the service's domains. The proxy merges the labels of every container
/// that names the same site, so replicas only differ in their address. The error pages are
/// imported here as well as on the route, with the same value, so they hold while the route
/// container is being replaced.
pub fn upstream_labels(domains: &[Domain], service_port: Option<u16>) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    for (i, (site, port)) in routable(domains, service_port).enumerate() {
        labels.insert(format!("caddy_{i}"), site);
        labels.insert(
            format!("caddy_{i}.reverse_proxy"),
            format!("{{{{upstreams {port}}}}}"),
        );
        labels.insert(format!("caddy_{i}.import"), proxy_pages::snippet());
    }
    labels
}

/// How the proxy balances a service's domains over its replicas, carried by the service's route
/// container ([`route_container`]) and nowhere else: when two containers give one site different
/// options, caddy-docker-proxy drops the whole configuration and every site on the server goes
/// down. Keeping the options on one container lets them change without touching the replicas,
/// and with no replica left the route alone answers 503, which the imported error handling
/// turns into the "No server available" page.
///
/// Health checks and retries: an active probe of `health_path` when the service has one, a
/// replica that fails a request is skipped for 30 seconds, and a request whose replica cannot
/// be reached is retried at once on up to three others. Retries are counted, not timed: with
/// `lb_try_duration` Caddy waits between tries and gives up when the configuration reloads,
/// which is exactly what happens when a replica dies. A killed replica's address stops
/// answering instead of refusing, so dials give up after a second.
pub fn route_labels(
    domains: &[Domain],
    service_port: Option<u16>,
    policy: LbPolicy,
    health_path: Option<&str>,
) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    for (i, (site, _)) in routable(domains, service_port).enumerate() {
        let rp = format!("caddy_{i}.reverse_proxy");
        labels.insert(format!("caddy_{i}"), site);
        labels.insert(format!("{rp}.lb_policy"), policy.as_str().into());
        labels.insert(format!("{rp}.lb_retries"), "3".into());
        labels.insert(format!("{rp}.transport"), "http".into());
        labels.insert(format!("{rp}.transport.dial_timeout"), "1s".into());
        labels.insert(format!("{rp}.fail_duration"), "30s".into());
        labels.insert(format!("{rp}.header_up"), REAL_IP_HEADER.into());
        labels.insert(format!("caddy_{i}.import"), proxy_pages::snippet());
        if let Some(path) = health_path {
            let path = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            };
            labels.insert(format!("{rp}.health_uri"), path);
            labels.insert(format!("{rp}.health_interval"), "5s".into());
            labels.insert(format!("{rp}.health_timeout"), "3s".into());
        }
    }
    labels
}

/// Creates, replaces or removes a service's route container so it carries `labels`. The old
/// one is removed before the new one starts, never alongside it (see [`route_labels`]); in
/// between the replicas' own labels keep routing with the proxy's defaults.
pub async fn ensure_route(
    docker: &Docker,
    service_id: &str,
    labels: HashMap<String, String>,
) -> Result<()> {
    let name = route_container(service_id);
    let current = inspect(docker, &name).await?;
    if labels.is_empty() {
        if current.is_some() {
            remove_container(docker, &name).await?;
        }
        return Ok(());
    }
    if let Some(c) = &current {
        let carried: HashMap<String, String> = c
            .config
            .as_ref()
            .and_then(|c| c.labels.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|(k, _)| k.starts_with("caddy"))
            .collect();
        if carried == labels && RunState::from_inspect(c) == RunState::Running {
            return Ok(());
        }
        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };
        if let Err(e) = docker.remove_container(&name, Some(options)).await
            && !is_not_found(&e)
        {
            return Err(e).context("cannot replace the route container");
        }
    }
    if !image_exists(docker, HELPER_IMAGE).await? {
        pull_image(docker, HELPER_IMAGE, |_| {}).await?;
    }
    let mut labels = labels;
    labels.insert(brand::label("managed"), "true".into());
    labels.insert(brand::label("route"), service_id.into());
    let spec = ContainerSpec {
        name,
        image: HELPER_IMAGE.into(),
        cmd: Some(vec!["sleep".into(), "infinity".into()]),
        labels,
        restart: RestartPolicy::UnlessStopped,
        ..Default::default()
    };
    let id = create_container(docker, &spec).await?;
    start_container(docker, &id).await
}

/// Hands upstreams the visitor's address as `X-Real-IP`: the connecting address, or the one a
/// trusted proxy such as Cloudflare forwards.
const REAL_IP_HEADER: &str = "X-Real-IP {client_ip}";

// Containers

/// Everything needed to create one container.
#[derive(Debug, Clone, Default)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub env: Vec<String>,
    pub cmd: Option<Vec<String>>,
    pub labels: HashMap<String, String>,
    /// `source:target[:ro]` volume or bind mounts.
    pub binds: Vec<String>,
    pub restart: RestartPolicy,
    /// Networks to join with their DNS aliases. The first is the primary network.
    pub networks: Vec<(String, Vec<String>)>,
    /// Published ports as `(\"443/udp\", 443)`.
    pub published: Vec<(String, u16)>,
    /// Host address the published ports bind to; every interface when unset.
    pub publish_ip: Option<String>,
    /// `/etc/hosts` entries as `name:address`; `host-gateway` is the Docker host.
    pub extra_hosts: Vec<String>,
}

pub async fn create_container(docker: &Docker, spec: &ContainerSpec) -> Result<String> {
    let restart_name = match spec.restart {
        RestartPolicy::No => RestartPolicyNameEnum::NO,
        RestartPolicy::Always => RestartPolicyNameEnum::ALWAYS,
        RestartPolicy::UnlessStopped => RestartPolicyNameEnum::UNLESS_STOPPED,
        RestartPolicy::OnFailure => RestartPolicyNameEnum::ON_FAILURE,
    };
    let port_bindings: HashMap<String, Option<Vec<PortBinding>>> = spec
        .published
        .iter()
        .map(|(port, host)| {
            let binding = PortBinding {
                host_ip: spec.publish_ip.clone(),
                host_port: Some(host.to_string()),
            };
            (port.clone(), Some(vec![binding]))
        })
        .collect();
    let endpoints: HashMap<String, EndpointSettings> = spec
        .networks
        .iter()
        .map(|(net, aliases)| {
            let settings = EndpointSettings {
                aliases: (!aliases.is_empty()).then(|| aliases.clone()),
                ..Default::default()
            };
            (net.clone(), settings)
        })
        .collect();
    let body = ContainerCreateBody {
        image: Some(spec.image.clone()),
        env: Some(spec.env.clone()),
        cmd: spec.cmd.clone(),
        labels: Some(spec.labels.clone()),
        exposed_ports: (!spec.published.is_empty())
            .then(|| spec.published.iter().map(|(p, _)| p.clone()).collect()),
        host_config: Some(HostConfig {
            binds: Some(spec.binds.clone()),
            restart_policy: Some(DockerRestart {
                name: Some(restart_name),
                maximum_retry_count: None,
            }),
            port_bindings: (!port_bindings.is_empty()).then_some(port_bindings),
            network_mode: spec.networks.first().map(|(n, _)| n.clone()),
            extra_hosts: (!spec.extra_hosts.is_empty()).then(|| spec.extra_hosts.clone()),
            ..Default::default()
        }),
        networking_config: Some(NetworkingConfig {
            endpoints_config: Some(endpoints),
        }),
        ..Default::default()
    };
    let options = CreateContainerOptions {
        name: Some(spec.name.clone()),
        ..Default::default()
    };
    let created = docker
        .create_container(Some(options), body)
        .await
        .with_context(|| format!("cannot create container {}", spec.name))?;
    Ok(created.id)
}

pub async fn start_container(docker: &Docker, id: &str) -> Result<()> {
    docker
        .start_container(id, None)
        .await
        .with_context(|| format!("cannot start container {}", short_id(id)))
}

/// Stops gracefully, then removes. A container that is already gone is not an error.
pub async fn remove_container(docker: &Docker, id: &str) -> Result<()> {
    let stop = StopContainerOptions {
        t: Some(10),
        ..Default::default()
    };
    match docker.stop_container(id, Some(stop)).await {
        Ok(()) => {}
        Err(e) if is_not_found(&e) => return Ok(()),
        Err(e) if is_not_modified(&e) => {}
        Err(e) => tracing::warn!(
            container = short_id(id),
            "stop failed, forcing removal: {e}"
        ),
    }
    let remove = RemoveContainerOptions {
        force: true,
        ..Default::default()
    };
    match docker.remove_container(id, Some(remove)).await {
        Ok(()) => Ok(()),
        Err(e) if is_not_found(&e) => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot remove container {}", short_id(id))),
    }
}

/// Every container the daemon manages on this server, running or not.
pub async fn list_managed(docker: &Docker) -> Result<Vec<ContainerSummary>> {
    list_by_label(docker, &format!("{}=true", brand::label("managed"))).await
}

pub async fn list_service_containers(
    docker: &Docker,
    service_id: &str,
) -> Result<Vec<ContainerSummary>> {
    list_by_label(docker, &format!("{}={service_id}", brand::label("service"))).await
}

pub async fn list_by_label(docker: &Docker, label: &str) -> Result<Vec<ContainerSummary>> {
    let filters = HashMap::from([("label".to_string(), vec![label.to_string()])]);
    let options = ListContainersOptions {
        all: true,
        filters: Some(filters),
        ..Default::default()
    };
    docker
        .list_containers(Some(options))
        .await
        .context("cannot list containers")
}

/// Removes every container of a service on one server but `keep`. Returns how many were
/// removed.
pub async fn remove_service_containers(
    docker: &Docker,
    service_id: &str,
    keep: &[String],
) -> Result<usize> {
    let mut removed = 0;
    for c in list_service_containers(docker, service_id).await? {
        let Some(id) = c.id else { continue };
        if keep.contains(&id) {
            continue;
        }
        remove_container(docker, &id).await?;
        removed += 1;
    }
    Ok(removed)
}

/// Puts a container behind the proxy by joining the proxy network under the service alias.
pub async fn connect_to_proxy(docker: &Docker, id: &str, alias: &str) -> Result<()> {
    ensure_network(docker, brand::PROXY_NETWORK).await?;
    let request = NetworkConnectRequest {
        container: id.to_string(),
        endpoint_config: Some(EndpointSettings {
            aliases: Some(vec![alias.to_string()]),
            ..Default::default()
        }),
    };
    docker
        .connect_network(brand::PROXY_NETWORK, request)
        .await
        .with_context(|| format!("cannot attach {} to the proxy network", short_id(id)))
}

pub async fn inspect(docker: &Docker, id: &str) -> Result<Option<ContainerInspectResponse>> {
    match docker.inspect_container(id, None).await {
        Ok(c) => Ok(Some(c)),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(e).context("cannot inspect container"),
    }
}

/// Coarse run state of a container, from a list summary or an inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    Restarting,
    Exited {
        code: Option<i64>,
    },
    /// Created, paused, removing or dead.
    Other,
}

impl RunState {
    pub fn from_summary(c: &ContainerSummary) -> Self {
        match c.state {
            Some(ContainerSummaryStateEnum::RUNNING) => Self::Running,
            Some(ContainerSummaryStateEnum::RESTARTING) => Self::Restarting,
            Some(ContainerSummaryStateEnum::EXITED) => Self::Exited {
                code: c.status.as_deref().and_then(parse_exit_code),
            },
            _ => Self::Other,
        }
    }

    pub fn from_inspect(c: &ContainerInspectResponse) -> Self {
        let Some(state) = &c.state else {
            return Self::Other;
        };
        if state.restarting == Some(true) {
            Self::Restarting
        } else if state.running == Some(true) {
            Self::Running
        } else if state.status == Some(bollard::models::ContainerStateStatusEnum::EXITED) {
            Self::Exited {
                code: state.exit_code,
            }
        } else {
            Self::Other
        }
    }
}

/// Parses the exit code out of a status such as `Exited (137) 2 minutes ago`.
fn parse_exit_code(status: &str) -> Option<i64> {
    let rest = status.strip_prefix("Exited (")?;
    rest.split(')').next()?.parse().ok()
}

pub fn container_info(c: &ContainerInspectResponse) -> ContainerInfo {
    let state = c.state.as_ref();
    let running = state.and_then(|s| s.running).unwrap_or(false);
    let mut ports: Vec<String> = c
        .config
        .as_ref()
        .and_then(|cfg| cfg.exposed_ports.clone())
        .unwrap_or_default();
    ports.sort();
    let mut networks: Vec<String> = c
        .network_settings
        .as_ref()
        .and_then(|n| n.networks.as_ref())
        .map(|n| n.keys().cloned().collect())
        .unwrap_or_default();
    networks.sort();
    ContainerInfo {
        id: short_id(c.id.as_deref().unwrap_or_default()).to_string(),
        name: c
            .name
            .as_deref()
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_string(),
        image: c
            .config
            .as_ref()
            .and_then(|cfg| cfg.image.clone())
            .unwrap_or_default(),
        state: state
            .and_then(|s| s.status)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".into()),
        started_at: state
            .and_then(|s| s.started_at.as_deref())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&chrono::Utc))
            .filter(|_| running),
        restart_count: c.restart_count.unwrap_or(0),
        exit_code: if running {
            None
        } else {
            state.and_then(|s| s.exit_code)
        },
        ports,
        networks,
    }
}

/// IP address of the container on `network`, or on any network when that one is missing.
pub fn container_ip(c: &ContainerInspectResponse, network: &str) -> Option<String> {
    let networks = c.network_settings.as_ref()?.networks.as_ref()?;
    networks
        .get(network)
        .into_iter()
        .chain(networks.values())
        .filter_map(|n| n.ip_address.clone())
        .find(|ip| !ip.is_empty())
}

/// Runs a command in a container and returns its exit code.
pub async fn exec(docker: &Docker, container: &str, cmd: Vec<String>) -> Result<i64> {
    let created = docker
        .create_exec(
            container,
            CreateExecOptions::<String> {
                cmd: Some(cmd),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;
    if let StartExecResults::Attached { mut output, .. } =
        docker.start_exec(&created.id, None).await?
    {
        while output.next().await.is_some() {}
    }
    let inspected = docker.inspect_exec(&created.id).await?;
    Ok(inspected.exit_code.unwrap_or(-1))
}

/// How an exec ended: its exit code and the tail of what it wrote to stderr.
#[derive(Debug)]
pub struct ExecOutcome {
    pub exit_code: i64,
    pub stderr: String,
}

impl ExecOutcome {
    /// Fails with the command's stderr unless it exited with 0.
    pub fn check(self, what: &str) -> Result<()> {
        if self.exit_code == 0 {
            return Ok(());
        }
        let stderr = self.stderr.trim();
        if stderr.is_empty() {
            bail!("{what} exited with code {}", self.exit_code);
        }
        bail!("{what} exited with code {}: {stderr}", self.exit_code)
    }
}

/// Keeps the last `STDERR_KEEP` bytes of stderr for error messages.
const STDERR_KEEP: usize = 4096;

/// Runs a command in a container, feeding `stdin` (then closing it) and streaming stdout into
/// `stdout`. `env` entries are `KEY=value` and override the container's environment.
pub async fn exec_stream(
    docker: &Docker,
    container: &str,
    cmd: Vec<String>,
    env: Vec<String>,
    stdin: Option<&mut (dyn AsyncRead + Unpin + Send)>,
    stdout: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<ExecOutcome> {
    let created = docker
        .create_exec(
            container,
            CreateExecOptions::<String> {
                cmd: Some(cmd),
                env: Some(env),
                attach_stdin: Some(stdin.is_some()),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await
        .context("cannot create exec")?;
    let StartExecResults::Attached {
        mut output,
        mut input,
    } = docker
        .start_exec(&created.id, None)
        .await
        .context("cannot start exec")?
    else {
        bail!("exec started detached");
    };
    let feed = async {
        if let Some(stdin) = stdin {
            tokio::io::copy(stdin, &mut input)
                .await
                .context("cannot write to the command's stdin")?;
        }
        input.shutdown().await.context("cannot close stdin")
    };
    let drain = async {
        let mut stderr: Vec<u8> = Vec::new();
        while let Some(frame) = output.next().await {
            match frame.context("exec output failed")? {
                LogOutput::StdOut { message } | LogOutput::Console { message } => {
                    stdout
                        .write_all(&message)
                        .await
                        .context("cannot write the command's output")?;
                }
                LogOutput::StdErr { message } => {
                    stderr.extend_from_slice(&message);
                    let excess = stderr.len().saturating_sub(STDERR_KEEP);
                    stderr.drain(..excess);
                }
                LogOutput::StdIn { .. } => {}
            }
        }
        stdout
            .flush()
            .await
            .context("cannot flush the command's output")?;
        anyhow::Ok(String::from_utf8_lossy(&stderr).into_owned())
    };
    let (fed, stderr) = tokio::join!(feed, drain);
    let stderr = stderr?;
    let inspected = docker.inspect_exec(&created.id).await?;
    let exit_code = inspected.exit_code.unwrap_or(-1);
    if exit_code == 0 {
        fed?;
    }
    Ok(ExecOutcome { exit_code, stderr })
}

/// Starts a throwaway container from `image` with `binds` mounted and nothing running but
/// `sleep`, so commands can be exec'd against a volume while its service is stopped.
pub async fn start_helper(
    docker: &Docker,
    name: &str,
    image: &str,
    binds: Vec<String>,
    network_mode: Option<&str>,
) -> Result<String> {
    remove_container(docker, name).await?;
    let body = ContainerCreateBody {
        image: Some(image.to_string()),
        entrypoint: Some(vec!["sleep".into()]),
        cmd: Some(vec!["3600".into()]),
        labels: Some(HashMap::from([(
            brand::label("managed"),
            "true".to_string(),
        )])),
        host_config: Some(HostConfig {
            binds: Some(binds),
            network_mode: network_mode.map(str::to_string),
            ..Default::default()
        }),
        ..Default::default()
    };
    let options = CreateContainerOptions {
        name: Some(name.to_string()),
        ..Default::default()
    };
    let created = docker
        .create_container(Some(options), body)
        .await
        .with_context(|| format!("cannot create helper container {name}"))?;
    start_container(docker, &created.id).await?;
    Ok(created.id)
}

/// Stops a container, giving it `timeout_secs` to shut down. Already stopped is fine.
pub async fn stop_container(docker: &Docker, id: &str, timeout_secs: i32) -> Result<()> {
    let options = StopContainerOptions {
        t: Some(timeout_secs),
        ..Default::default()
    };
    match docker.stop_container(id, Some(options)).await {
        Ok(()) => Ok(()),
        Err(e) if is_not_modified(&e) => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot stop container {}", short_id(id))),
    }
}

/// Tail of a container's output, used to explain a failed health check.
pub async fn tail_logs(docker: &Docker, container: &str, lines: usize) -> Vec<LogLine> {
    let options = bollard::query_parameters::LogsOptions {
        stdout: true,
        stderr: true,
        timestamps: true,
        tail: lines.to_string(),
        ..Default::default()
    };
    let mut stream = docker.logs(container, Some(options));
    let mut out = Vec::new();
    while let Some(Ok(chunk)) = stream.next().await {
        out.extend(log_lines(&chunk));
    }
    out
}

/// Splits one Docker log frame into lines, parsing the timestamp prefix.
pub fn log_lines(chunk: &bollard::container::LogOutput) -> Vec<LogLine> {
    use bollard::container::LogOutput;
    let (stream, bytes) = match chunk {
        LogOutput::StdErr { message } => (LogStream::Stderr, message),
        LogOutput::StdOut { message } | LogOutput::Console { message } => {
            (LogStream::Stdout, message)
        }
        LogOutput::StdIn { .. } => return Vec::new(),
    };
    String::from_utf8_lossy(bytes)
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(|raw| {
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            let parsed = raw.split_once(' ').and_then(|(ts, rest)| {
                chrono::DateTime::parse_from_rfc3339(ts)
                    .ok()
                    .map(|t| (t.with_timezone(&chrono::Utc), rest))
            });
            match parsed {
                Some((ts, line)) => LogLine {
                    ts: Some(ts),
                    stream,
                    line: line.to_string(),
                },
                None => LogLine {
                    ts: None,
                    stream,
                    line: raw.to_string(),
                },
            }
        })
        .collect()
}

// Networks, volumes, images

pub async fn ensure_network(docker: &Docker, name: &str) -> Result<()> {
    match docker.inspect_network(name, None).await {
        Ok(_) => return Ok(()),
        Err(e) if is_not_found(&e) => {}
        Err(e) => return Err(e).with_context(|| format!("cannot inspect network {name}")),
    }
    let request = NetworkCreateRequest {
        name: name.to_string(),
        driver: Some("bridge".into()),
        labels: Some(HashMap::from([(
            brand::label("managed"),
            "true".to_string(),
        )])),
        ..Default::default()
    };
    match docker.create_network(request).await {
        Ok(_) => Ok(()),
        Err(e) if is_conflict(&e) => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot create network {name}")),
    }
}

pub async fn remove_network(docker: &Docker, name: &str) -> Result<()> {
    match docker.remove_network(name).await {
        Ok(()) => Ok(()),
        Err(e) if is_not_found(&e) => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot remove network {name}")),
    }
}

/// Removes the networks a compose project created and, with `volumes`, its named volumes.
/// Its containers must be gone already.
pub async fn remove_compose_leftovers(docker: &Docker, project: &str, volumes: bool) -> Result<()> {
    let filters = HashMap::from([(
        "label".to_string(),
        vec![format!("com.docker.compose.project={project}")],
    )]);
    let networks = docker
        .list_networks(Some(ListNetworksOptions {
            filters: Some(filters.clone()),
        }))
        .await
        .context("cannot list networks")?;
    for name in networks.into_iter().filter_map(|n| n.name) {
        remove_network(docker, &name).await?;
    }
    if volumes {
        let listed = docker
            .list_volumes(Some(ListVolumesOptions {
                filters: Some(filters),
            }))
            .await
            .context("cannot list volumes")?;
        for v in listed.volumes.unwrap_or_default() {
            remove_volume(docker, &v.name).await?;
        }
    }
    Ok(())
}

/// Creates the volume unless it exists; true when it was created.
pub async fn create_volume_if_missing(docker: &Docker, name: &str) -> Result<bool> {
    match docker.inspect_volume(name).await {
        Ok(_) => Ok(false),
        Err(e) if is_not_found(&e) => ensure_volume(docker, name).await.map(|()| true),
        Err(e) => Err(e).with_context(|| format!("cannot inspect volume {name}")),
    }
}

/// Where the volume's files are on its server, for bind-mounting single files out of it.
pub async fn volume_mountpoint(docker: &Docker, name: &str) -> Result<String> {
    let volume = docker
        .inspect_volume(name)
        .await
        .with_context(|| format!("cannot inspect volume {name}"))?;
    if volume.mountpoint.is_empty() {
        bail!(
            "volume {name} uses the {} driver, which has no path on the server",
            volume.driver
        );
    }
    Ok(volume.mountpoint)
}

pub async fn ensure_volume(docker: &Docker, name: &str) -> Result<()> {
    create_volume(
        docker,
        name,
        HashMap::from([(brand::label("managed"), "true".to_string())]),
    )
    .await
}

/// Creates a volume, or does nothing when one of that name exists.
pub async fn create_volume(
    docker: &Docker,
    name: &str,
    labels: HashMap<String, String>,
) -> Result<()> {
    let request = VolumeCreateRequest {
        name: Some(name.to_string()),
        labels: Some(labels),
        ..Default::default()
    };
    docker
        .create_volume(request)
        .await
        .map(drop)
        .with_context(|| format!("cannot create volume {name}"))
}

/// The volume's labels, or `None` when it does not exist.
pub async fn volume_labels(docker: &Docker, name: &str) -> Result<Option<HashMap<String, String>>> {
    match docker.inspect_volume(name).await {
        Ok(v) => Ok(Some(v.labels)),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot inspect volume {name}")),
    }
}

/// Names and labels of the volumes carrying `label`.
pub async fn volumes_by_label(
    docker: &Docker,
    label: &str,
) -> Result<Vec<(String, HashMap<String, String>)>> {
    let filters = HashMap::from([("label".to_string(), vec![label.to_string()])]);
    let listed = docker
        .list_volumes(Some(ListVolumesOptions {
            filters: Some(filters),
        }))
        .await
        .context("cannot list volumes")?;
    Ok(listed
        .volumes
        .unwrap_or_default()
        .into_iter()
        .map(|v| (v.name, v.labels))
        .collect())
}

/// Streams the contents of volume `name` on `from` into the volume of the same name on `to`,
/// which must exist. Each side mounts its volume in a helper container that is created but never
/// started, and the archive endpoints carry a tar between them, owners and modes included.
/// Returns the size of the tar.
pub async fn copy_volume(from: &Docker, to: &Docker, name: &str, helper: &str) -> Result<u64> {
    const MOUNT: &str = "/volume";
    let holder = |docker: &Docker| {
        let docker = docker.clone();
        async move {
            let body = ContainerCreateBody {
                image: Some(HELPER_IMAGE.to_string()),
                labels: Some(HashMap::from([(
                    brand::label("managed"),
                    "true".to_string(),
                )])),
                host_config: Some(HostConfig {
                    binds: Some(vec![format!("{name}:{MOUNT}")]),
                    ..Default::default()
                }),
                ..Default::default()
            };
            remove_container(&docker, helper).await?;
            docker
                .create_container(
                    Some(CreateContainerOptions {
                        name: Some(helper.to_string()),
                        ..Default::default()
                    }),
                    body,
                )
                .await
                .map(|c| c.id)
                .with_context(|| format!("cannot create helper container {helper}"))
        }
    };
    let source = holder(from).await?;
    let copied = async {
        let target = holder(to).await?;
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = sent.clone();
        let tar = from
            .download_from_container(
                &source,
                Some(DownloadFromContainerOptions {
                    path: MOUNT.to_string(),
                }),
            )
            .map(move |chunk| {
                chunk
                    .inspect(|b| {
                        counter.fetch_add(b.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    })
                    .map_err(std::io::Error::other)
            });
        let uploaded = to
            .upload_to_container(
                &target,
                Some(UploadToContainerOptions {
                    path: "/".to_string(),
                    ..Default::default()
                }),
                bollard::body_try_stream(tar),
            )
            .await
            .with_context(|| format!("cannot copy volume {name}"));
        let removed = remove_container(to, &target).await;
        uploaded?;
        removed?;
        anyhow::Ok(sent.load(std::sync::atomic::Ordering::Relaxed))
    }
    .await;
    let removed = remove_container(from, &source).await;
    let size = copied?;
    removed?;
    Ok(size)
}

/// Streams an image from one engine into another, like `docker save | docker load`. Returns the
/// size of the archive.
pub async fn copy_image(from: &Docker, to: &Docker, image: &str) -> Result<u64> {
    let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter = sent.clone();
    let archive = from.export_image(image).map(move |chunk| {
        chunk.inspect(|b| {
            counter.fetch_add(b.len() as u64, std::sync::atomic::Ordering::Relaxed);
        })
    });
    let mut loading = to.import_image_stream(
        ImportImageOptions {
            quiet: true,
            ..Default::default()
        },
        archive,
        None,
    );
    while let Some(info) = loading.next().await {
        let info = info.with_context(|| format!("cannot copy image {image}"))?;
        if let Some(message) = info.error_detail.and_then(|e| e.message) {
            bail!("cannot load image {image}: {message}");
        }
    }
    if !image_exists(to, image).await? {
        bail!("copying image {image} finished without the image on the target");
    }
    Ok(sent.load(std::sync::atomic::Ordering::Relaxed))
}

pub async fn remove_volume(docker: &Docker, name: &str) -> Result<()> {
    match docker
        .remove_volume(name, Some(RemoveVolumeOptions { force: false }))
        .await
    {
        Ok(()) => Ok(()),
        Err(e) if is_not_found(&e) => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot remove volume {name}")),
    }
}

/// Pulls an image, reporting condensed progress lines (no per-chunk download noise).
pub async fn pull_image(
    docker: &Docker,
    image: &str,
    mut on_line: impl FnMut(String),
) -> Result<()> {
    let (repo, tag) = match image.rsplit_once(':') {
        Some((repo, tag)) if !image.contains('@') && !tag.contains('/') => (repo, Some(tag)),
        _ => (image, None),
    };
    let options = CreateImageOptions {
        from_image: Some(repo.to_string()),
        tag: tag.map(str::to_string),
        ..Default::default()
    };
    let mut stream = docker.create_image(Some(options), None, None);
    while let Some(item) = stream.next().await {
        let info = item.with_context(|| format!("cannot pull {image}"))?;
        if let Some(err) = info.error_detail.and_then(|e| e.message) {
            bail!("cannot pull {image}: {err}");
        }
        let Some(status) = info.status else { continue };
        if matches!(
            status.as_str(),
            "Downloading" | "Extracting" | "Waiting" | "Verifying Checksum"
        ) {
            continue;
        }
        match info.id {
            Some(id) => on_line(format!("{id}: {status}")),
            None => on_line(status),
        }
    }
    Ok(())
}

pub async fn image_exists(docker: &Docker, image: &str) -> Result<bool> {
    match docker.inspect_image(image).await {
        Ok(_) => Ok(true),
        Err(e) if is_not_found(&e) => Ok(false),
        Err(e) => Err(e).with_context(|| format!("cannot inspect image {image}")),
    }
}

// Proxy

pub async fn proxy_status(docker: &Docker) -> Result<ProxyStatus> {
    Ok(match inspect(docker, brand::PROXY_CONTAINER).await? {
        None => ProxyStatus::Missing,
        Some(c) if RunState::from_inspect(&c) == RunState::Running => ProxyStatus::Running,
        Some(_) => ProxyStatus::Stopped,
    })
}

/// Hostname the proxy reaches the Docker host, and so the daemon, under.
pub const HOST_GATEWAY: &str = "host.docker.internal";

/// The instance domain served by the proxy: HTTPS on `domain`, forwarded to the daemon's API
/// on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRoute {
    pub domain: String,
    /// Port the daemon listens on.
    pub port: u16,
    /// The domain resolves to Cloudflare's proxy, where only the HTTP-01 challenge can work.
    pub proxied: bool,
}

/// Everything the proxy container is configured with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    pub route: Option<InstanceRoute>,
    /// Proxies whose forwarded visitor address (`CF-Connecting-IP`) is believed, as
    /// space-separated CIDRs: Cloudflare's ranges.
    pub trusted: String,
}

/// Labels on the proxy container itself: global options, and the routes that send the instance
/// domain to the daemon. Responses are flushed at once so log streams arrive line by line. On
/// plain http only the domain check's token goes to the daemon, so the check can tell whether a
/// request really arrived over http; everything else redirects to https as usual. The routes
/// answer errors of their own with the branded pages, or JSON for API clients.
pub fn proxy_container_labels(config: &ProxyConfig) -> HashMap<String, String> {
    let mut labels = HashMap::from([
        (brand::label("proxy"), "true".to_string()),
        (
            brand::label("proxy.pages"),
            proxy_pages::VERSION.to_string(),
        ),
        (
            "caddy_0.servers.trusted_proxies".to_string(),
            format!("static {}", config.trusted),
        ),
        (
            "caddy_0.servers.client_ip_headers".to_string(),
            "CF-Connecting-IP".to_string(),
        ),
        ("caddy_0.metrics.per_host".to_string(), String::new()),
    ]);
    let Some(route) = &config.route else {
        return labels;
    };
    let upstream = format!("{HOST_GATEWAY}:{}", route.port);
    let scheme = |s: &str| format!("{} {s}", crate::domain::scheme_header());
    labels.extend([
        ("caddy".to_string(), route.domain.clone()),
        ("caddy.reverse_proxy".to_string(), upstream.clone()),
        (
            "caddy.reverse_proxy.flush_interval".to_string(),
            "-1".into(),
        ),
        (
            "caddy.reverse_proxy.header_up_0".to_string(),
            scheme("https"),
        ),
        (
            "caddy.reverse_proxy.header_up_1".to_string(),
            REAL_IP_HEADER.into(),
        ),
        ("caddy.import".to_string(), proxy_pages::snippet()),
        ("caddy_1".to_string(), format!("http://{}", route.domain)),
        ("caddy_1.import".to_string(), proxy_pages::snippet()),
        (
            "caddy_1.0_handle".to_string(),
            format!("{}*", crate::domain::verify_prefix()),
        ),
        ("caddy_1.0_handle.reverse_proxy".to_string(), upstream),
        (
            "caddy_1.0_handle.reverse_proxy.header_up_0".to_string(),
            scheme("http"),
        ),
        (
            "caddy_1.0_handle.reverse_proxy.header_up_1".to_string(),
            REAL_IP_HEADER.into(),
        ),
        (
            "caddy_1.1_handle.redir".to_string(),
            "https://{host}{uri} 308".into(),
        ),
    ]);
    if route.proxied {
        labels.extend([
            ("caddy.tls.issuer".to_string(), "acme".to_string()),
            (
                "caddy.tls.issuer.disable_tlsalpn_challenge".to_string(),
                String::new(),
            ),
        ]);
    }
    labels
}

/// The labels the proxy container was created with, if there is one, leaving out those its
/// image brings.
pub async fn proxy_labels_now(docker: &Docker) -> Result<Option<HashMap<String, String>>> {
    let ours = |k: &String| k.starts_with("caddy") || k.starts_with(&brand::label("proxy"));
    Ok(inspect(docker, brand::PROXY_CONTAINER).await?.map(|c| {
        c.config
            .and_then(|c| c.labels)
            .unwrap_or_default()
            .into_iter()
            .filter(|(k, _)| ours(k))
            .collect()
    }))
}

/// Creates (or recreates) the proxy container on the shared proxy network, with `pages` (from
/// [`proxy_pages::archive`]) in place before it starts.
pub async fn install_proxy(docker: &Docker, config: &ProxyConfig, pages: Vec<u8>) -> Result<()> {
    ensure_network(docker, brand::PROXY_NETWORK).await?;
    if !image_exists(docker, PROXY_IMAGE).await? {
        pull_image(docker, PROXY_IMAGE, |line| {
            tracing::debug!("proxy pull: {line}")
        })
        .await?;
    }
    remove_container(docker, brand::PROXY_CONTAINER).await?;
    let spec = ContainerSpec {
        name: brand::PROXY_CONTAINER.to_string(),
        image: PROXY_IMAGE.to_string(),
        env: vec![
            format!("CADDY_INGRESS_NETWORKS={}", brand::PROXY_NETWORK),
            format!(
                "CADDY_DOCKER_CADDYFILE_PATH={}",
                proxy_pages::caddyfile_path()
            ),
            format!("CADDY_DOCKER_POLLING_INTERVAL={PROXY_POLLING}"),
        ],
        labels: proxy_container_labels(config),
        extra_hosts: vec![format!("{HOST_GATEWAY}:host-gateway")],
        binds: vec![
            "/var/run/docker.sock:/var/run/docker.sock:ro".to_string(),
            format!("{}:/data", proxy_volume()),
        ],
        restart: RestartPolicy::UnlessStopped,
        networks: vec![(brand::PROXY_NETWORK.to_string(), Vec::new())],
        published: vec![
            ("80/tcp".into(), 80),
            ("443/tcp".into(), 443),
            ("443/udp".into(), 443),
        ],
        ..Default::default()
    };
    let id = create_container(docker, &spec).await?;
    write_proxy_pages(docker, pages).await?;
    start_container(docker, &id).await
}

/// How often the proxy rereads its base Caddyfile, which changes without a Docker event when
/// [`write_proxy_pages`] holds or releases a domain.
const PROXY_POLLING: &str = "5s";

/// Writes the proxy's base Caddyfile and pages. A running proxy picks them up on its next poll.
pub async fn write_proxy_pages(docker: &Docker, pages: Vec<u8>) -> Result<()> {
    docker
        .upload_to_container(
            brand::PROXY_CONTAINER,
            Some(UploadToContainerOptions {
                path: "/srv".into(),
                ..Default::default()
            }),
            bollard::body_full(pages.into()),
        )
        .await
        .context("cannot write the error pages into the proxy")
}

/// Removes everything the daemon put on a server: managed containers, the proxy, managed
/// networks and, with `volumes`, managed volumes and the proxy's certificate volume.
pub async fn purge(docker: &Docker, volumes: bool) -> Result<ServerCleanup> {
    let mut cleanup = ServerCleanup::default();
    let mut containers: Vec<String> = list_managed(docker)
        .await?
        .into_iter()
        .filter_map(|c| c.id)
        .collect();
    if inspect(docker, brand::PROXY_CONTAINER).await?.is_some() {
        containers.push(brand::PROXY_CONTAINER.to_string());
    }
    for id in &containers {
        remove_container(docker, id).await?;
        cleanup.containers += 1;
    }
    let managed = HashMap::from([(
        "label".to_string(),
        vec![format!("{}=true", brand::label("managed"))],
    )]);
    let networks = docker
        .list_networks(Some(ListNetworksOptions {
            filters: Some(managed.clone()),
        }))
        .await
        .context("cannot list networks")?;
    for name in networks.into_iter().filter_map(|n| n.name) {
        remove_network(docker, &name).await?;
        cleanup.networks += 1;
    }
    if volumes {
        let listed = docker
            .list_volumes(Some(ListVolumesOptions {
                filters: Some(managed),
            }))
            .await
            .context("cannot list volumes")?;
        let mut names: Vec<String> = listed
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| v.name)
            .collect();
        if docker.inspect_volume(&proxy_volume()).await.is_ok() {
            names.push(proxy_volume());
        }
        for name in names {
            remove_volume(docker, &name).await?;
            cleanup.volumes += 1;
        }
    }
    Ok(cleanup)
}

// Errors

pub fn status_code(e: &DockerError) -> Option<u16> {
    match e {
        DockerError::DockerResponseServerError { status_code, .. } => Some(*status_code),
        _ => None,
    }
}

pub fn is_not_found(e: &DockerError) -> bool {
    status_code(e) == Some(404)
}

fn is_not_modified(e: &DockerError) -> bool {
    status_code(e) == Some(304)
}

fn is_conflict(e: &DockerError) -> bool {
    status_code(e) == Some(409)
}

pub fn short_id(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn domain(host: &str, port: Option<u16>, tls: bool) -> Domain {
        Domain {
            id: "d".into(),
            service_id: "s".into(),
            host: host.into(),
            port,
            tls,
            compose_service: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn images_get_a_default_tag() {
        assert_eq!(normalize_image("nginx"), "nginx:latest");
        assert_eq!(normalize_image("traefik/whoami"), "traefik/whoami:latest");
        assert_eq!(normalize_image("redis:7"), "redis:7");
        assert_eq!(
            normalize_image("localhost:5000/app"),
            "localhost:5000/app:latest"
        );
        assert_eq!(normalize_image("app@sha256:abc"), "app@sha256:abc");
    }

    #[test]
    fn replicas_add_their_address_to_each_domain() {
        let domains = [
            domain("a.com", None, true),
            domain("b.localhost", Some(81), false),
            domain("c.com", None, true),
        ];
        let labels = upstream_labels(&domains, Some(80));
        assert_eq!(labels["caddy_0"], "a.com");
        assert_eq!(labels["caddy_0.reverse_proxy"], "{{upstreams 80}}");
        assert_eq!(labels["caddy_1"], "http://b.localhost");
        assert_eq!(labels["caddy_1.reverse_proxy"], "{{upstreams 81}}");
        assert_eq!(labels["caddy_2"], "c.com");
        assert_eq!(labels["caddy_0.import"], proxy_pages::snippet());
        assert_eq!(
            labels.len(),
            9,
            "replicas carry no proxy options: {labels:?}"
        );
        assert!(upstream_labels(&[domain("a.com", None, true)], None).is_empty());
    }

    #[test]
    fn the_route_carries_every_option_but_no_upstream() {
        let domains = [domain("a.com", None, true), domain("b.com", None, false)];
        let route = route_labels(&domains, Some(80), LbPolicy::IpHash, Some("healthz"));
        assert_eq!(route["caddy_0"], "a.com");
        assert_eq!(route["caddy_1"], "http://b.com");
        assert!(!route.contains_key("caddy_0.reverse_proxy"));
        assert_eq!(route["caddy_0.reverse_proxy.lb_policy"], "ip_hash");
        assert_eq!(route["caddy_0.import"], proxy_pages::snippet());
        assert_eq!(route["caddy_1.reverse_proxy.health_uri"], "/healthz");
        assert_eq!(route["caddy_0.reverse_proxy.fail_duration"], "30s");
        assert_eq!(route["caddy_0.reverse_proxy.lb_retries"], "3");
        assert!(!route.contains_key("caddy_0.reverse_proxy.lb_try_duration"));
        assert_eq!(route["caddy_0.reverse_proxy.transport.dial_timeout"], "1s");
        let plain = route_labels(&domains, Some(80), LbPolicy::LeastConn, None);
        assert_eq!(plain["caddy_0.reverse_proxy.lb_policy"], "least_conn");
        assert!(!plain.contains_key("caddy_0.reverse_proxy.health_uri"));
        // Every site a replica names has its options on the route, and nothing else.
        let replica = upstream_labels(&domains, Some(80));
        for (k, v) in replica
            .iter()
            .filter(|(k, _)| !k.ends_with(".reverse_proxy"))
        {
            assert_eq!(route.get(k), Some(v));
        }
        assert!(route_labels(&domains, None, LbPolicy::LeastConn, None).is_empty());
    }

    #[test]
    fn the_instance_domain_routes_to_the_daemon_on_the_host() {
        let config = |proxied| ProxyConfig {
            route: Some(InstanceRoute {
                domain: "deploy.example.com".into(),
                port: 7700,
                proxied,
            }),
            trusted: "173.245.48.0/20 2400:cb00::/32".into(),
        };
        let labels = proxy_container_labels(&config(false));
        assert_eq!(labels["caddy"], "deploy.example.com");
        assert_eq!(
            labels["caddy.reverse_proxy"],
            format!("{HOST_GATEWAY}:7700")
        );
        assert_eq!(labels["caddy.reverse_proxy.flush_interval"], "-1");
        assert_eq!(labels["caddy_1"], "http://deploy.example.com");
        assert_eq!(
            labels["caddy_1.0_handle"],
            format!("/.well-known/acme-challenge/{}-verify-*", brand::PREFIX)
        );
        assert_eq!(
            labels["caddy_1.0_handle.reverse_proxy.header_up_0"],
            format!("x-{}-origin-scheme http", brand::PREFIX)
        );
        assert_eq!(labels["caddy_1.1_handle.redir"], "https://{host}{uri} 308");
        assert_eq!(labels["caddy.import"], proxy_pages::snippet());
        assert_eq!(labels["caddy_1.import"], proxy_pages::snippet());
        assert_eq!(labels[&brand::label("proxy.pages")], proxy_pages::VERSION);
        assert!(!labels.contains_key("caddy.tls.issuer"));

        let proxied = proxy_container_labels(&config(true));
        assert_eq!(proxied["caddy.tls.issuer"], "acme");
        assert_eq!(proxied["caddy.tls.issuer.disable_tlsalpn_challenge"], "");
    }

    #[test]
    fn the_proxy_trusts_cloudflare_for_the_visitor_address() {
        let labels = proxy_container_labels(&ProxyConfig {
            route: None,
            trusted: "173.245.48.0/20 2400:cb00::/32".into(),
        });
        assert_eq!(
            labels["caddy_0.servers.trusted_proxies"],
            "static 173.245.48.0/20 2400:cb00::/32"
        );
        assert_eq!(
            labels["caddy_0.servers.client_ip_headers"],
            "CF-Connecting-IP"
        );
        assert_eq!(labels[&brand::label("proxy")], "true");
        assert!(!labels.contains_key("caddy"));
        let service = route_labels(
            &[domain("a.com", None, true)],
            Some(80),
            LbPolicy::LeastConn,
            None,
        );
        assert_eq!(
            service["caddy_0.reverse_proxy.header_up"],
            "X-Real-IP {client_ip}"
        );
    }

    #[test]
    fn exit_codes_parse_from_status() {
        assert_eq!(parse_exit_code("Exited (137) 2 minutes ago"), Some(137));
        assert_eq!(parse_exit_code("Up 3 hours"), None);
    }

    #[test]
    fn log_frames_split_into_timestamped_lines() {
        let chunk = bollard::container::LogOutput::StdErr {
            message: "2026-01-02T03:04:05.123456789Z hello world\nno-timestamp\n".into(),
        };
        let lines = log_lines(&chunk);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line, "hello world");
        assert_eq!(lines[0].stream, LogStream::Stderr);
        assert!(lines[0].ts.is_some());
        assert_eq!(lines[1].ts, None);
        assert_eq!(lines[1].line, "no-timestamp");
    }

    #[test]
    fn names_use_the_prefix() {
        assert_eq!(
            container_name("web", "abcdefghijkl", 3, 2),
            format!("{}-web-abcdef-3-r2", brand::PREFIX)
        );
        assert_eq!(build_tag("svc", 2), format!("{}/svc:2", brand::PREFIX));
    }
}
