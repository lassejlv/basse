//! Domain types and the HTTP API contract shared by the daemon, client, CLI and TUI.
//!
//! Every type here is serialized as JSON on the wire. Enums use `snake_case` tags.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod api;
pub mod brand;
pub mod notify;

/// Default TCP port the daemon's API listens on.
pub const DEFAULT_PORT: u16 = 7700;
/// Prefix of every API route.
pub const API_PREFIX: &str = "/api/v1";

pub type Id = String;
pub type Timestamp = DateTime<Utc>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    /// The machine the daemon runs on, reached through the local Docker socket.
    Local,
    /// A remote machine reached over SSH with the daemon's key.
    Ssh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    Online,
    Offline,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyStatus {
    Running,
    Stopped,
    #[default]
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    pub id: Id,
    pub name: String,
    pub kind: ServerKind,
    /// Hostname or IP for SSH servers; `None` for the local server.
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// The `ssh_key` secret the daemon connects with; `None` uses the daemon's own key.
    #[serde(default)]
    pub ssh_key_secret_id: Option<Id>,
    #[serde(default)]
    pub status: ServerStatus,
    #[serde(default)]
    pub proxy: ProxyStatus,
    /// Docker engine version reported on the last check.
    pub docker_version: Option<String>,
    pub last_checked_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub description: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub id: Id,
    pub project_id: Id,
    pub name: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseKind {
    Postgres,
    Mysql,
    Mariadb,
    Redis,
    Mongo,
}

impl DatabaseKind {
    pub const ALL: [Self; 5] = [
        Self::Postgres,
        Self::Mysql,
        Self::Mariadb,
        Self::Redis,
        Self::Mongo,
    ];

    pub fn image(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::Redis => "redis",
            Self::Mongo => "mongo",
        }
    }

    /// Newest stable major tag of the official image.
    pub fn default_version(self) -> &'static str {
        match self {
            Self::Postgres => "18-alpine",
            Self::Mysql => "9.7",
            Self::Mariadb => "13",
            Self::Redis => "8-alpine",
            Self::Mongo => "9",
        }
    }

    pub fn port(self) -> u16 {
        match self {
            Self::Postgres => 5432,
            Self::Mysql | Self::Mariadb => 3306,
            Self::Redis => 6379,
            Self::Mongo => 27017,
        }
    }

    /// Where the database keeps its data inside the container for an image tag.
    ///
    /// Postgres 18 images keep `PGDATA` in `/var/lib/postgresql/<major>/docker` and refuse to
    /// start with a mount at the old `/var/lib/postgresql/data`, so newer and non-numeric tags
    /// mount the parent directory.
    pub fn data_path(self, version: &str) -> &'static str {
        match self {
            Self::Postgres if major_version(version).is_some_and(|major| major < 18) => {
                "/var/lib/postgresql/data"
            }
            Self::Postgres => "/var/lib/postgresql",
            Self::Mysql | Self::Mariadb => "/var/lib/mysql",
            Self::Redis => "/data",
            Self::Mongo => "/data/db",
        }
    }
}

/// Leading major number of an image tag: `18-alpine` is 18, `9.7` is 9, `latest` is `None`.
fn major_version(tag: &str) -> Option<u32> {
    let digits: String = tag.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

impl fmt::Display for DatabaseKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.image())
    }
}

impl std::str::FromStr for DatabaseKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|k| k.image() == s || (s == "postgresql" && *k == Self::Postgres))
            .ok_or_else(|| {
                format!(
                    "unknown database '{s}', expected one of postgres, mysql, mariadb, redis, mongo"
                )
            })
    }
}

/// Where a service's image comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    /// A prebuilt image pulled from a registry.
    Image { image: String },
    /// A git repository cloned and built with its Dockerfile on the target server.
    Git {
        repo: String,
        branch: String,
        /// Dockerfile path relative to `context`.
        dockerfile: String,
        /// Build context directory relative to the repository root.
        context: String,
        /// The [`GitSource`] that authenticates the clone, with `repo` as `owner/name`. `None`
        /// clones `repo` as a URL, anonymously or with the daemon's SSH key as a deploy key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_id: Option<Id>,
        /// Deploy when the tracked branch is pushed. Needs a GitHub App source.
        #[serde(default = "default_true")]
        auto_deploy: bool,
        /// How the image is built. Services from before build methods use their Dockerfile.
        #[serde(default)]
        build: Build,
    },
    /// A one-click database; the daemon fills in image, credentials and a data volume.
    Database { kind: DatabaseKind, version: String },
}

