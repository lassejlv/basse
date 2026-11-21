//! Shared application state handed to every handler and background task.

use crate::backup_store::BackupStore;
use crate::cloudflare::CloudflareIps;
use crate::config::{Config, DataDir};
use crate::database;
use crate::db::Db;
use crate::deploy::Deploys;
use crate::docker::{DockerPool, InstanceRoute, ProxyConfig};
use crate::domain::{NetProbe, Probe, Report};
use crate::error::AppError;
use crate::notify::Notifier;
use crate::secrets::{MasterKey, Secrets};
use crate::stats::Stats;
use anyhow::{Context as _, Result, bail};
use bollard::Docker;
use chrono::Utc;
use common::{
    Activity, ActivityLevel, BackupStorage, GitRepo, Id, Server, Service, StorageConnection,
    Timestamp, VersionCheck,
};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::time::{Instant, SystemTime};
use subtle::ConstantTimeEq as _;

pub struct AppState {
    pub db: Db,
    pub docker: DockerPool,
    pub dirs: DataDir,
    pub deploys: Deploys,
    pub stats: Stats,
    pub db_locks: database::Locks,
    pub secrets: Secrets,
    pub notifier: Notifier,
    /// Held while an instance backup runs; there is at most one at a time.
    pub instance_backup_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// Client used for health checks; does not follow redirects so 3xx counts as healthy.
    pub http: reqwest::Client,
    /// Address the API listens on.
    pub listen: String,
    config: ConfigStore,
    /// Latest release check, kept for an hour.
    pub latest_release: Mutex<Option<(Instant, VersionCheck)>>,
    /// DNS and network access for the instance domain checks.
    pub probe: Box<dyn Probe>,
    /// Results of the last instance domain check, for the domain they ran against.
    pub domain_checks: Mutex<Option<(String, Report, Timestamp)>>,
    /// Tokens of the domain checks running now, answered on the verification route.
    pub verify_nonces: Mutex<Vec<String>>,
    /// Cloudflare's edge ranges, refreshed daily.
    pub cloudflare: CloudflareIps,
    /// This server's public addresses as the internet sees them, kept for ten minutes.
    public_ips: Mutex<Option<(Instant, PublicIps)>>,
    /// Repositories each git source can see, by source id, with when they were listed.
    repo_cache: Mutex<HashMap<Id, (Instant, Vec<GitRepo>)>>,
}

/// Public IPv4 and IPv6 address.
pub type PublicIps = (Option<IpAddr>, Option<IpAddr>);

const PUBLIC_IP_CACHE: std::time::Duration = std::time::Duration::from_secs(600);

