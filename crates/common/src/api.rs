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
//! | GET | `/services/{id}/logs?tail=200&follow=true&container=web` | | NDJSON `LogLine` stream; `container` picks a compose service |
//! | GET | `/services/{id}/stats` | | `ServiceStats` |
//! | PUT | `/services/{id}/env` | `Vec<EnvVar>` | `Vec<EnvVar>` (replaces all; applies on next deploy) |
//! | POST | `/services/{id}/domains` | `CreateDomain` | `Domain` (applied to the live container at once) |
//! | DELETE | `/domains/{id}` | | 204 |
//! | POST | `/services/{id}/volumes` | `CreateVolume` | `Volume` (mounted on next deploy) |
//! | DELETE | `/volumes/{id}?purge=true` | | 204 |
//! | GET | `/services/{id}/deployments?limit=20` | | `Vec<Deployment>`, newest first |
//! | GET | `/deployments/{id}` | | `Deployment` |
//! | GET | `/deployments/{id}/logs?follow=true` | | NDJSON `LogLine` stream, ends when the deployment finishes |
//! | POST | `/deployments/{id}/cancel` | | `Deployment` (cancelled after cleanup; 409 once live cutover starts) |
//! | POST | `/deployments/{id}/rollback` | | `Deployment` (new deployment reusing that image) |
//! | GET | `/settings` | | `InstanceSettings` |
//! | PATCH | `/settings` | `UpdateSettings` | `InstanceSettings` (persisted in the daemon's config.toml) |
//! | POST | `/settings/token/rotate` | | `RotatedToken` (the old token stops working at once) |
//! | POST | `/settings/proxy/restart` | | `Server` (reinstalls the proxy on the local server) |
//! | POST | `/settings/prune` | | `PruneResult` (old build images, dangling images, build cache) |
//! | GET | `/settings/version` | | `VersionCheck` (latest release, cached for an hour) |
//! | POST | `/settings/update` | `UpdateRequest` | `UpdateResult` (replaces the binaries, then the daemon exits to restart) |
//! | GET | `/settings/domain` | | `InstanceDomain` (the DNS records to create and the last check) |
//! | PUT | `/settings/domain` | `SetInstanceDomain` | `InstanceDomain` (routes the domain to the API through the proxy, then checks it) |
//! | POST | `/settings/domain/check` | | `InstanceDomain` (DNS, ports 80 and 443, certificate, API) |
//! | DELETE | `/settings/domain` | | `InstanceDomain` (the API is served on IP:port only again) |
//!
//! Git sources. Credentials are stored as secrets and never returned. GitHub App sources need a
//! verified instance domain, because GitHub calls back to it.
//!
//! | Method | Path | Body | Response |
//! |---|---|---|---|
//! | GET | `/sources` | | `Vec<GitSource>` |
//! | POST | `/sources/github` | `CreateGithubApp` | `GithubAppSetup` (open `setup_url` to create the app with the manifest flow) |
//! | POST | `/sources/github/connect` | `ConnectGithubApp` | `GitSource` (an existing app, checked against GitHub) |
//! | POST | `/sources/github/token` | `ConnectGithubToken` | `GitSource` (a personal access token) |
//! | GET | `/sources/{id}` | | `GitSource` |
//! | POST | `/sources/{id}/check` | | `GitSource` (mints a token and updates the status) |
//! | DELETE | `/sources/{id}` | | 204, or 409 while a service uses it |
//! | GET | `/sources/{id}/repos?q=&page=1&per_page=30` | | `RepoPage` (`q` matches `owner/name`) |
//! | GET | `/sources/{id}/branches?repo=owner/name` | | `Vec<GitBranch>`, default branch first |
//! | GET | `/sources/{id}/detect?repo=owner/name&branch=main` | | `BuildDetection` (Dockerfile, else compose file, else Railpack) |
//!
//! GitHub calls these without a bearer token; each checks its own proof instead.
//!
//! | Method | Path | Proof |
//! |---|---|---|
//! | GET | `/github/{id}/start?state=` | the setup state; an HTML page that posts the app manifest to GitHub |
//! | GET | `/github/{id}/callback?code=&state=` | the setup state; exchanges the code for the app's credentials, then redirects to the install page |
//! | GET | `/github/{id}/setup?installation_id=` | the installation must belong to the app, asked with the app's JWT |
//! | POST | `/github/{id}/webhook` | `X-Hub-Signature-256` with the app's webhook secret; a push to a tracked branch deploys |
//!
//! Git services build with `Source::Git.build`: a Dockerfile (with `target` and the service's
//! env as build args), a compose file deployed as one group (`ServiceDetail.compose` lists its
//! containers; domains name a `compose_service`), or Railpack. Changing the method with
//! `PATCH /services/{id}` applies on the next deploy.
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
//! Download and restore read from whichever store the backup lives in, local or S3.
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
//!
//! Storage connections are S3-compatible buckets for backups.
//!
//! | Method | Path | Body | Response |
//! |---|---|---|---|
//! | GET | `/storage` | | `Vec<StorageConnection>` |
//! | POST | `/storage` | `CreateStorage` | `StorageConnection` |
//! | GET | `/storage/{id}` | | `StorageConnection` |
//! | PATCH | `/storage/{id}` | `UpdateStorage` | `StorageConnection` |
//! | DELETE | `/storage/{id}` | | 204; 409 while backups or schedules use it |
//! | POST | `/storage/{id}/test` | | `StorageTest` (puts, reads and deletes a probe object) |
//!
//! Instance backups are encrypted archives of the daemon itself. Restoring one is the
//! `<daemon> restore` command on the new machine, not an API call.
//!
//! | Method | Path | Body | Response |
//! |---|---|---|---|
//! | GET | `/instance/backups` | | `Vec<InstanceBackup>`, newest first |
//! | POST | `/instance/backups` | `CreateInstanceBackup` | `InstanceBackup` with status `running` (runs in background) |
//! | GET | `/instance/backups/schedule` | | `InstanceBackupSchedule` |
//! | PUT | `/instance/backups/schedule` | `UpdateInstanceBackupSchedule` | `InstanceBackupSchedule` |
//! | GET | `/instance/backups/{id}/download` | | the encrypted archive as `application/octet-stream` |
//! | DELETE | `/instance/backups/{id}` | | 204 |