impl Source {
    /// Short human label: `acme/api@main`, `postgres:17-alpine`.
    pub fn label(&self) -> String {
        match self {
            Self::Image { image } => image.clone(),
            Self::Git { repo, branch, .. } => {
                let short = repo
                    .trim_end_matches(".git")
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_start_matches("git@")
                    .trim_start_matches("github.com/")
                    .trim_start_matches("github.com:")
                    .to_string();
                format!("{short}@{branch}")
            }
            Self::Database { kind, version } => format!("{}:{version}", kind.image()),
        }
    }
}

/// How a git service's image is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BuildMethod {
    /// Chosen on the first deploy from what the repository has: a Dockerfile, else a compose
    /// file, else Railpack. The choice is saved on the service.
    Auto,
    /// `docker build` with the service's Dockerfile and context.
    #[default]
    Dockerfile,
    /// `docker compose up` of a compose file, deployed as one group of containers.
    Compose,
    /// Railpack detects the language and builds the image with BuildKit; no Dockerfile needed.
    Railpack,
}

impl BuildMethod {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Dockerfile, Self::Compose, Self::Railpack];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dockerfile => "dockerfile",
            Self::Compose => "compose",
            Self::Railpack => "railpack",
        }
    }
}

impl fmt::Display for BuildMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for BuildMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|m| m.as_str() == s || (s == "docker-compose" && *m == Self::Compose))
            .ok_or_else(|| {
                format!("unknown build method '{s}', expected one of auto, dockerfile, compose, railpack")
            })
    }
}

/// Build settings of a git service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Build {
    #[serde(default)]
    pub method: BuildMethod,
    /// Dockerfile stage to build (`--target`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Compose file relative to the repository root; `None` looks for `compose.yaml`,
    /// `compose.yml`, `docker-compose.yaml` and `docker-compose.yml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_file: Option<String>,
}

/// What `GET /sources/{id}/detect` found at a branch's root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildDetection {
    /// Never `Auto`.
    pub method: BuildMethod,
    /// The Dockerfile or compose file that decided it; `None` for Railpack.
    pub file: Option<String>,
}

/// One container of a compose service group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComposeService {
    /// The name in the compose file.
    pub name: String,
    pub container: Option<ContainerInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    No,
    Always,
    #[default]
    UnlessStopped,
    OnFailure,
}

impl RestartPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Always => "always",
            Self::UnlessStopped => "unless-stopped",
            Self::OnFailure => "on-failure",
        }
    }
}

/// Live state of a service, computed by the daemon from its deployments and containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Created but never deployed.
    #[default]
    New,
    Building,
    Deploying,
    Running,
    Stopped,
    Failed,
    /// Container is restarting or the server cannot be reached.
    Degraded,
}