/// `config.toml`, reloaded whenever the file changes so that `token rotate` on the command line
/// takes effect without a restart, and written through `update` by the settings API.
struct ConfigStore {
    path: PathBuf,
    cached: RwLock<(Option<SystemTime>, Config)>,
    writing: Mutex<()>,
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl ConfigStore {
    fn current(&self) -> Config {
        let mtime = mtime(&self.path);
        {
            let cached = self.cached.read().unwrap_or_else(|p| p.into_inner());
            if cached.0 == mtime {
                return cached.1.clone();
            }
        }
        let mut cached = self.cached.write().unwrap_or_else(|p| p.into_inner());
        match Config::load_path(&self.path).and_then(|c| c.token_hash().map(|_| c)) {
            Ok(config) => *cached = (mtime, config),
            Err(e) => tracing::warn!("cannot reload {}: {e:#}", self.path.display()),
        }
        cached.1.clone()
    }

    /// Changes the config on disk and in memory at once, so the next request already sees it.
    fn update<T>(
        &self,
        dirs: &DataDir,
        change: impl FnOnce(&mut Config) -> Result<T, AppError>,
    ) -> Result<(Config, T), AppError> {
        let _writing = self.writing.lock().unwrap_or_else(|p| p.into_inner());
        let mut config = self.current();
        let out = change(&mut config)?;
        config.save(dirs)?;
        *self.cached.write().unwrap_or_else(|p| p.into_inner()) =
            (mtime(&self.path), config.clone());
        Ok((config, out))
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl AppState {
    pub fn new(db: Db, dirs: DataDir, config: &Config, listen: String) -> Result<Self> {
        config.token_hash()?;
        let config = ConfigStore {
            cached: RwLock::new((mtime(&dirs.config()), config.clone())),
            path: dirs.config(),
            writing: Mutex::new(()),
        };
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(3))
            .build()?;
        let secrets = Secrets::new(db.clone(), MasterKey::load(&dirs.master_key())?);
        Ok(Self {
            docker: DockerPool::new(crate::ssh::paths(&dirs.ssh_dir()), secrets.clone()),
            secrets,
            notifier: Notifier::default(),
            instance_backup_lock: Default::default(),
            db,
            dirs,
            deploys: Deploys::default(),
            stats: Stats::default(),
            db_locks: database::Locks::default(),
            http,
            listen,
            config,
            latest_release: Mutex::new(None),
            probe: Box::new(NetProbe::new()?),
            domain_checks: Mutex::new(None),
            verify_nonces: Mutex::default(),
            cloudflare: CloudflareIps::default(),
            public_ips: Mutex::new(None),
            repo_cache: Mutex::default(),
        })
    }

    pub fn repo_cache_get(
        &self,
        source_id: &str,
        max_age: std::time::Duration,
    ) -> Option<Vec<GitRepo>> {
        lock(&self.repo_cache)
            .get(source_id)
            .filter(|(at, _)| at.elapsed() < max_age)
            .map(|(_, repos)| repos.clone())
    }

    pub fn repo_cache_put(&self, source_id: &str, repos: Vec<GitRepo>, at: Instant) {
        lock(&self.repo_cache).insert(source_id.to_string(), (at, repos));
    }

    pub fn repo_cache_forget(&self, source_id: &str) {
        lock(&self.repo_cache).remove(source_id);
    }

    /// The public addresses DNS records should point at: the configured override, else what
    /// an echo service sees, else the address of the interface that routes to the internet.
    pub async fn public_ips(&self) -> PublicIps {
        let place = |ip: IpAddr| match ip {
            IpAddr::V4(_) => (Some(ip), None),
            IpAddr::V6(_) => (None, Some(ip)),
        };
        let configured = self.config().public_ip_override.or_else(|| {
            std::env::var(common::brand::env_var("PUBLIC_IP"))
                .ok()
                .filter(|ip| !ip.trim().is_empty())
        });
        if let Some(ip) = configured.and_then(|ip| ip.trim().parse().ok()) {
            return place(ip);
        }
        let cached = lock(&self.public_ips)
            .filter(|(at, _)| at.elapsed() < PUBLIC_IP_CACHE)
            .map(|(_, ips)| ips);
        if let Some(ips) = cached {
            return ips;
        }
        let (v4, v6) = self.probe.public_ips().await;
        let ips = (
            v4.or_else(|| crate::host::public_ip().and_then(|ip| ip.parse().ok())),
            v6,
        );
        *lock(&self.public_ips) = Some((Instant::now(), ips));
        ips
    }

    /// `https://<domain>` once the instance domain is verified.
    pub fn instance_url(&self) -> Option<String> {
        let config = self.config();
        config
            .domain
            .filter(|_| config.domain_verified_at.is_some())
            .map(|d| format!("https://{d}"))
    }

    /// What the proxy on `server` should run with. Only the local server serves the instance
    /// domain, and only when the API listens beyond loopback.
    pub async fn proxy_config(&self, server: &Server) -> ProxyConfig {
        let ranges = self.cloudflare.get(&self.http).await;
        let route = match (server.kind, self.config().domain) {
            (common::ServerKind::Local, Some(domain)) => {
                match self.listen.parse::<std::net::SocketAddr>() {
                    Ok(listen) => {
                        let proxied = self
                            .probe
                            .resolve(&domain)
                            .await
                            .is_ok_and(|ips| ips.iter().any(|ip| ranges.contains(*ip)));
                        Some(InstanceRoute {
                            domain,
                            port: listen.port(),
                            proxied,
                        })
                    }
                    Err(_) => None,
                }
            }
            _ => None,
        };
        ProxyConfig {
            route,
            trusted: ranges.to_list(),
        }
    }

    /// The daemon's `config.toml` as it is now.
    pub fn config(&self) -> Config {
        self.config.current()
    }

    pub fn update_config<T>(
        &self,
        change: impl FnOnce(&mut Config) -> Result<T, AppError>,
    ) -> Result<(Config, T), AppError> {
        self.config.update(&self.dirs, change)
    }

    /// Constant-time comparison of the presented token's hash with the stored one.
    pub fn token_matches(&self, token: &str) -> bool {
        self.config()
            .token_hash()
            .is_ok_and(|hash| crate::config::hash_token(token).ct_eq(&hash).into())
    }

    pub async fn server(&self, id: &str) -> Result<Server, AppError> {
        self.db
            .server(id)
            .await?
            .ok_or_else(|| AppError::not_found("server", id))
    }

    /// The store backups with this storage live in.
    pub async fn backup_store(
        &self,
        storage: BackupStorage,
        storage_id: Option<&str>,
    ) -> Result<BackupStore> {
        match (storage, storage_id) {
            (BackupStorage::Local, _) => Ok(BackupStore::Local {
                root: self.dirs.backups_dir(),
            }),
            (BackupStorage::S3, Some(id)) => {
                let conn = self
                    .db
                    .storage(id)
                    .await?
                    .with_context(|| format!("storage connection '{id}' no longer exists"))?;
                BackupStore::for_connection(&conn, &self.secrets).await
            }
            (BackupStorage::S3, None) => bail!("an S3 backup without a storage connection"),
        }
    }

    /// Where a new backup with this optional storage connection goes.
    pub async fn storage_target(
        &self,
        storage_id: Option<&str>,
    ) -> Result<(BackupStorage, Option<StorageConnection>), AppError> {
        let Some(id) = storage_id.filter(|id| !id.is_empty()) else {
            return Ok((BackupStorage::Local, None));
        };
        let conn = self
            .db
            .storage(id)
            .await?
            .ok_or_else(|| AppError::not_found("storage connection", id))?;
        Ok((BackupStorage::S3, Some(conn)))
    }

    pub async fn docker_for(&self, server_id: &str) -> Result<(Server, Docker), AppError> {
        let server = self.server(server_id).await?;
        let docker = self.docker.get(&server).await?;
        Ok((server, docker))
    }

    /// Records an activity row; failures are logged, never propagated.
    pub async fn activity(&self, level: ActivityLevel, message: String, detail: Option<String>) {
        self.insert_activity(Activity {
            id: 0,
            at: Utc::now(),
            level,
            service_id: None,
            service_name: None,
            project_id: None,
            message,
            detail,
        })
        .await;
    }

    pub async fn project_activity(&self, project_id: &str, level: ActivityLevel, message: String) {
        self.insert_activity(Activity {
            id: 0,
            at: Utc::now(),
            level,
            service_id: None,
            service_name: None,
            project_id: Some(project_id.to_string()),
            message,
            detail: None,
        })
        .await;
    }

    pub async fn service_activity(
        &self,
        service: &Service,
        level: ActivityLevel,
        message: String,
        detail: Option<String>,
    ) {
        let project_id = self
            .db
            .environment(&service.environment_id)
            .await
            .ok()
            .flatten()
            .map(|e| e.project_id);
        self.insert_activity(Activity {
            id: 0,
            at: Utc::now(),
            level,
            service_id: Some(service.id.clone()),
            service_name: Some(service.name.clone()),
            project_id,
            message,
            detail,
        })
        .await;
    }

    async fn insert_activity(&self, activity: Activity) {
        if let Err(e) = self.db.insert_activity(&activity).await {
            tracing::warn!("cannot record activity '{}': {e:#}", activity.message);
        }
    }
}
