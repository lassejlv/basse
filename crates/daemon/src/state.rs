//! Shared application state handed to every handler and background task.

use crate::config::{Config, DataDir};
use crate::db::Db;
use crate::deploy::Deploys;
use crate::docker::DockerPool;
use crate::error::AppError;
use crate::stats::Stats;
use anyhow::Result;
use bollard::Docker;
use chrono::Utc;
use common::{Activity, ActivityLevel, Server, Service};
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::SystemTime;
use subtle::ConstantTimeEq as _;

pub struct AppState {
    pub db: Db,
    pub docker: DockerPool,
    pub dirs: DataDir,
    pub deploys: Deploys,
    pub stats: Stats,
    /// Client used for health checks; does not follow redirects so 3xx counts as healthy.
    pub http: reqwest::Client,
    token: TokenStore,
}

/// The token hash from `config.toml`, reloaded whenever the file changes so that
/// `token rotate` takes effect without a restart.
struct TokenStore {
    path: PathBuf,
    cached: RwLock<(Option<SystemTime>, [u8; 32])>,
}

impl TokenStore {
    fn current(&self) -> [u8; 32] {
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        {
            let cached = self.cached.read().unwrap_or_else(|p| p.into_inner());
            if cached.0 == mtime {
                return cached.1;
            }
        }
        let mut cached = self.cached.write().unwrap_or_else(|p| p.into_inner());
        match Config::load_path(&self.path).and_then(|c| c.token_hash()) {
            Ok(hash) => *cached = (mtime, hash),
            Err(e) => tracing::warn!("cannot reload the API token: {e:#}"),
        }
        cached.1
    }
}

impl AppState {
    pub fn new(db: Db, dirs: DataDir, config: &Config) -> Result<Self> {
        let token = TokenStore {
            cached: RwLock::new((
                std::fs::metadata(dirs.config())
                    .and_then(|m| m.modified())
                    .ok(),
                config.token_hash()?,
            )),
            path: dirs.config(),
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
            http,
            token,
        })
    }

    /// Constant-time comparison of the presented token's hash with the stored one.
    pub fn token_matches(&self, token: &str) -> bool {
        crate::config::hash_token(token)
            .ct_eq(&self.token.current())
            .into()
    }

    pub fn server(&self, id: &str) -> Result<Server, AppError> {
        self.db
            .server(id)?
            .ok_or_else(|| AppError::not_found("server", id))
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