impl ServiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Building => "building",
            Self::Deploying => "deploying",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Degraded => "degraded",
        }
    }

    pub fn is_busy(self) -> bool {
        matches!(self, Self::Building | Self::Deploying)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    pub id: Id,
    pub environment_id: Id,
    pub server_id: Id,
    pub name: String,
    pub source: Source,
    /// Port the app listens on inside the container; the proxy routes domains to it.
    pub port: Option<u16>,
    /// Overrides the image's command, run through `sh -c`.
    pub command: Option<String>,
    #[serde(default)]
    pub restart: RestartPolicy,
    /// HTTP path probed before traffic switches to a new container, e.g. `/healthz`.
    pub health_path: Option<String>,
    pub created_at: Timestamp,
    /// Computed by the daemon on every read.
    #[serde(default)]
    pub state: ServiceState,
    /// First domain, for list views.
    #[serde(default)]
    pub primary_domain: Option<String>,
    #[serde(default)]
    pub last_deployed_at: Option<Timestamp>,
    /// The deployment whose container currently serves traffic.
    #[serde(default)]
    pub live_deployment_id: Option<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
    /// Secret values are masked in the TUI and CLI listings.
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Domain {
    pub id: Id,
    pub service_id: Id,
    pub host: String,
    /// Container port to route to; falls back to the service port.
    pub port: Option<u16>,
    /// Serve HTTPS with an automatic certificate. When false the route is plain HTTP.
    pub tls: bool,
    /// For compose services: the service in the compose file the domain routes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_service: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Volume {
    pub id: Id,
    pub service_id: Id,
    /// Short name; the Docker volume is `<brand prefix>-<service id>-<name>`.
    pub name: String,
    pub mount_path: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Queued,
    Building,
    Starting,
    /// Serving traffic.
    Live,
    /// Was live, replaced by a newer deployment.
    Superseded,
    Failed,
    Cancelled,
}

impl DeploymentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Building => "building",
            Self::Starting => "starting",
            Self::Live => "live",
            Self::Superseded => "superseded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_finished(self) -> bool {
        !matches!(self, Self::Queued | Self::Building | Self::Starting)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Clone,
    Build,
    Pull,
    Start,
    Health,
    Switch,
    Cleanup,
}

