//! Domain types and the HTTP API contract shared by the daemon, client, CLI and TUI.
//!
//! Every type here is serialized as JSON on the wire. Enums use `snake_case` tags.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod api;
pub mod brand;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSchedule {
    pub enabled: bool,
    /// Hour of the day (UTC, 0-23) the backup runs.
    pub hour_utc: u8,
    /// Scheduled backups to keep; older ones are deleted. Manual backups are never pruned.
    pub keep: u32,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            hour_utc: 3,
            keep: 7,
        }
    }
}

/// Error body returned by every failing API call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
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
        };
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains(r#""type":"git""#));
        assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), src);
        assert_eq!(src.label(), "acme/api@main");
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
