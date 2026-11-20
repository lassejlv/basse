//! Docker access: a client per server plus container, network, image and proxy helpers.

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
    CreateContainerOptions, CreateImageOptions, ListContainersOptions, RemoveContainerOptions,
    RemoveVolumeOptions, StopContainerOptions,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use common::{
    ContainerInfo, Domain, Id, LogLine, LogStream, ProxyStatus, RestartPolicy, Server, ServerKind,
    brand,
};
use futures_util::StreamExt as _;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, RwLock};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};

/// Reverse proxy image run on every server.
pub const PROXY_IMAGE: &str = "lucaslorentz/caddy-docker-proxy:2.12-alpine";
const DOCKER_TIMEOUT_SECS: u64 = 120;

/// One Docker client per server, created on first use. Remote servers are reached through an
/// SSH tunnel to their Docker socket.
pub struct DockerPool {
    clients: RwLock<HashMap<Id, Docker>>,
    tunnels: Mutex<HashMap<Id, Tunnel>>,
    connecting: tokio::sync::Mutex<()>,
    ssh: SshPaths,
}

impl DockerPool {
    pub fn new(ssh: SshPaths) -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
            connecting: tokio::sync::Mutex::new(()),
            ssh,
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
                let tunnel = Tunnel::open(&self.ssh, server).await?;
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

    /// Drops the cached client and closes its tunnel, e.g. after the server was deleted or a
    /// check failed.
    pub fn forget(&self, server_id: &str) {
        self.clients
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(server_id);
        lock(&self.tunnels).remove(server_id);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// Names

pub fn container_name(service_name: &str, env_id: &str, number: i64) -> String {
    let short: String = env_id.chars().take(6).collect();
    format!("{}-{service_name}-{short}-{number}", brand::PREFIX)
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

/// Labels routing the domains to the service through the proxy. Domains without a port
/// (none on the domain and none on the service) are skipped.
pub fn proxy_labels(
    domains: &[Domain],
    service_port: Option<u16>,
    service_id: &str,
) -> HashMap<String, String> {
    let alias = proxy_alias(service_id);
    let mut labels = HashMap::new();
    let routable = domains
        .iter()
        .filter_map(|d| d.port.or(service_port).map(|p| (d, p)));
    for (i, (domain, port)) in routable.enumerate() {
        let site = if domain.tls {
            domain.host.clone()
        } else {
            format!("http://{}", domain.host)
        };
        labels.insert(format!("caddy_{i}"), site);
        labels.insert(
            format!("caddy_{i}.reverse_proxy"),
            format!("{alias}:{port}"),
        );
        labels.insert(
            format!("caddy_{i}.reverse_proxy.lb_try_duration"),
            "5s".into(),
        );
    }
    labels
}

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
                host_ip: None,
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

/// Removes every container of a service on one server. Returns how many were removed.
pub async fn remove_service_containers(
    docker: &Docker,
    service_id: &str,
    keep: Option<&str>,
) -> Result<usize> {
    let mut removed = 0;
    for c in list_service_containers(docker, service_id).await? {
        let Some(id) = c.id else { continue };
        if keep.is_some_and(|k| k == id) {
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

pub async fn ensure_volume(docker: &Docker, name: &str) -> Result<()> {
    let request = VolumeCreateRequest {
        name: Some(name.to_string()),
        labels: Some(HashMap::from([(
            brand::label("managed"),
            "true".to_string(),
        )])),
        ..Default::default()
    };
    docker
        .create_volume(request)
        .await
        .map(drop)
        .with_context(|| format!("cannot create volume {name}"))
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

/// Creates (or recreates) the proxy container on the shared proxy network.
pub async fn install_proxy(docker: &Docker) -> Result<()> {
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
        env: vec![format!("CADDY_INGRESS_NETWORKS={}", brand::PROXY_NETWORK)],
        labels: HashMap::from([(brand::label("proxy"), "true".to_string())]),
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
    start_container(docker, &id).await
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
    fn proxy_labels_route_each_domain() {
        let labels = proxy_labels(
            &[
                domain("a.com", None, true),
                domain("b.localhost", Some(81), false),
                domain("c.com", None, true),
            ],
            Some(80),
            "svc",
        );
        let alias = proxy_alias("svc");
        assert_eq!(labels["caddy_0"], "a.com");
        assert_eq!(labels["caddy_0.reverse_proxy"], format!("{alias}:80"));
        assert_eq!(labels["caddy_1"], "http://b.localhost");
        assert_eq!(labels["caddy_1.reverse_proxy"], format!("{alias}:81"));
        assert_eq!(labels["caddy_2"], "c.com");
        assert!(proxy_labels(&[domain("a.com", None, true)], None, "svc").is_empty());
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
            container_name("web", "abcdefghijkl", 3),
            format!("{}-web-abcdef-3", brand::PREFIX)
        );
        assert_eq!(build_tag("svc", 2), format!("{}/svc:2", brand::PREFIX));
    }
}