impl StepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clone => "clone",
            Self::Build => "build",
            Self::Pull => "pull",
            Self::Start => "start",
            Self::Health => "health",
            Self::Switch => "switch",
            Self::Cleanup => "cleanup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub kind: StepKind,
    pub status: StepStatus,
    /// Short context such as a commit sha, `step 7/12` or `GET /healthz`.
    pub detail: Option<String>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deployment {
    pub id: Id,
    pub service_id: Id,
    /// Per-service sequence number, starting at 1.
    pub number: i64,
    pub status: DeploymentStatus,
    /// What started it: `cli`, `tui`, `rollback`, `webhook`.
    pub trigger: String,
    pub commit: Option<String>,
    pub commit_message: Option<String>,
    /// Image reference the container runs.
    pub image: Option<String>,
    pub container_id: Option<String>,
    pub steps: Vec<Step>,
    pub error: Option<String>,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

impl Deployment {
    pub fn can_cancel(&self) -> bool {
        !self.status.is_finished()
            && !self.steps.iter().any(|step| {
                step.kind == StepKind::Switch
                    && matches!(step.status, StepStatus::Running | StepStatus::Done)
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityLevel {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    pub id: i64,
    pub at: Timestamp,
    pub level: ActivityLevel,
    pub service_id: Option<Id>,
    pub service_name: Option<String>,
    pub project_id: Option<Id>,
    pub message: String,
    /// Extra detail shown muted, e.g. a duration or exit code.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
    /// Lines written by the daemon itself, such as deploy step headers.
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub ts: Option<Timestamp>,
    pub stream: LogStream,
    pub line: String,
}

/// Snapshot of the daemon host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub version: String,
    pub hostname: String,
    pub public_ip: Option<String>,
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    /// Recent CPU samples, oldest first, one every 15 seconds.
    pub cpu_history: Vec<f32>,
    pub proxy: ProxyStatus,
    /// Public key remote servers must trust for SSH connections.
    pub ssh_public_key: Option<String>,
    /// `https://<instance domain>` once the domain is verified; clients prefer it.
    #[serde(default)]
    pub url: Option<String>,
}

/// Everything an operator can see and change about the daemon itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceSettings {
    pub version: String,
    pub hostname: String,
    /// The override when set, else the detected address.
    pub public_ip: Option<String>,
    pub public_ip_override: Option<String>,
    pub listen: String,
    pub data_dir: String,
    /// Bytes used by the data directory.
    pub data_dir_size: u64,
    /// Install and restart the proxy on the local server automatically.
    pub manage_proxy: bool,
    pub proxy: ProxyStatus,
    pub proxy_image: String,
    pub docker_version: Option<String>,
    pub ssh_public_key: Option<String>,
    /// What a prune would free on the local server; `None` when Docker cannot be asked.
    pub reclaimable: Option<Reclaimable>,
    /// Whether systemd supervises the daemon, so a self-update can restart it.
    pub supervised: bool,
    /// Domain the API is served on; see `GET /settings/domain` for the guide.
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub domain_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reclaimable {
    /// Old build images and dangling images a prune would remove.
    pub images: usize,
    pub image_bytes: u64,
    pub build_cache_bytes: u64,
}

impl Reclaimable {
    pub fn total(&self) -> u64 {
        self.image_bytes + self.build_cache_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneResult {
    pub reclaimed_bytes: u64,
    /// Image references that were removed.
    pub removed_images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionCheck {
    pub current: String,
    /// Latest published release tag, e.g. `v0.2.0`; `None` when it could not be fetched.
    pub latest: Option<String>,
    pub update_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateResult {
    pub from: String,
    pub to: String,
    /// `false` when `to` was already installed: nothing was replaced and nothing restarts.
    /// Daemons from before this field only answered when they updated.
    #[serde(default = "api::default_true")]
    pub updated: bool,
}

/// The hint a client shows when it and the daemon it talks to run different versions, like
/// "<cli> 0.3.0 ≠ <daemon> 0.4.0 · run <cli> update".
pub fn version_skew(cli: &str, daemon: &str) -> Option<String> {
    let (cli, daemon) = (cli.trim_start_matches('v'), daemon.trim_start_matches('v'));
    (cli != daemon).then(|| {
        format!(
            "{} {cli} ≠ {} {daemon} · run {} update",
            brand::CLI_BIN,
            brand::DAEMON_BIN,
            brand::CLI_BIN
        )
    })
}

/// What `DELETE /servers/{id}?purge=true` removed from the server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCleanup {
    pub containers: usize,
    pub networks: usize,
    pub volumes: usize,
    /// Services that ran there and were deleted with it.
    pub services: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceStats {
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_limit: u64,
    /// Samples, oldest first, one every 15 seconds for up to an hour.
    pub cpu_history: Vec<f32>,
    pub mem_history: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub started_at: Option<Timestamp>,
    pub restart_count: i64,
    pub exit_code: Option<i64>,
    pub ports: Vec<String>,
    pub networks: Vec<String>,
}

/// Everything the overview screen needs in one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    pub projects: Vec<ProjectNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectNode {
    pub project: Project,
    pub environments: Vec<EnvironmentNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentNode {
    pub environment: Environment,
    pub services: Vec<Service>,
}

/// Full detail of one service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceDetail {
    pub service: Service,
    pub project: Project,
    pub environment: Environment,
    pub server: Server,
    pub env: Vec<EnvVar>,
    pub domains: Vec<Domain>,
    pub volumes: Vec<Volume>,
    pub container: Option<ContainerInfo>,
    /// Connection details; only for database services.
    #[serde(default)]
    pub database: Option<DatabaseInfo>,
    /// Each service of a compose build with its container; empty for other build methods.
    #[serde(default)]
    pub compose: Vec<ComposeService>,
}

/// How to reach a one-click database. Only set on [`ServiceDetail`] for database services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub kind: DatabaseKind,
    /// Image tag, e.g. `18-alpine`.
    pub version: String,
    /// Hostname other services in the same environment connect to: the service name.
    pub host: String,
    /// Port inside the environment network.
    pub port: u16,
    /// `None` for Redis, which only has a password.
    pub username: Option<String>,
    /// Default database, where the engine has one (Postgres, MySQL, MariaDB).
    pub database: Option<String>,
    /// Secret: the TUI and CLI mask it unless asked to reveal it.
    pub password: String,
    /// Connection URL for services in the same environment, e.g.
    /// `postgres://user:pass@db:5432/db`.
    pub internal_url: String,
    /// Host port the database is published on, if public access is enabled.
    pub public_port: Option<u16>,
    /// Connection URL from outside, using the server's public address. Set when
    /// `public_port` is set and the address is known.
    pub public_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupStatus {
    Running,
    Done,
    Failed,
}

impl BackupStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTrigger {
    Manual,
    Schedule,
}

impl BackupTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
        }
    }
}

/// Where a backup file is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackupStorage {
    /// The daemon's data directory: `<data dir>/backups/<service id>/<file>`.
    #[default]
    Local,
    /// An S3-compatible storage connection, named by `storage_id`:
    /// `<path prefix>/<service id>/<file>` in its bucket.
    S3,
}

impl BackupStorage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
        }
    }
}

