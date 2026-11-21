//! Typed client for the daemon API, plus the local store of daemon contexts.

mod config;

pub use config::{Config, Context};

use anyhow::{Context as _, Result, anyhow};
use common::api::*;
use common::metrics::{MetricsRange, ServiceMetrics};
use common::notify::*;
use common::*;
use futures_util::{Stream, StreamExt};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use std::pin::Pin;

pub type LogStreamBox = Pin<Box<dyn Stream<Item = Result<LogLine>> + Send>>;
/// Raw body chunks of a download.
pub type ByteStreamBox = Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send>>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    pub fn new(url: &str, token: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("{}/{}", brand::CLI_BIN, env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()?;
        Ok(Self {
            http,
            base: format!("{}{}", url.trim_end_matches('/'), API_PREFIX),
            token: token.to_string(),
        })
    }

    pub fn from_context(ctx: &Context) -> Result<Self> {
        Self::new(&ctx.url, &ctx.token)
    }

    /// Daemon URL without the API prefix.
    pub fn url(&self) -> &str {
        self.base.trim_end_matches(API_PREFIX)
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// The same daemon with a new token, e.g. after rotating it.
    pub fn with_token(&self, token: &str) -> Self {
        Self {
            token: token.to_string(),
            ..self.clone()
        }
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
    }

    async fn send(rb: RequestBuilder) -> Result<Response> {
        let res = rb.send().await.map_err(|e| {
            if e.is_connect() {
                anyhow!("cannot reach the daemon: {e}")
            } else {
                anyhow!(e)
            }
        })?;
        if res.status().is_success() {
            return Ok(res);
        }
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        let message = serde_json::from_str::<ApiError>(&body)
            .map(|e| e.error)
            .unwrap_or(body);
        Err(match status {
            StatusCode::UNAUTHORIZED => anyhow!("unauthorized: the token was rejected"),
            _ if message.is_empty() => anyhow!("daemon returned {status}"),
            _ => anyhow!("{message}"),
        })
    }

    async fn json<T: DeserializeOwned>(rb: RequestBuilder) -> Result<T> {
        Self::send(rb)
            .await?
            .json()
            .await
            .context("unexpected response from the daemon")
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Self::json(self.req(Method::GET, path)).await
    }

    async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        Self::json(self.req(Method::POST, path).json(body)).await
    }

    async fn patch<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        Self::json(self.req(Method::PATCH, path).json(body)).await
    }

    async fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Self::json(self.req(Method::POST, path)).await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        Self::send(self.req(Method::DELETE, path)).await.map(drop)
    }

    async fn stream(&self, path: &str) -> Result<LogStreamBox> {
        let res = Self::send(self.req(Method::GET, path)).await?;
        let bytes = res.bytes_stream();
        let lines = futures_util::stream::unfold(
            (bytes, Vec::<u8>::new(), false),
            |(mut bytes, mut buf, mut done)| async move {
                loop {
                    if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line = &line[..line.len() - 1];
                        if line.iter().all(u8::is_ascii_whitespace) {
                            continue;
                        }
                        let item =
                            serde_json::from_slice::<LogLine>(line).map_err(anyhow::Error::from);
                        return Some((item, (bytes, buf, done)));
                    }
                    if done {
                        return None;
                    }
                    match bytes.next().await {
                        Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
                        Some(Err(e)) => return Some((Err(anyhow!(e)), (bytes, buf, true))),
                        None => {
                            done = true;
                            buf.push(b'\n');
                        }
                    }
                }
            },
        );
        Ok(Box::pin(lines))
    }

    // Daemon

    pub async fn info(&self) -> Result<DaemonInfo> {
        self.get("/info").await
    }

    pub async fn tree(&self) -> Result<Tree> {
        self.get("/tree").await
    }

    pub async fn activity(&self, limit: usize) -> Result<Vec<Activity>> {
        self.get(&format!("/activity?limit={limit}")).await
    }

    // Servers

    pub async fn onboarding(&self) -> Result<common::onboarding::Onboarding> {
        self.get("/onboarding").await
    }

    pub async fn update_onboarding(
        &self,
        body: &UpdateOnboarding,
    ) -> Result<common::onboarding::Onboarding> {
        self.patch("/onboarding", body).await
    }

    pub async fn servers(&self) -> Result<Vec<Server>> {
        self.get("/servers").await
    }

    pub async fn create_server(&self, body: &CreateServer) -> Result<Server> {
        self.post("/servers", body).await
    }

    pub async fn delete_server(&self, id: &str) -> Result<()> {
        self.delete(&format!("/servers/{id}")).await
    }

    /// Removes what the daemon put on the server (and the services assigned to it), then forgets it.
    pub async fn purge_server(&self, id: &str, volumes: bool) -> Result<ServerCleanup> {
        Self::json(self.req(
            Method::DELETE,
            &format!("/servers/{id}?purge=true&volumes={volumes}"),
        ))
        .await
    }

    pub async fn check_server(&self, id: &str) -> Result<Server> {
        self.post_empty(&format!("/servers/{id}/check")).await
    }

    pub async fn setup_proxy(&self, id: &str) -> Result<Server> {
        self.post_empty(&format!("/servers/{id}/proxy")).await
    }

    /// Installs Docker on the server over SSH, then checks it. Takes a minute or two.
    pub async fn install_docker(&self, id: &str) -> Result<Server> {
        self.post_empty(&format!("/servers/{id}/docker")).await
    }

    /// Changes the key a server connects with; the daemon reconnects and checks it.
    pub async fn update_server(&self, id: &str, body: &UpdateServer) -> Result<Server> {
        self.patch(&format!("/servers/{id}"), body).await
    }

    // Storage connections

    pub async fn storages(&self) -> Result<Vec<StorageConnection>> {
        self.get("/storage").await
    }

    pub async fn storage(&self, id: &str) -> Result<StorageConnection> {
        self.get(&format!("/storage/{id}")).await
    }

    pub async fn create_storage(&self, body: &CreateStorage) -> Result<StorageConnection> {
        self.post("/storage", body).await
    }

    pub async fn update_storage(
        &self,
        id: &str,
        body: &UpdateStorage,
    ) -> Result<StorageConnection> {
        self.patch(&format!("/storage/{id}"), body).await
    }

    pub async fn delete_storage(&self, id: &str) -> Result<()> {
        self.delete(&format!("/storage/{id}")).await
    }

    /// Writes, reads back and deletes a probe object; the result is also saved on the connection.
    pub async fn test_storage(&self, id: &str) -> Result<StorageTest> {
        self.post_empty(&format!("/storage/{id}/test")).await
    }

    // Instance backups

    pub async fn instance_backups(&self) -> Result<Vec<InstanceBackup>> {
        self.get("/instance/backups").await
    }

    /// Starts an instance backup; poll [`Self::instance_backups`] until it finishes.
    pub async fn create_instance_backup(
        &self,
        body: &CreateInstanceBackup,
    ) -> Result<InstanceBackup> {
        self.post("/instance/backups", body).await
    }

    /// The encrypted archive of a finished instance backup.
    pub async fn download_instance_backup(&self, id: &str) -> Result<ByteStreamBox> {
        let res =
            Self::send(self.req(Method::GET, &format!("/instance/backups/{id}/download"))).await?;
        Ok(Box::pin(res.bytes_stream().map(|chunk| {
            chunk.map(|c| c.to_vec()).map_err(anyhow::Error::from)
        })))
    }

    pub async fn delete_instance_backup(&self, id: &str) -> Result<()> {
        self.delete(&format!("/instance/backups/{id}")).await
    }

    pub async fn instance_backup_schedule(&self) -> Result<InstanceBackupSchedule> {
        self.get("/instance/backups/schedule").await
    }

    pub async fn set_instance_backup_schedule(
        &self,
        body: &UpdateInstanceBackupSchedule,
    ) -> Result<InstanceBackupSchedule> {
        Self::json(
            self.req(Method::PUT, "/instance/backups/schedule")
                .json(body),
        )
        .await
    }

    // Secrets

    pub async fn secrets(&self) -> Result<Vec<Secret>> {
        self.get("/secrets").await
    }

    pub async fn secret(&self, id: &str) -> Result<Secret> {
        self.get(&format!("/secrets/{id}")).await
    }

    pub async fn create_secret(&self, body: &CreateSecret) -> Result<Secret> {
        self.post("/secrets", body).await
    }

    /// Has the daemon generate an ed25519 key; the result carries its public key.
    pub async fn generate_ssh_key(&self, name: &str) -> Result<Secret> {
        self.post(
            "/secrets/generate-ssh-key",
            &GenerateSshKey { name: name.into() },
        )
        .await
    }

    pub async fn update_secret(&self, id: &str, body: &UpdateSecret) -> Result<Secret> {
        self.patch(&format!("/secrets/{id}"), body).await
    }

    pub async fn delete_secret(&self, id: &str) -> Result<()> {
        self.delete(&format!("/secrets/{id}")).await
    }

    /// The decrypted value. The daemon records every reveal in the activity feed.
    pub async fn reveal_secret(&self, id: &str) -> Result<RevealedSecret> {
        self.post_empty(&format!("/secrets/{id}/reveal")).await
    }

    // Projects and environments

    pub async fn projects(&self) -> Result<Vec<Project>> {
        self.get("/projects").await
    }

    pub async fn create_project(&self, body: &CreateProject) -> Result<Project> {
        self.post("/projects", body).await
    }

    pub async fn delete_project(&self, id: &str) -> Result<()> {
        self.delete(&format!("/projects/{id}")).await
    }

    pub async fn create_environment(
        &self,
        project_id: &str,
        body: &CreateEnvironment,
    ) -> Result<Environment> {
        self.post(&format!("/projects/{project_id}/environments"), body)
            .await
    }

    pub async fn delete_environment(&self, id: &str) -> Result<()> {
        self.delete(&format!("/environments/{id}")).await
    }

    // Services

    pub async fn create_service(&self, body: &CreateService) -> Result<ServiceDetail> {
        self.post("/services", body).await
    }

    pub async fn service(&self, id: &str) -> Result<ServiceDetail> {
        self.get(&format!("/services/{id}")).await
    }

    pub async fn update_service(&self, id: &str, body: &UpdateService) -> Result<ServiceDetail> {
        Self::json(
            self.req(Method::PATCH, &format!("/services/{id}"))
                .json(body),
        )
        .await
    }

    pub async fn delete_service(&self, id: &str, volumes: bool) -> Result<()> {
        self.delete(&format!("/services/{id}?volumes={volumes}"))
            .await
    }

    pub async fn deploy(&self, id: &str, trigger: &str) -> Result<Deployment> {
        let body = DeployRequest {
            trigger: Some(trigger.to_string()),
        };
        self.post(&format!("/services/{id}/deploy"), &body).await
    }

    /// Moves a service to another server; follow the returned deployment like a deploy.
    pub async fn move_service(&self, id: &str, body: &MoveService) -> Result<Deployment> {
        self.post(&format!("/services/{id}/move"), body).await
    }

    pub async fn restart(&self, id: &str) -> Result<ServiceDetail> {
        self.post_empty(&format!("/services/{id}/restart")).await
    }

    pub async fn stop(&self, id: &str) -> Result<ServiceDetail> {
        self.post_empty(&format!("/services/{id}/stop")).await
    }

    pub async fn start(&self, id: &str) -> Result<ServiceDetail> {
        self.post_empty(&format!("/services/{id}/start")).await
    }

    /// Container logs; `container` picks one service of a compose build.
    pub async fn service_logs(
        &self,
        id: &str,
        tail: usize,
        follow: bool,
        container: Option<&str>,
    ) -> Result<LogStreamBox> {
        let mut path = format!("/services/{id}/logs?tail={tail}&follow={follow}");
        if let Some(c) = container {
            path.push_str(&format!("&container={c}"));
        }
        self.stream(&path).await
    }

    pub async fn service_stats(&self, id: &str) -> Result<ServiceStats> {
        self.get(&format!("/services/{id}/stats")).await
    }

    /// Metrics over `range`; `container` picks one compose service instead of the sum.
    pub async fn service_metrics(
        &self,
        id: &str,
        range: MetricsRange,
        container: Option<&str>,
    ) -> Result<ServiceMetrics> {
        let mut path = format!("/services/{id}/metrics?range={range}");
        if let Some(c) = container {
            path.push_str(&format!("&container={c}"));
        }
        self.get(&path).await
    }

    /// Metrics of one replica over `range`.
    pub async fn replica_metrics(
        &self,
        id: &str,
        range: MetricsRange,
        replica: u16,
    ) -> Result<ServiceMetrics> {
        self.get(&format!(
            "/services/{id}/metrics?range={range}&replica={replica}"
        ))
        .await
    }

    /// Logs of one replica.
    pub async fn replica_logs(
        &self,
        id: &str,
        replica: u16,
        tail: usize,
        follow: bool,
    ) -> Result<LogStreamBox> {
        self.stream(&format!(
            "/services/{id}/logs?tail={tail}&follow={follow}&replica={replica}"
        ))
        .await
    }

    /// Sets the replica count; replicas are added or drained in the background.
    pub async fn scale(&self, id: &str, replicas: u16) -> Result<ServiceDetail> {
        self.post(&format!("/services/{id}/scale"), &ScaleRequest { replicas })
            .await
    }

    pub async fn set_autoscale(&self, id: &str, autoscale: &Autoscale) -> Result<ServiceDetail> {
        Self::json(
            self.req(Method::PUT, &format!("/services/{id}/autoscale"))
                .json(autoscale),
        )
        .await
    }

    /// Room for more replicas on the service's server.
    pub async fn scale_capacity(&self, id: &str) -> Result<ScaleCapacity> {
        self.get(&format!("/services/{id}/capacity")).await
    }

    pub async fn set_env(&self, id: &str, vars: &[EnvVar]) -> Result<Vec<EnvVar>> {
        Self::json(
            self.req(Method::PUT, &format!("/services/{id}/env"))
                .json(vars),
        )
        .await
    }

    pub async fn add_domain(&self, service_id: &str, body: &CreateDomain) -> Result<Domain> {
        self.post(&format!("/services/{service_id}/domains"), body)
            .await
    }

    /// Sets a custom error page of a service, or of the instance with `service_id: None`.
    pub async fn set_error_page(
        &self,
        service_id: Option<&str>,
        kind: ErrorPageKind,
        body: &SetErrorPage,
    ) -> Result<CustomErrorPage> {
        Self::json(
            self.req(Method::PUT, &error_page_path(service_id, kind))
                .json(body),
        )
        .await
    }

    pub async fn delete_error_page(
        &self,
        service_id: Option<&str>,
        kind: ErrorPageKind,
    ) -> Result<()> {
        self.delete(&error_page_path(service_id, kind)).await
    }

    /// The instance-wide default error pages.
    pub async fn instance_error_pages(&self) -> Result<ErrorPages> {
        self.get("/settings/error-pages").await
    }

    pub async fn delete_domain(&self, id: &str) -> Result<()> {
        self.delete(&format!("/domains/{id}")).await
    }

    pub async fn add_volume(&self, service_id: &str, body: &CreateVolume) -> Result<Volume> {
        self.post(&format!("/services/{service_id}/volumes"), body)
            .await
    }

    pub async fn delete_volume(&self, id: &str, purge: bool) -> Result<()> {
        self.delete(&format!("/volumes/{id}?purge={purge}")).await
    }

    // Settings

    pub async fn settings(&self) -> Result<InstanceSettings> {
        self.get("/settings").await
    }

    pub async fn update_settings(&self, body: &UpdateSettings) -> Result<InstanceSettings> {
        Self::json(self.req(Method::PATCH, "/settings").json(body)).await
    }

    /// Issues a new token; the old one, and so this client, stop working at once.
    pub async fn rotate_token(&self) -> Result<RotatedToken> {
        self.post_empty("/settings/token/rotate").await
    }

    pub async fn restart_proxy(&self) -> Result<Server> {
        self.post_empty("/settings/proxy/restart").await
    }

    pub async fn prune(&self) -> Result<PruneResult> {
        self.post_empty("/settings/prune").await
    }

    pub async fn version_check(&self) -> Result<VersionCheck> {
        self.get("/settings/version").await
    }

    pub async fn update(&self, body: &UpdateRequest) -> Result<UpdateResult> {
        self.post("/settings/update", body).await
    }

    pub async fn instance_domain(&self) -> Result<InstanceDomain> {
        self.get("/settings/domain").await
    }

    /// Routes `domain` to the API and runs the checks once.
    pub async fn set_instance_domain(&self, domain: &str) -> Result<InstanceDomain> {
        let body = SetInstanceDomain {
            domain: domain.to_string(),
        };
        Self::json(self.req(Method::PUT, "/settings/domain").json(&body)).await
    }

    pub async fn check_instance_domain(&self) -> Result<InstanceDomain> {
        self.post_empty("/settings/domain/check").await
    }

    pub async fn remove_instance_domain(&self) -> Result<InstanceDomain> {
        Self::json(self.req(Method::DELETE, "/settings/domain")).await
    }

    // Git sources

    pub async fn sources(&self) -> Result<Vec<GitSource>> {
        self.get("/sources").await
    }

    pub async fn source(&self, id: &str) -> Result<GitSource> {
        self.get(&format!("/sources/{id}")).await
    }

    /// Starts the GitHub App manifest flow; open the returned `setup_url` in a browser.
    pub async fn create_github_app(&self, body: &CreateGithubApp) -> Result<GithubAppSetup> {
        self.post("/sources/github", body).await
    }

    pub async fn connect_github_app(&self, body: &ConnectGithubApp) -> Result<GitSource> {
        self.post("/sources/github/connect", body).await
    }

    pub async fn connect_github_token(&self, body: &ConnectGithubToken) -> Result<GitSource> {
        self.post("/sources/github/token", body).await
    }

    pub async fn check_source(&self, id: &str) -> Result<GitSource> {
        self.post_empty(&format!("/sources/{id}/check")).await
    }

    pub async fn delete_source(&self, id: &str) -> Result<()> {
        self.delete(&format!("/sources/{id}")).await
    }

    /// Repositories the source can clone, filtered by `query` on `owner/name`.
    pub async fn source_repos(
        &self,
        id: &str,
        query: &str,
        page: u32,
        per_page: u32,
    ) -> Result<RepoPage> {
        let rb = self
            .req(Method::GET, &format!("/sources/{id}/repos"))
            .query(&[
                ("q", query.to_string()),
                ("page", page.to_string()),
                ("per_page", per_page.to_string()),
            ]);
        Self::json(rb).await
    }

    /// The build method a branch would get for a service in `root` (empty for the repository
    /// root): Dockerfile, else compose file, else Railpack.
    pub async fn detect_build(
        &self,
        id: &str,
        repo: &str,
        branch: &str,
        root: &str,
    ) -> Result<BuildDetection> {
        let rb = self
            .req(Method::GET, &format!("/sources/{id}/detect"))
            .query(&[("repo", repo), ("branch", branch), ("root", root)]);
        Self::json(rb).await
    }

    /// Names of the directories directly under `path` on a branch (empty for the repository
    /// root), for picking a service's root directory.
    pub async fn source_dirs(
        &self,
        id: &str,
        repo: &str,
        branch: &str,
        path: &str,
    ) -> Result<Vec<String>> {
        let rb = self
            .req(Method::GET, &format!("/sources/{id}/dirs"))
            .query(&[("repo", repo), ("branch", branch), ("path", path)]);
        Self::json(rb).await
    }

    pub async fn source_branches(&self, id: &str, repo: &str) -> Result<Vec<GitBranch>> {
        let rb = self
            .req(Method::GET, &format!("/sources/{id}/branches"))
            .query(&[("repo", repo)]);
        Self::json(rb).await
    }

    // Databases

    /// Publishes a database on a server port, or makes it internal-only with `None`.
    pub async fn update_database(
        &self,
        service_id: &str,
        body: &UpdateDatabase,
    ) -> Result<DatabaseInfo> {
        Self::json(
            self.req(Method::PATCH, &format!("/services/{service_id}/database"))
                .json(body),
        )
        .await
    }

    pub async fn rotate_database_password(&self, service_id: &str) -> Result<DatabaseInfo> {
        self.post_empty(&format!("/services/{service_id}/database/rotate-password"))
            .await
    }

    /// Backups of a service, newest first.
    pub async fn backups(&self, service_id: &str) -> Result<Vec<Backup>> {
        self.get(&format!("/services/{service_id}/backups")).await
    }

    /// Starts a backup into a storage connection, or the data directory when `storage_id` is
    /// `None`; poll [`Self::backup`] until it is no longer running.
    pub async fn create_backup(
        &self,
        service_id: &str,
        storage_id: Option<&str>,
    ) -> Result<Backup> {
        self.post(
            &format!("/services/{service_id}/backups"),
            &CreateBackup {
                storage_id: storage_id.map(str::to_string),
            },
        )
        .await
    }

    pub async fn backup(&self, id: &str) -> Result<Backup> {
        self.get(&format!("/backups/{id}")).await
    }

    /// The gzip file of a finished backup.
    pub async fn download_backup(&self, id: &str) -> Result<ByteStreamBox> {
        let res = Self::send(self.req(Method::GET, &format!("/backups/{id}/download"))).await?;
        Ok(Box::pin(res.bytes_stream().map(|chunk| {
            chunk.map(|c| c.to_vec()).map_err(anyhow::Error::from)
        })))
    }

    /// Restores a backup into its database; returns when the restore has finished.
    pub async fn restore_backup(&self, id: &str) -> Result<Backup> {
        self.post_empty(&format!("/backups/{id}/restore")).await
    }

    pub async fn delete_backup(&self, id: &str) -> Result<()> {
        self.delete(&format!("/backups/{id}")).await
    }

    pub async fn backup_schedule(&self, service_id: &str) -> Result<BackupSchedule> {
        self.get(&format!("/services/{service_id}/backups/schedule"))
            .await
    }

    pub async fn set_backup_schedule(
        &self,
        service_id: &str,
        schedule: &BackupSchedule,
    ) -> Result<BackupSchedule> {
        Self::json(
            self.req(
                Method::PUT,
                &format!("/services/{service_id}/backups/schedule"),
            )
            .json(schedule),
        )
        .await
    }

    // Deployments

    pub async fn deployments(&self, service_id: &str, limit: usize) -> Result<Vec<Deployment>> {
        self.get(&format!("/services/{service_id}/deployments?limit={limit}"))
            .await
    }

    pub async fn deployment(&self, id: &str) -> Result<Deployment> {
        self.get(&format!("/deployments/{id}")).await
    }

    pub async fn deployment_logs(&self, id: &str, follow: bool) -> Result<LogStreamBox> {
        self.stream(&format!("/deployments/{id}/logs?follow={follow}"))
            .await
    }

    pub async fn cancel_deployment(&self, id: &str) -> Result<Deployment> {
        self.post_empty(&format!("/deployments/{id}/cancel")).await
    }

    pub async fn rollback(&self, id: &str) -> Result<Deployment> {
        self.post_empty(&format!("/deployments/{id}/rollback"))
            .await
    }

    pub async fn notify_catalog(&self) -> Result<NotifyCatalog> {
        self.get("/notify/catalog").await
    }

    pub async fn notify_channels(&self) -> Result<Vec<NotifyChannel>> {
        self.get("/notify/channels").await
    }

    pub async fn notify_channel(&self, id: &str) -> Result<NotifyChannel> {
        self.get(&format!("/notify/channels/{id}")).await
    }

    pub async fn create_notify_channel(&self, body: &CreateNotifyChannel) -> Result<NotifyChannel> {
        self.post("/notify/channels", body).await
    }

    pub async fn update_notify_channel(
        &self,
        id: &str,
        body: &UpdateNotifyChannel,
    ) -> Result<NotifyChannel> {
        self.patch(&format!("/notify/channels/{id}"), body).await
    }

    pub async fn delete_notify_channel(&self, id: &str) -> Result<()> {
        self.delete(&format!("/notify/channels/{id}")).await
    }

    /// Sends a test message and returns its delivery once it has been attempted.
    pub async fn test_notify_channel(&self, id: &str) -> Result<NotifyDelivery> {
        self.post_empty(&format!("/notify/channels/{id}/test"))
            .await
    }

    pub async fn notify_rules(&self) -> Result<Vec<NotifyRule>> {
        self.get("/notify/rules").await
    }

    pub async fn create_notify_rule(&self, body: &CreateNotifyRule) -> Result<NotifyRule> {
        self.post("/notify/rules", body).await
    }

    pub async fn update_notify_rule(
        &self,
        id: &str,
        body: &UpdateNotifyRule,
    ) -> Result<NotifyRule> {
        self.patch(&format!("/notify/rules/{id}"), body).await
    }

    pub async fn delete_notify_rule(&self, id: &str) -> Result<()> {
        self.delete(&format!("/notify/rules/{id}")).await
    }

    /// The newest deliveries first, optionally only those of one channel or rule.
    pub async fn notify_deliveries(
        &self,
        limit: usize,
        channel_id: Option<&str>,
        rule_id: Option<&str>,
    ) -> Result<Vec<NotifyDelivery>> {
        let mut path = format!("/notify/deliveries?limit={limit}");
        if let Some(id) = channel_id {
            path.push_str(&format!("&channel_id={id}"));
        }
        if let Some(id) = rule_id {
            path.push_str(&format!("&rule_id={id}"));
        }
        self.get(&path).await
    }
}

fn error_page_path(service_id: Option<&str>, kind: ErrorPageKind) -> String {
    match service_id {
        Some(id) => format!("/services/{id}/error-pages/{kind}"),
        None => format!("/settings/error-pages/{kind}"),
    }
}
