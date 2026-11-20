//! Request bodies and the route table of the daemon API.
//!
//! All routes live under [`crate::API_PREFIX`] and require `Authorization: Bearer <token>`.
//! Successful responses are JSON; failures return [`crate::ApiError`] with a 4xx or 5xx status.
//! Streaming routes return newline-delimited JSON, one [`crate::LogLine`] per line.
//!
//! | Method | Path | Body | Response |
//! |---|---|---|---|
//! | GET | `/info` | | `DaemonInfo` |
//! | GET | `/tree` | | `Tree` |
//! | GET | `/activity?limit=50` | | `Vec<Activity>` |
//! | GET | `/servers` | | `Vec<Server>` |
//! | POST | `/servers` | `CreateServer` | `Server` |
//! | PATCH | `/servers/{id}` | `UpdateServer` | `Server` (reconnects with the new key and checks it) |
//! | DELETE | `/servers/{id}?purge=true&volumes=true` | | 204, or `ServerCleanup` with `purge` (removes what the daemon put there, its services included, then forgets it) |
//! | POST | `/servers/{id}/check` | | `Server` |
//! | POST | `/servers/{id}/proxy` | | `Server` (installs or restarts the proxy) |
//! | GET | `/projects` | | `Vec<Project>` |
//! | POST | `/projects` | `CreateProject` | `Project` (also creates a `production` environment) |
//! | DELETE | `/projects/{id}` | | 204 (removes environments, services, containers) |
//! | POST | `/projects/{id}/environments` | `CreateEnvironment` | `Environment` |
//! | DELETE | `/environments/{id}` | | 204 |
//! | POST | `/services` | `CreateService` | `ServiceDetail` |
//! | GET | `/services/{id}` | | `ServiceDetail` |
//! | PATCH | `/services/{id}` | `UpdateService` | `ServiceDetail` |
//! | DELETE | `/services/{id}` | | 204 (removes containers; volumes kept unless `?volumes=true`) |
//! | POST | `/services/{id}/deploy` | `DeployRequest` | `Deployment` (returns at once, runs in background) |
//! | POST | `/services/{id}/restart` | | `ServiceDetail` |
//! | POST | `/services/{id}/stop` | | `ServiceDetail` |
//! | POST | `/services/{id}/start` | | `ServiceDetail` |
//! | GET | `/services/{id}/logs?tail=200&follow=true` | | NDJSON `LogLine` stream |
//! | GET | `/services/{id}/stats` | | `ServiceStats` |
//! | PUT | `/services/{id}/env` | `Vec<EnvVar>` | `Vec<EnvVar>` (replaces all; applies on next deploy) |
//! | POST | `/services/{id}/domains` | `CreateDomain` | `Domain` (applied to the live container at once) |
//! | DELETE | `/domains/{id}` | | 204 |
//! | POST | `/services/{id}/volumes` | `CreateVolume` | `Volume` (mounted on next deploy) |
//! | DELETE | `/volumes/{id}?purge=true` | | 204 |
//! | GET | `/services/{id}/deployments?limit=20` | | `Vec<Deployment>`, newest first |
//! | GET | `/deployments/{id}` | | `Deployment` |
//! | GET | `/deployments/{id}/logs?follow=true` | | NDJSON `LogLine` stream, ends when the deployment finishes |
//! | POST | `/deployments/{id}/cancel` | | `Deployment` |
//! | POST | `/deployments/{id}/rollback` | | `Deployment` (new deployment reusing that image) |
//! | GET | `/settings` | | `InstanceSettings` |
//! | PATCH | `/settings` | `UpdateSettings` | `InstanceSettings` (persisted in the daemon's config.toml) |
//! | POST | `/settings/token/rotate` | | `RotatedToken` (the old token stops working at once) |
//! | POST | `/settings/proxy/restart` | | `Server` (reinstalls the proxy on the local server) |
//! | POST | `/settings/prune` | | `PruneResult` (old build images, dangling images, build cache) |
//! | GET | `/settings/version` | | `VersionCheck` (latest release, cached for an hour) |
//! | POST | `/settings/update` | `UpdateRequest` | `UpdateResult` (replaces the binaries, then the daemon exits to restart) |
//!
//! Database services (`Source::Database`) also have these routes; other services get 400.
//! `ServiceDetail.database` carries their `DatabaseInfo`.
//!
//! | Method | Path | Body | Response |
//! |---|---|---|---|
//! | PATCH | `/services/{id}/database` | `UpdateDatabase` | `DatabaseInfo` (publishing changes redeploy the live image) |
//! | POST | `/services/{id}/database/rotate-password` | | `DatabaseInfo` with the new password |
//! | GET | `/services/{id}/backups` | | `Vec<Backup>`, newest first |
//! | POST | `/services/{id}/backups` |  | `Backup` with status `running` (returns at once, runs in background) |
//! | GET | `/services/{id}/backups/schedule` | | `BackupSchedule` |
//! | PUT | `/services/{id}/backups/schedule` | `BackupSchedule` | `BackupSchedule` |
//! | GET | `/backups/{id}` | | `Backup` |
//! | GET | `/backups/{id}/download` | | the gzip file as `application/gzip` |
//! | POST | `/backups/{id}/restore` | | `Backup` (returns when the restore has finished) |
//! | DELETE | `/backups/{id}` | | 204 |
//!
//! Backups, restores and password rotations of one service run one at a time; a second one
//! while another is in progress gets 409. Restore and rotation also need a running container.
//!
//! Secrets are encrypted at rest with the daemon's master key. No route but `reveal` returns a
//! value, and every reveal is recorded in the activity feed.
//!
//! | Method | Path | Body | Response |
//! |---|---|---|---|
//! | GET | `/secrets` | | `Vec<Secret>` |
//! | POST | `/secrets` | `CreateSecret` | `Secret` (create or import) |
//! | POST | `/secrets/generate-ssh-key` | `GenerateSshKey` | `Secret` with the new ed25519 public key |
//! | GET | `/secrets/{id}` | | `Secret` |
//! | PATCH | `/secrets/{id}` | `UpdateSecret` | `Secret` (rename, or replace the value with one of the same kind) |
//! | DELETE | `/secrets/{id}` | | 204; 409 naming its users while it is in use |
//! | POST | `/secrets/{id}/reveal` | | `RevealedSecret` |