/// A dump of a database service, gzip-compressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    pub id: Id,
    pub service_id: Id,
    pub created_at: Timestamp,
    /// Compressed size; 0 while running or after a failure.
    pub size_bytes: u64,
    pub status: BackupStatus,
    pub trigger: BackupTrigger,
    /// `<timestamp>-<n>.<ext>.gz`, e.g. `20261120-030000-1.dump.gz`; also the download name.
    pub file_name: String,
    pub error: Option<String>,
    #[serde(default)]
    pub storage: BackupStorage,
    /// Storage connection the file lives in; `None` for local storage.
    #[serde(default)]
    pub storage_id: Option<Id>,
    pub finished_at: Option<Timestamp>,
}

/// Daily automatic backups of a database service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSchedule {
    pub enabled: bool,
    /// Hour of the day (UTC, 0-23) the backup runs.
    pub hour_utc: u8,
    /// Scheduled backups to keep per store; older ones are deleted. Manual backups are never
    /// pruned.
    pub keep: u32,
    /// Storage connection scheduled backups go to; `None` keeps them in the data directory.
    #[serde(default)]
    pub storage_id: Option<Id>,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            hour_utc: 3,
            keep: 7,
            storage_id: None,
        }
    }
}

/// What a secret holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    /// An SSH private key; its public key is derived and shown.
    SshKey,
    /// An access key id and secret for S3-compatible storage.
    S3Credentials,
    /// An opaque value.
    Generic,
    /// The credentials of a GitHub App. Managed by its git source and never revealed.
    GithubApp,
}

impl SecretKind {
    pub const ALL: [Self; 4] = [
        Self::SshKey,
        Self::S3Credentials,
        Self::Generic,
        Self::GithubApp,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SshKey => "ssh_key",
            Self::S3Credentials => "s3_credentials",
            Self::Generic => "generic",
            Self::GithubApp => "github_app",
        }
    }

    /// Short human label: `ssh key`, `s3 credentials`, `generic`.
    pub fn label(self) -> &'static str {
        match self {
            Self::SshKey => "ssh key",
            Self::S3Credentials => "s3 credentials",
            Self::Generic => "generic",
            Self::GithubApp => "github app",
        }
    }
}

impl fmt::Display for SecretKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SecretKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.replace('-', "_").as_str() {
            "ssh_key" | "ssh" => Ok(Self::SshKey),
            "s3_credentials" | "s3" => Ok(Self::S3Credentials),
            "generic" => Ok(Self::Generic),
            "github_app" => Ok(Self::GithubApp),
            _ => Err(format!(
                "unknown secret kind '{s}', expected one of ssh-key, s3, generic"
            )),
        }
    }
}

/// The plaintext of a secret. Clients send it when creating or replacing a secret, and only
/// `POST /secrets/{id}/reveal` returns it. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SecretValue {
    /// An OpenSSH private key (`-----BEGIN OPENSSH PRIVATE KEY-----`) without a passphrase.
    SshKey {
        private_key: String,
    },
    S3Credentials {
        access_key_id: String,
        secret_access_key: String,
    },
    Generic {
        value: String,
    },
    GithubApp {
        app_id: u64,
        /// The app's PEM private key, used to sign the JWTs that mint installation tokens.
        private_key: String,
        #[serde(default)]
        webhook_secret: Option<String>,
        #[serde(default)]
        client_id: Option<String>,
        #[serde(default)]
        client_secret: Option<String>,
    },
}