use crate::{EnvVar, Id, RestartPolicy, SecretValue, Source};
use serde::{Deserialize, Serialize};
use std::fmt;

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
    /// Required for compose builds: the service in the compose file to route to.
    #[serde(default)]
    pub compose_service: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetInstanceDomain {
    /// A hostname such as `deploy.example.com`, without scheme or path.
    pub domain: String,
}

/// Start the GitHub App manifest flow.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateGithubApp {
    pub name: String,
    /// Create the app in this organization; `None` creates it on the personal account of
    /// whoever opens the setup URL.
    #[serde(default)]
    pub organization: Option<String>,
    /// GitHub Enterprise web URL; defaults to `https://github.com`.
    #[serde(default)]
    pub html_url: Option<String>,
    /// GitHub Enterprise API URL; defaults to `https://api.github.com`, or `<html_url>/api/v3`.
    #[serde(default)]
    pub api_url: Option<String>,
}

/// Connect a GitHub App that already exists.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ConnectGithubApp {
    pub name: String,
    pub app_id: u64,
    /// The app's PEM private key.
    pub private_key: String,
    /// Defaults to the app's only installation.
    #[serde(default)]
    pub installation_id: Option<u64>,
    /// Needed for push webhooks.
    #[serde(default)]
    pub webhook_secret: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
}

impl fmt::Debug for ConnectGithubApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectGithubApp")
            .field("name", &self.name)
            .field("app_id", &self.app_id)
            .field("installation_id", &self.installation_id)
            .field("html_url", &self.html_url)
            .finish_non_exhaustive()
    }
}

/// Connect with a personal access token, for single repositories without webhooks.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ConnectGithubToken {
    pub name: String,
    pub token: String,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
}

impl fmt::Debug for ConnectGithubToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectGithubToken")
            .field("name", &self.name)
            .field("html_url", &self.html_url)
            .finish_non_exhaustive()
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateStorage {
    pub name: String,
    pub endpoint: String,
    #[serde(default = "default_region")]
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub path_prefix: String,
    #[serde(default)]
    pub path_style: bool,
    pub credentials_secret_id: Id,
}

fn default_region() -> String {
    "us-east-1".into()
}

/// Fields left as `None` keep their value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateStorage {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub bucket: Option<String>,
    pub path_prefix: Option<String>,
    pub path_style: Option<bool>,
    pub credentials_secret_id: Option<Id>,
}

/// Body of a manual database backup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateBackup {
    /// Storage connection to write to; `None` keeps the backup in the data directory.
    pub storage_id: Option<Id>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CreateInstanceBackup {
    /// Storage connection to write to; `None` keeps the archive in the data directory.
    #[serde(default)]
    pub storage_id: Option<Id>,
    /// Encrypts the archive. Required, at least 12 characters, and never stored.
    pub passphrase: String,
}

impl fmt::Debug for CreateInstanceBackup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateInstanceBackup")
            .field("storage_id", &self.storage_id)
            .field("passphrase", &"redacted")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UpdateInstanceBackupSchedule {
    pub enabled: bool,
    pub hour_utc: u8,
    pub keep: u32,
    #[serde(default)]
    pub storage_id: Option<Id>,
    /// Passphrase for scheduled runs, stored encrypted under the master key. Required when no
    /// passphrase is stored yet and `enabled` is true; `None` keeps the stored one.
    #[serde(default)]
    pub passphrase: Option<String>,
}

impl fmt::Debug for UpdateInstanceBackupSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateInstanceBackupSchedule")
            .field("enabled", &self.enabled)
            .field("hour_utc", &self.hour_utc)
            .field("keep", &self.keep)
            .field("storage_id", &self.storage_id)
            .field("passphrase", &self.passphrase.as_ref().map(|_| "redacted"))
            .finish()
    }
}

/// Shortest passphrase an instance backup accepts.
pub const MIN_PASSPHRASE_LEN: usize = 12;
