//! Domain types and the HTTP API contract shared by the Berth daemon, client, CLI and TUI.
//!
//! Every type here is serialized as JSON on the wire. Enums use `snake_case` tags.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod api;

/// Default TCP port the daemon's API listens on.
pub const DEFAULT_PORT: u16 = 7700;
/// Prefix of every API route.
pub const API_PREFIX: &str = "/api/v1";
/// Docker network shared by the proxy and every service that has a domain.
pub const PROXY_NETWORK: &str = "berth";
/// Container name of the reverse proxy on each server.
pub const PROXY_CONTAINER: &str = "berth-proxy";

pub type Id = String;
pub type Timestamp = DateTime<Utc>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    /// The machine berthd runs on, reached through the local Docker socket.
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
    pub const ALL: [Self; 5] = [Self::Postgres, Self::Mysql, Self::Mariadb, Self::Redis, Self::Mongo];

    pub fn image(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::Redis => "redis",
            Self::Mongo => "mongo",
        }
    }

    pub fn default_version(self) -> &'static str {
        match self {
            Self::Postgres => "17-alpine",
            Self::Mysql => "8.4",
            Self::Mariadb => "11",
            Self::Redis => "7-alpine",
            Self::Mongo => "8",
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

    /// Where the database keeps its data inside the container.
    pub fn data_path(self) -> &'static str {
        match self {
            Self::Postgres => "/var/lib/postgresql/data",
            Self::Mysql | Self::Mariadb => "/var/lib/mysql",
            Self::Redis => "/data",
            Self::Mongo => "/data/db",
        }
    }
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
            .ok_or_else(|| format!("unknown database '{s}', expected one of postgres, mysql, mariadb, redis, mongo"))
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
    /// Short name; the Docker volume is `berth-<service id>-<name>`.
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
    /// Lines written by berthd itself, such as deploy step headers.
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
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
        assert_eq!("postgresql".parse::<DatabaseKind>().unwrap(), DatabaseKind::Postgres);
        assert!("oracle".parse::<DatabaseKind>().is_err());
    }
}