impl SecretValue {
    pub fn kind(&self) -> SecretKind {
        match self {
            Self::SshKey { .. } => SecretKind::SshKey,
            Self::S3Credentials { .. } => SecretKind::S3Credentials,
            Self::Generic { .. } => SecretKind::Generic,
            Self::GithubApp { .. } => SecretKind::GithubApp,
        }
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretValue({}, redacted)", self.kind())
    }
}

/// A stored secret. The value itself is encrypted at rest and never part of this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    pub id: Id,
    pub name: String,
    pub kind: SecretKind,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// The non-secret half: the OpenSSH public key of an `ssh_key`, the access key id of
    /// `s3_credentials`; `None` for `generic`.
    pub public_part: Option<String>,
    /// What references the secret; it cannot be deleted while this is not empty.
    #[serde(default)]
    pub used_by: Vec<Usage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    Server,
    Storage,
    Service,
    Source,
}

/// One thing that uses a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub kind: UsageKind,
    pub id: Id,
    pub name: String,
}

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            UsageKind::Server => "server",
            UsageKind::Storage => "storage",
            UsageKind::Service => "service",
            UsageKind::Source => "source",
        };
        write!(f, "{kind} {}", self.name)
    }
}

/// Response of `POST /secrets/{id}/reveal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevealedSecret {
    pub secret: Secret,
    pub value: SecretValue,
}

/// An S3-compatible bucket backups can be kept in: AWS S3, Cloudflare R2, Backblaze B2, MinIO
/// and others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageConnection {
    pub id: Id,
    pub name: String,
    /// Base URL of the S3 API, e.g. `https://s3.eu-west-1.amazonaws.com` or
    /// `https://<account>.r2.cloudflarestorage.com`.
    pub endpoint: String,
    /// Signing region; `auto` for R2.
    pub region: String,
    pub bucket: String,
    /// Key prefix inside the bucket, without leading or trailing slashes; may be empty.
    pub path_prefix: String,
    /// Address the bucket as `<endpoint>/<bucket>` instead of `<bucket>.<endpoint>`
    /// (MinIO and most self-hosted stores need this).
    pub path_style: bool,
    /// The `s3_credentials` secret used to sign requests.
    pub credentials_secret_id: Id,
    pub created_at: Timestamp,
    pub last_test: Option<StorageTest>,
}

/// Outcome of `POST /storage/{id}/test`: put, get and delete of a small probe object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageTest {
    pub ok: bool,
    pub at: Timestamp,
    pub error: Option<String>,
    /// Time the three requests took together.
    pub latency_ms: Option<u64>,
}

/// An encrypted archive of the daemon itself: its store, `config.toml`, `master.key` and
/// SSH directory. Restoring one is `<daemon> restore`, not an API call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceBackup {
    pub id: Id,
    pub created_at: Timestamp,
    pub status: BackupStatus,
    pub trigger: BackupTrigger,
    /// `instance-<timestamp>-<n>.tar.gz.enc`; also the download name.
    pub file_name: String,
    /// Encrypted size; 0 while running or after a failure.
    pub size_bytes: u64,
    pub storage: BackupStorage,
    pub storage_id: Option<Id>,
    pub error: Option<String>,
    pub finished_at: Option<Timestamp>,
}

/// Daily automatic instance backups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceBackupSchedule {
    pub enabled: bool,
    /// Hour of the day (UTC, 0-23) the backup runs.
    pub hour_utc: u8,
    /// Scheduled instance backups to keep per store.
    pub keep: u32,
    /// Where scheduled backups go; `None` is the data directory.
    pub storage_id: Option<Id>,
    /// Whether a passphrase is stored for scheduled runs. The passphrase itself is never
    /// returned.
    pub has_passphrase: bool,
}

impl Default for InstanceBackupSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            hour_utc: 2,
            keep: 7,
            storage_id: None,
            has_passphrase: false,
        }
    }
}

/// How a git source authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitSourceKind {
    /// A GitHub App, created through the manifest flow or connected by its id and key. Clones
    /// use short-lived installation tokens, and pushes arrive as webhooks.
    GithubApp,
    /// A personal access token. No webhooks.
    GithubToken,
}