use crate::{EnvVar, Id, RestartPolicy, SecretValue, Source};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateServer {
    pub name: String,
    pub host: String,
    #[serde(default = "default_ssh_user")]
    pub user: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// An `ssh_key` secret to connect with instead of the daemon's own key.
    #[serde(default)]
    pub ssh_key_secret_id: Option<Id>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateServer {
    /// An `ssh_key` secret id, or an empty string to go back to the daemon's own key.
    /// `None` keeps the current key.
    pub ssh_key_secret_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSecret {
    pub name: String,
    pub value: SecretValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateSshKey {
    pub name: String,
}

/// Fields left as `None` keep their value. A new `value` must be of the secret's kind.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateSecret {
    pub name: Option<String>,
    pub value: Option<SecretValue>,
}

fn default_ssh_user() -> String {
    "root".into()
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnvironment {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub name: String,
    pub mount_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateService {
    pub environment_id: Id,
    /// Defaults to the local server.
    pub server_id: Option<Id>,
    pub name: String,
    pub source: Source,
    pub port: Option<u16>,
    pub command: Option<String>,
    #[serde(default)]
    pub restart: RestartPolicy,
    pub health_path: Option<String>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    /// Hostnames routed to the service with automatic HTTPS.
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<VolumeSpec>,
    /// Start the first deployment right away.
    #[serde(default)]
    pub deploy: bool,
}

/// Fields left as `None` keep their value. `Some(None)` is not used; send an empty string to clear
/// `command` or `health_path`, and `0` to clear `port`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateService {
    pub name: Option<String>,
    pub source: Option<Source>,
    pub port: Option<u16>,
    pub command: Option<String>,
    pub restart: Option<RestartPolicy>,
    pub health_path: Option<String>,
    pub server_id: Option<Id>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployRequest {
    /// `cli`, `tui` or `webhook`; defaults to `api`.
    pub trigger: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDomain {
    pub host: String,
    pub port: Option<u16>,
    #[serde(default = "default_true")]
    pub tls: bool,
}

fn default_true() -> bool {
    true
}

/// Fields left as `None` keep their value. An empty `public_ip_override` (or `auto`) clears the
/// override so the address is detected again.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateSettings {
    pub public_ip_override: Option<String>,
    pub manage_proxy: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotatedToken {
    pub token: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateRequest {
    /// Release tag such as `v0.2.0`; defaults to the latest release.
    pub version: Option<String>,
    /// Update even when no supervisor restarts the daemon, or when the version is not newer.
    #[serde(default)]
    pub force: bool,
}

/// Public access to a database service.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateDatabase {
    /// Host port to publish the database port on. `None` or `0` makes it internal-only again.
    pub public_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVolume {
    pub name: String,
    pub mount_path: String,
}
