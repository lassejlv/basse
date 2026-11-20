//! Shared application state handed to every handler and background task.

use crate::backup_store::BackupStore;
use crate::config::{Config, DataDir};
use crate::database;
use crate::db::Db;
use crate::deploy::Deploys;
use crate::docker::DockerPool;
use crate::error::AppError;
use crate::stats::Stats;
use anyhow::Result;
use bollard::Docker;
use chrono::Utc;
use common::{Activity, ActivityLevel, BackupStorage, Server, Service, VersionCheck};
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
    /// Client used for health checks; does not follow redirects so 3xx counts as healthy.
    pub http: reqwest::Client,
    /// Address the API listens on.
    pub listen: String,
    config: ConfigStore,
    /// Latest release check, kept for an hour.
    pub latest_release: Mutex<Option<(Instant, VersionCheck)>>,
}

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
        Ok(Self {
            docker: DockerPool::new(crate::ssh::paths(&dirs.ssh_dir())),
            db,
            dirs,
            deploys: Deploys::default(),
            stats: Stats::default(),
            db_locks: database::Locks::default(),
            http,
            listen,
            config,
            latest_release: Mutex::new(None),
        })
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

    pub fn server(&self, id: &str) -> Result<Server, AppError> {
        self.db
            .server(id)?
            .ok_or_else(|| AppError::not_found("server", id))
    }

    /// The store backups with this storage live in.
    pub fn backup_store(&self, storage: BackupStorage) -> BackupStore {
        match storage {
            BackupStorage::Local => BackupStore::Local {
                root: self.dirs.backups_dir(),
            },
        }
    }

    pub async fn docker_for(&self, server_id: &str) -> Result<(Server, Docker), AppError> {
        let server = self.server(server_id)?;
        let docker = self.docker.get(&server).await?;
        Ok((server, docker))
    }

    /// Records an activity row; failures are logged, never propagated.
    pub fn activity(&self, level: ActivityLevel, message: String, detail: Option<String>) {
        self.insert_activity(Activity {
            id: 0,
            at: Utc::now(),
            level,
            service_id: None,
            service_name: None,
            project_id: None,
            message,
            detail,
        });
    }

    pub fn project_activity(&self, project_id: &str, level: ActivityLevel, message: String) {
        self.insert_activity(Activity {
            id: 0,
            at: Utc::now(),
            level,
            service_id: None,
            service_name: None,
            project_id: Some(project_id.to_string()),
            message,
            detail: None,
        });
    }

    pub fn service_activity(
        &self,
        service: &Service,
        level: ActivityLevel,
        message: String,
        detail: Option<String>,
    ) {
        let project_id = self
            .db
            .environment(&service.environment_id)
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
        });
    }

    fn insert_activity(&self, activity: Activity) {
        if let Err(e) = self.db.insert_activity(&activity) {
            tracing::warn!("cannot record activity '{}': {e:#}", activity.message);
        }
    }
}