impl GitSourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::GithubApp => "github app",
            Self::GithubToken => "token",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitSourceStatus {
    /// The manifest flow started; the app does not exist on GitHub yet.
    Pending,
    /// The app exists but is not installed on an account yet.
    NeedsInstall,
    Ready,
    /// The last check failed; `status_detail` says why.
    Error,
}

impl GitSourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::NeedsInstall => "needs install",
            Self::Ready => "ready",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitOwnerKind {
    User,
    Organization,
}

/// Where services clone from. Credentials live in a secret and never leave the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSource {
    pub id: Id,
    pub name: String,
    pub kind: GitSourceKind,
    /// GitHub's web URL, `https://github.com` or a GitHub Enterprise host.
    pub html_url: String,
    /// GitHub's REST API, `https://api.github.com` or `<enterprise>/api/v3`.
    pub api_url: String,
    /// Account or organization the app is installed on, or the token's user.
    pub owner: Option<String>,
    pub owner_kind: Option<GitOwnerKind>,
    pub app_id: Option<u64>,
    pub app_slug: Option<String>,
    pub installation_id: Option<u64>,
    pub status: GitSourceStatus,
    /// What to do next, or the error of the last check.
    pub status_detail: Option<String>,
    /// Where to install the app or change which repositories it sees.
    pub install_url: Option<String>,
    /// Where GitHub delivers push events.
    pub webhook_url: Option<String>,
    /// The secret holding the credentials.
    pub secret_id: Option<Id>,
    pub last_checked_at: Option<Timestamp>,
    pub created_at: Timestamp,
    /// Services that clone from this source; it cannot be deleted while this is not empty.
    #[serde(default)]
    pub used_by: Vec<Usage>,
}

/// A repository a source can clone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepo {
    /// `owner/name`.
    pub full_name: String,
    pub private: bool,
    pub default_branch: String,
    pub description: Option<String>,
    #[serde(default)]
    pub pushed_at: Option<Timestamp>,
}

/// One page of `GET /sources/{id}/repos`, most recently pushed first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoPage {
    pub repos: Vec<GitRepo>,
    /// 1-based.
    pub page: u32,
    pub per_page: u32,
    /// Repositories matching the query across all pages.
    pub total: u32,
}

impl RepoPage {
    pub fn pages(&self) -> u32 {
        self.total.div_ceil(self.per_page.max(1)).max(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranch {
    pub name: String,
    /// The repository's default branch.
    pub default: bool,
}

/// Response of `POST /sources/github`: open `setup_url` in a browser to create the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubAppSetup {
    pub source: GitSource,
    pub setup_url: String,
}

/// The domain the daemon's own API is served on, with automatic HTTPS, and the guide to set
/// it up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InstanceDomain {
    pub domain: Option<String>,
    /// `https://<domain>`, once every check passed.
    pub url: Option<String>,
    pub verified: bool,
    pub verified_at: Option<Timestamp>,
    pub public_ipv4: Option<String>,
    pub public_ipv6: Option<String>,
    /// The DNS records to create at the domain's DNS provider.
    pub records: Vec<DnsRecord>,
    /// Results of the last check, in order; empty before the first one.
    pub checks: Vec<DomainCheck>,
    pub checked_at: Option<Timestamp>,
    /// Set when the last check found the domain behind Cloudflare's proxy (orange cloud).
    #[serde(default)]
    pub cloudflare: Option<CloudflareProxy>,
}

/// How the instance domain goes through Cloudflare's proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudflareProxy {
    /// Record types that resolve to Cloudflare's edge, `A` and/or `AAAA`.
    pub records: Vec<String>,
    /// Record types that point straight at this server instead, as when only AAAA is proxied.
    #[serde(default)]
    pub direct: Vec<String>,
    /// Cloudflare forwarded an https request to this server over plain http: the zone's
    /// SSL/TLS mode is Flexible, which loops redirects. It should be Full (strict).
    pub flexible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRecord {
    /// `A` or `AAAA`.
    pub kind: String,
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainCheckKind {
    /// The domain resolves to this server's public address.
    Dns,
    /// Port 80 accepts connections (certificate challenges and redirects).
    Http,
    /// Port 443 accepts connections.
    Https,
    /// A certificate valid for the domain is served.
    Certificate,
    /// The API answers through the domain.
    Api,
}

impl DomainCheckKind {
    pub const ALL: [Self; 5] = [
        Self::Dns,
        Self::Http,
        Self::Https,
        Self::Certificate,
        Self::Api,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dns => "DNS points to this server",
            Self::Http => "Port 80 reachable",
            Self::Https => "Port 443 reachable",
            Self::Certificate => "Certificate issued",
            Self::Api => "API reachable over HTTPS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    /// Not run because an earlier check failed.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainCheck {
    pub kind: DomainCheckKind,
    pub status: CheckStatus,
    pub detail: String,
    /// What to change when the check failed.
    pub hint: Option<String>,
}

/// Error body returned by every failing API call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

fn default_true() -> bool {
    true
}

/// Lowercase letters, digits and dashes, starting with a letter, at most 48 characters.
pub fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 48
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid name '{name}': use lowercase letters, digits and dashes, starting with a letter"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_round_trips_with_type_tag() {
        let src = Source::Git {
            repo: "https://github.com/acme/api".into(),
            branch: "main".into(),
            dockerfile: "Dockerfile".into(),
            context: ".".into(),
            source_id: None,
            auto_deploy: true,
            build: Build::default(),
        };
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains(r#""type":"git""#));
        assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), src);
        assert_eq!(src.label(), "acme/api@main");
    }

    #[test]
    fn git_sources_from_before_build_methods_keep_their_dockerfile() {
        let old = r#"{"type":"git","repo":"https://github.com/acme/api","branch":"main","dockerfile":"Dockerfile","context":"."}"#;
        let Source::Git {
            build,
            auto_deploy,
            source_id,
            ..
        } = serde_json::from_str(old).unwrap()
        else {
            panic!("not git");
        };
        assert_eq!(build.method, BuildMethod::Dockerfile);
        assert!(auto_deploy);
        assert_eq!(source_id, None);
        assert_eq!(
            "docker-compose".parse::<BuildMethod>().unwrap(),
            BuildMethod::Compose
        );
        assert!("make".parse::<BuildMethod>().is_err());
    }

    #[test]
    fn version_skew_names_both_sides() {
        assert_eq!(version_skew("0.4.0", "0.4.0"), None);
        assert_eq!(version_skew("0.4.0", "v0.4.0"), None);
        assert_eq!(
            version_skew("0.3.0", "0.4.0").unwrap(),
            format!(
                "{} 0.3.0 ≠ {} 0.4.0 · run {} update",
                brand::CLI_BIN,
                brand::DAEMON_BIN,
                brand::CLI_BIN
            )
        );
    }

    #[test]
    fn update_results_from_older_daemons_count_as_updated() {
        let old: UpdateResult = serde_json::from_str(r#"{"from":"0.3.0","to":"0.4.0"}"#).unwrap();
        assert!(old.updated);
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("web").is_ok());
        assert!(validate_name("api-v2").is_ok());
        assert!(validate_name("Web").is_err());
        assert!(validate_name("2web").is_err());
        assert!(validate_name("web-").is_err());
        assert!(validate_name("").is_err());
    }

    #[test]
    fn database_kind_parses() {
        assert_eq!(
            "postgresql".parse::<DatabaseKind>().unwrap(),
            DatabaseKind::Postgres
        );
        assert!("oracle".parse::<DatabaseKind>().is_err());
    }

    #[test]
    fn postgres_18_mounts_the_parent_data_directory() {
        let pg = DatabaseKind::Postgres;
        assert_eq!(pg.data_path("17-alpine"), "/var/lib/postgresql/data");
        assert_eq!(pg.data_path("9.6"), "/var/lib/postgresql/data");
        assert_eq!(pg.data_path("18-alpine"), "/var/lib/postgresql");
        assert_eq!(pg.data_path("19"), "/var/lib/postgresql");
        assert_eq!(pg.data_path("latest"), "/var/lib/postgresql");
        assert_eq!(DatabaseKind::Redis.data_path("8-alpine"), "/data");
    }
}
