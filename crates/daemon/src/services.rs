//! Service lifecycle: validation, creation, computed state, teardown and power actions.

use crate::database::{self, Credentials, generate_password};
use crate::db::{NewService, ServiceRecord, new_id};
use crate::deploy;
use crate::docker::{self, RunState};
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::notify::Event;
use crate::state::AppState;
use bollard::query_parameters::StopContainerOptions;
use chrono::Utc;
use common::api::{CreateService, UpdateService, VolumeSpec};
use common::notify::NotifyEvent;
use common::*;
use futures_util::future::join_all;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Read paths give up on an unreachable server after this long and report it as degraded.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// State of a service from its stored facts and its live container.
pub fn compute_state(rec: &ServiceRecord, run: Option<RunState>, reachable: bool) -> ServiceState {
    match rec.busy {
        Some(DeploymentStatus::Starting) => return ServiceState::Deploying,
        Some(_) => return ServiceState::Building,
        None => {}
    }
    if rec.live_container.is_none() {
        return if rec.stopped {
            ServiceState::Stopped
        } else if rec.ever_failed {
            ServiceState::Failed
        } else {
            ServiceState::New
        };
    }
    if !reachable {
        return ServiceState::Degraded;
    }
    match run {
        Some(RunState::Running) => ServiceState::Running,
        Some(RunState::Restarting) => ServiceState::Degraded,
        _ if rec.stopped => ServiceState::Stopped,
        Some(RunState::Exited { code: Some(0) }) => ServiceState::Stopped,
        _ => ServiceState::Failed,
    }
}

/// Fills `state` on every service with one container listing per server.
pub async fn with_states(state: &AppState, recs: Vec<ServiceRecord>) -> Vec<Service> {
    let servers: HashSet<&str> = recs
        .iter()
        .filter(|r| r.live_container.is_some() && r.busy.is_none())
        .map(|r| r.service.server_id.as_str())
        .collect();
    let listings = join_all(servers.into_iter().map(|server_id| async move {
        let listing = async {
            let (_, docker) = state.docker_for(server_id).await.ok()?;
            docker::list_managed(&docker).await.ok()
        };
        let containers = tokio::time::timeout(LOOKUP_TIMEOUT, listing)
            .await
            .ok()
            .flatten();
        let runs = containers.map(|cs| {
            cs.iter()
                .filter_map(|c| Some((c.id.clone()?, RunState::from_summary(c))))
                .collect::<HashMap<_, _>>()
        });
        (server_id.to_string(), runs)
    }))
    .await;
    let listings: HashMap<String, Option<HashMap<String, RunState>>> =
        listings.into_iter().collect();
    recs.into_iter()
        .map(|rec| {
            let listing = listings.get(&rec.service.server_id);
            let reachable = !matches!(listing, Some(None));
            let run = rec
                .live_container
                .as_ref()
                .and_then(|id| listing?.as_ref()?.get(id).copied());
            let mut service = rec.service.clone();
            service.state = compute_state(&rec, run, reachable);
            service
        })
        .collect()
}

pub async fn record(state: &AppState, id: &str) -> ApiResult<ServiceRecord> {
    state
        .db
        .service(id)
        .await?
        .ok_or_else(|| AppError::not_found("service", id))
}

pub async fn detail(state: &AppState, id: &str) -> ApiResult<ServiceDetail> {
    let rec = record(state, id).await?;
    let db = &state.db;
    let environment = db
        .environment(&rec.service.environment_id)
        .await?
        .ok_or_else(|| AppError::not_found("environment", &rec.service.environment_id))?;
    let project = db
        .project(&environment.project_id)
        .await?
        .ok_or_else(|| AppError::not_found("project", &environment.project_id))?;
    let server = state.server(&rec.service.server_id).await?;
    let lookup = async {
        let Some(container_id) = &rec.live_container else {
            return Ok(None);
        };
        let docker = state.docker.get(&server).await?;
        docker::inspect(&docker, container_id).await
    };
    let (container, run, reachable) = match tokio::time::timeout(LOOKUP_TIMEOUT, lookup).await {
        Ok(Ok(Some(c))) => (
            Some(docker::container_info(&c)),
            Some(RunState::from_inspect(&c)),
            true,
        ),
        Ok(Ok(None)) => (None, None, true),
        Ok(Err(_)) | Err(_) => (None, None, false),
    };
    let mut service = rec.service.clone();
    service.state = compute_state(&rec, run, reachable);
    let env = db.env(id).await?;
    let database = database::info(&service, &server, &env, db.public_port(id).await?);
    Ok(ServiceDetail {
        service,
        project,
        environment,
        server,
        env,
        domains: db.domains(id).await?,
        volumes: db.volumes(id).await?,
        container,
        database,
    })
}

pub async fn create(state: &AppState, body: CreateService) -> ApiResult<ServiceRecord> {
    let db = &state.db;
    validate_name(&body.name).map_err(AppError::BadRequest)?;
    let env = db
        .environment(&body.environment_id)
        .await?
        .ok_or_else(|| AppError::not_found("environment", &body.environment_id))?;
    let server = match &body.server_id {
        Some(id) => state.server(id).await?,
        None => db
            .local_server()
            .await?
            .ok_or_else(|| AppError::BadRequest("no local server is registered".into()))?,
    };
    if db.service_name_taken(&env.id, &body.name, None).await? {
        conflict!(
            "a service named '{}' already exists in this environment",
            body.name
        );
    }
    let source = validate_source(body.source)?;
    if body.port == Some(0) {
        bad_request!("port must be between 1 and 65535");
    }
    let mut env_vars = body.env;
    validate_env(&env_vars)?;
    let mut volume_specs = body.volumes;
    let mut port = body.port;
    let mut command = non_empty(body.command);
    if let Source::Database { kind, version } = &source {
        port = port.or(Some(kind.port()));
        if !volume_specs.iter().any(|v| v.name == "data") {
            volume_specs.push(VolumeSpec {
                name: "data".into(),
                mount_path: kind.data_path(version).into(),
            });
        }
        for var in database_env(*kind, &body.name) {
            if !env_vars.iter().any(|v| v.key == var.key) {
                env_vars.push(var);
            }
        }
        if *kind == DatabaseKind::Redis && command.is_none() {
            command =
                Some("exec redis-server --requirepass \"$REDIS_PASSWORD\" --appendonly yes".into());
        }
    }
    let service = Service {
        id: new_id(),
        environment_id: env.id.clone(),
        server_id: server.id.clone(),
        name: body.name,
        source,
        port,
        command,
        restart: body.restart,
        health_path: validate_health_path(body.health_path)?,
        created_at: Utc::now(),
        state: ServiceState::New,
        primary_domain: None,
        last_deployed_at: None,
        live_deployment_id: None,
    };
    let mut seen = HashSet::new();
    let mut domains = Vec::new();
    for host in body.domains {
        let host = validate_host(&host)?;
        if !seen.insert(host.clone()) {
            bad_request!("domain '{host}' is listed twice");
        }
        if db.domain_taken(&host).await? {
            conflict!("domain '{host}' is already in use");
        }
        domains.push(Domain {
            id: new_id(),
            service_id: service.id.clone(),
            host,
            port: None,
            tls: true,
            created_at: Utc::now(),
        });
    }
    let mut volumes: Vec<Volume> = Vec::new();
    for spec in volume_specs {
        validate_volume(&spec.name, &spec.mount_path)?;
        if volumes.iter().any(|v| v.name == spec.name) {
            bad_request!("volume '{}' is listed twice", spec.name);
        }
        volumes.push(Volume {
            id: new_id(),
            service_id: service.id.clone(),
            name: spec.name,
            mount_path: spec.mount_path,
            created_at: Utc::now(),
        });
    }
    db.insert_service(&NewService {
        service: &service,
        env: &env_vars,
        domains: &domains,
        volumes: &volumes,
    })
    .await?;
    state
        .service_activity(
            &service,
            ActivityLevel::Info,
            "Service created".into(),
            Some(service.source.label()),
        )
        .await;
    if let Source::Database { kind, version } = &service.source {
        state
            .notify_service(
                &service,
                Event::new(NotifyEvent::DatabaseCreated)
                    .detail(format!("{kind} {version}"))
                    .hint(format!("{} db info {}", brand::CLI_BIN, service.name)),
            )
            .await;
    }
    record(state, &service.id).await
}

pub async fn update(state: &AppState, id: &str, body: UpdateService) -> ApiResult<()> {
    let rec = record(state, id).await?;
    let mut s = rec.service;
    if let Some(name) = body.name {
        validate_name(&name).map_err(AppError::BadRequest)?;
        if state
            .db
            .service_name_taken(&s.environment_id, &name, Some(&s.id))
            .await?
        {
            conflict!("a service named '{name}' already exists in this environment");
        }
        s.name = name;
    }
    if let Some(source) = body.source {
        s.source = validate_source(source)?;
    }
    if let Some(port) = body.port {
        s.port = (port != 0).then_some(port);
    }
    if let Some(command) = body.command {
        s.command = non_empty(Some(command));
    }
    if let Some(restart) = body.restart {
        s.restart = restart;
    }
    if let Some(path) = body.health_path {
        s.health_path = validate_health_path(Some(path))?;
    }
    if let Some(server_id) = body.server_id {
        s.server_id = state.server(&server_id).await?.id;
    }
    state.db.update_service(&s).await?;
    Ok(())
}

/// Cancels deployments, removes containers everywhere and deletes the service.
pub async fn delete(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    purge_volumes: bool,
) -> ApiResult<()> {
    let svc = &rec.service;
    for d in state.db.active_deployments(&svc.id).await? {
        if let Err(e) = deploy::cancel(state, &d.id).await {
            tracing::warn!("cannot cancel deployment {}: {e:?}", d.id);
        }
    }
    let service_lock = state.deploys.lock_for(&svc.id);
    let _guard = service_lock.lock().await;
    let volumes = state.db.volumes(&svc.id).await?;
    for server_id in state.db.deployment_servers(&svc.id).await? {
        let Ok((_, docker)) = state.docker_for(&server_id).await else {
            continue;
        };
        if let Err(e) = docker::remove_service_containers(&docker, &svc.id, None).await {
            tracing::warn!("cannot remove containers of {}: {e:#}", svc.name);
        }
        if purge_volumes {
            for v in &volumes {
                if let Err(e) =
                    docker::remove_volume(&docker, &docker::volume_name(&svc.id, &v.name)).await
                {
                    tracing::warn!("cannot remove volume {}: {e:#}", v.name);
                }
            }
        }
    }
    for d in state.db.deployments(&svc.id, usize::MAX).await? {
        let _ = std::fs::remove_file(state.dirs.deployment_log(&d.id));
    }
    database::delete_backup_files(state, &svc.id).await;
    state.db.delete_service(&svc.id).await?;
    state.deploys.forget_service(&svc.id);
    state.db_locks.forget_service(&svc.id);
    state.stats.forget_service(&svc.id);
    state
        .service_activity(svc, ActivityLevel::Info, "Service deleted".into(), None)
        .await;
    Ok(())
}

/// Deletes every service of an environment and its network. Best effort on Docker errors.
pub async fn delete_environment_services(
    state: &Arc<AppState>,
    env: &Environment,
) -> ApiResult<()> {
    let recs = state.db.services_in_environment(&env.id).await?;
    let mut servers = HashSet::new();
    for rec in &recs {
        servers.extend(state.db.deployment_servers(&rec.service.id).await?);
        delete(state, rec, false).await?;
    }
    for server_id in servers {
        if let Ok((_, docker)) = state.docker_for(&server_id).await
            && let Err(e) = docker::remove_network(&docker, &docker::env_network(&env.id)).await
        {
            tracing::warn!("cannot remove network of environment {}: {e:#}", env.name);
        }
    }
    Ok(())
}

pub enum Power {
    Stop,
    Start,
    Restart,
}

pub async fn power(state: &Arc<AppState>, id: &str, action: Power) -> ApiResult<()> {
    let rec = record(state, id).await?;
    if rec.busy.is_some() {
        conflict!("a deployment of '{}' is in progress", rec.service.name);
    }
    let Some(container) = rec.live_container.clone() else {
        bad_request!("'{}' has not been deployed yet", rec.service.name);
    };
    let (_, docker) = state.docker_for(&rec.service.server_id).await?;
    let exists = docker::inspect(&docker, &container).await?.is_some();
    let svc = &rec.service;
    match action {
        Power::Stop => {
            state.db.set_stopped(id, true).await?;
            if exists {
                let options = StopContainerOptions {
                    t: Some(10),
                    ..Default::default()
                };
                match docker.stop_container(&container, Some(options)).await {
                    Ok(()) => {}
                    Err(e) if docker::status_code(&e) == Some(304) => {}
                    Err(e) => {
                        return Err(anyhow::Error::from(e)
                            .context("cannot stop the container")
                            .into());
                    }
                }
            }
            state
                .service_activity(svc, ActivityLevel::Info, "Service stopped".into(), None)
                .await;
            state
                .notify_service(svc, Event::new(NotifyEvent::ServiceStopped))
                .await;
        }
        Power::Start | Power::Restart => {
            state.db.set_stopped(id, false).await?;
            if !exists {
                let rec = record(state, id).await?;
                deploy::enqueue(state, &rec, "start", deploy::Plan::Reconfigure).await?;
            } else if matches!(action, Power::Start) {
                match docker.start_container(&container, None).await {
                    Ok(()) => {}
                    Err(e) if docker::status_code(&e) == Some(304) => {}
                    Err(e) => {
                        return Err(anyhow::Error::from(e)
                            .context("cannot start the container")
                            .into());
                    }
                }
            } else {
                docker
                    .restart_container(&container, None)
                    .await
                    .map_err(|e| anyhow::Error::from(e).context("cannot restart the container"))?;
            }
            let verb = if matches!(action, Power::Start) {
                "started"
            } else {
                "restarted"
            };
            state
                .service_activity(svc, ActivityLevel::Info, format!("Service {verb}"), None)
                .await;
            let kind = if matches!(action, Power::Start) {
                NotifyEvent::ServiceStarted
            } else {
                NotifyEvent::ServiceRestarted
            };
            state.notify_service(svc, Event::new(kind)).await;
        }
    }
    Ok(())
}

// Validation

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn validate_source(source: Source) -> ApiResult<Source> {
    Ok(match source {
        Source::Image { image } => {
            let image = image.trim().to_string();
            if image.is_empty() || image.chars().any(char::is_whitespace) {
                bad_request!("image must be a reference such as nginx:1.27");
            }
            Source::Image { image }
        }
        Source::Git {
            repo,
            branch,
            dockerfile,
            context,
        } => {
            let repo = repo.trim().to_string();
            if repo.is_empty() || repo.starts_with('-') {
                bad_request!("git source needs a repository URL");
            }
            let branch = non_empty(Some(branch)).unwrap_or_else(|| "main".into());
            if branch.starts_with('-') {
                bad_request!("invalid branch '{branch}'");
            }
            let dockerfile = non_empty(Some(dockerfile)).unwrap_or_else(|| "Dockerfile".into());
            let context = non_empty(Some(context)).unwrap_or_else(|| ".".into());
            for path in [&dockerfile, &context] {
                if path.starts_with('/') || path.split('/').any(|c| c == "..") {
                    bad_request!("path '{path}' must stay inside the repository");
                }
            }
            Source::Git {
                repo,
                branch,
                dockerfile,
                context,
            }
        }
        Source::Database { kind, version } => {
            let version = non_empty(Some(version)).unwrap_or_else(|| kind.default_version().into());
            if version
                .chars()
                .any(|c| c.is_whitespace() || c == ':' || c == '/')
            {
                bad_request!("invalid database version '{version}'");
            }
            Source::Database { kind, version }
        }
    })
}

fn validate_health_path(path: Option<String>) -> ApiResult<Option<String>> {
    let Some(path) = non_empty(path) else {
        return Ok(None);
    };
    if !path.starts_with('/') || path.chars().any(char::is_whitespace) {
        bad_request!("health path must start with '/', e.g. /healthz");
    }
    Ok(Some(path))
}

pub fn validate_env(vars: &[EnvVar]) -> ApiResult<()> {
    let mut seen = HashSet::new();
    for var in vars {
        let valid = var
            .key
            .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && var
                .key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            bad_request!("invalid environment variable name '{}'", var.key);
        }
        if !seen.insert(var.key.as_str()) {
            bad_request!("environment variable '{}' is set twice", var.key);
        }
    }
    Ok(())
}

/// Lowercased hostname with only letters, digits, dots and dashes.
pub fn validate_host(host: &str) -> ApiResult<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let wildcard_ok = |h: &str| h.strip_prefix("*.").unwrap_or(h).split('.').all(label_ok);
    if host.is_empty() || host.len() > 253 || !wildcard_ok(&host) {
        bad_request!("invalid domain '{host}'");
    }
    Ok(host)
}

pub fn validate_volume(name: &str, mount_path: &str) -> ApiResult<()> {
    validate_name(name).map_err(AppError::BadRequest)?;
    if !mount_path.starts_with('/') || mount_path == "/" || mount_path.contains(':') {
        bad_request!("mount path must be an absolute path such as /data");
    }
    Ok(())
}

// Databases

/// Generated credentials for a one-click database, plus a connection URL for other services.
fn database_env(kind: DatabaseKind, service_name: &str) -> Vec<EnvVar> {
    let plain = |key: &str, value: &str| EnvVar {
        key: key.into(),
        value: value.into(),
        secret: false,
    };
    let secret = |key: &str, value: &str| EnvVar {
        key: key.into(),
        value: value.into(),
        secret: true,
    };
    let mut user = service_name.replace('-', "_");
    let reserved: &[&str] = match kind {
        DatabaseKind::Mysql | DatabaseKind::Mariadb => &[
            "mysql",
            "sys",
            "root",
            "information_schema",
            "performance_schema",
        ],
        DatabaseKind::Mongo => &["admin", "local", "config"],
        DatabaseKind::Postgres | DatabaseKind::Redis => &[],
    };
    if reserved.contains(&user.as_str()) {
        user = "app".into();
    }
    let pass = generate_password();
    let creds = Credentials {
        user: (kind != DatabaseKind::Redis).then(|| user.clone()),
        password: pass.clone(),
        database: matches!(
            kind,
            DatabaseKind::Postgres | DatabaseKind::Mysql | DatabaseKind::Mariadb
        )
        .then(|| user.clone()),
        root_password: None,
    };
    let url = database::connection_url(kind, &creds, service_name, kind.port());
    match kind {
        DatabaseKind::Postgres => vec![
            plain("POSTGRES_USER", &user),
            secret("POSTGRES_PASSWORD", &pass),
            plain("POSTGRES_DB", &user),
            secret("DATABASE_URL", &url),
        ],
        DatabaseKind::Mysql => vec![
            secret("MYSQL_ROOT_PASSWORD", &generate_password()),
            plain("MYSQL_DATABASE", &user),
            plain("MYSQL_USER", &user),
            secret("MYSQL_PASSWORD", &pass),
            secret("DATABASE_URL", &url),
        ],
        DatabaseKind::Mariadb => vec![
            secret("MARIADB_ROOT_PASSWORD", &generate_password()),
            plain("MARIADB_DATABASE", &user),
            plain("MARIADB_USER", &user),
            secret("MARIADB_PASSWORD", &pass),
            secret("DATABASE_URL", &url),
        ],
        DatabaseKind::Redis => vec![secret("REDIS_PASSWORD", &pass), secret("REDIS_URL", &url)],
        DatabaseKind::Mongo => vec![
            plain("MONGO_INITDB_ROOT_USERNAME", &user),
            secret("MONGO_INITDB_ROOT_PASSWORD", &pass),
            secret("DATABASE_URL", &url),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        busy: Option<DeploymentStatus>,
        live: bool,
        stopped: bool,
        ever_failed: bool,
    ) -> ServiceRecord {
        ServiceRecord {
            service: Service {
                id: "s".into(),
                environment_id: "e".into(),
                server_id: "srv".into(),
                name: "web".into(),
                source: Source::Image {
                    image: "nginx".into(),
                },
                port: None,
                command: None,
                restart: RestartPolicy::UnlessStopped,
                health_path: None,
                created_at: Utc::now(),
                state: ServiceState::New,
                primary_domain: None,
                last_deployed_at: None,
                live_deployment_id: None,
            },
            stopped,
            busy,
            live_container: live.then(|| "c".into()),
            ever_failed,
        }
    }

    #[test]
    fn state_follows_deployments_and_container() {
        use ServiceState as S;
        assert_eq!(
            compute_state(&rec(None, false, false, false), None, true),
            S::New
        );
        assert_eq!(
            compute_state(&rec(None, false, false, true), None, true),
            S::Failed
        );
        assert_eq!(
            compute_state(
                &rec(Some(DeploymentStatus::Building), true, false, false),
                None,
                true
            ),
            S::Building
        );
        assert_eq!(
            compute_state(
                &rec(Some(DeploymentStatus::Starting), true, false, false),
                None,
                true
            ),
            S::Deploying
        );
        let live = rec(None, true, false, false);
        assert_eq!(
            compute_state(&live, Some(RunState::Running), true),
            S::Running
        );
        assert_eq!(
            compute_state(&live, Some(RunState::Restarting), true),
            S::Degraded
        );
        assert_eq!(
            compute_state(&live, Some(RunState::Exited { code: Some(1) }), true),
            S::Failed
        );
        assert_eq!(
            compute_state(&live, Some(RunState::Exited { code: Some(0) }), true),
            S::Stopped
        );
        assert_eq!(compute_state(&live, None, false), S::Degraded);
        let stopped = rec(None, true, true, false);
        assert_eq!(
            compute_state(&stopped, Some(RunState::Exited { code: Some(137) }), true),
            S::Stopped
        );
    }

    #[test]
    fn hosts_are_validated_and_normalized() {
        assert_eq!(
            validate_host("App.Example.com.").unwrap(),
            "app.example.com"
        );
        assert!(validate_host("*.example.com").is_ok());
        assert!(validate_host("bad host").is_err());
        assert!(validate_host("-a.com").is_err());
        assert!(validate_host("").is_err());
    }

    #[test]
    fn env_keys_are_validated() {
        let var = |k: &str| EnvVar {
            key: k.into(),
            value: String::new(),
            secret: false,
        };
        assert!(validate_env(&[var("A_1"), var("_B")]).is_ok());
        assert!(validate_env(&[var("1A")]).is_err());
        assert!(validate_env(&[var("A"), var("A")]).is_err());
    }

    #[test]
    fn databases_get_credentials() {
        let vars = database_env(DatabaseKind::Postgres, "main-db");
        let get = |k: &str| vars.iter().find(|v| v.key == k).unwrap();
        assert_eq!(get("POSTGRES_USER").value, "main_db");
        assert!(get("POSTGRES_PASSWORD").secret);
        assert_eq!(get("POSTGRES_PASSWORD").value.len(), 24);
        assert!(get("DATABASE_URL").value.starts_with("postgres://main_db:"));
        assert!(get("DATABASE_URL").value.ends_with("@main-db:5432/main_db"));
        let mongo = database_env(DatabaseKind::Mongo, "docs");
        let url = &mongo
            .iter()
            .find(|v| v.key == "DATABASE_URL")
            .unwrap()
            .value;
        assert!(url.ends_with("@docs:27017/?authSource=admin"), "{url}");
        let mysql = database_env(DatabaseKind::Mysql, "mysql");
        let get = |k: &str| &mysql.iter().find(|v| v.key == k).unwrap().value;
        assert_eq!(get("MYSQL_DATABASE"), "app", "never the system schema");
        assert_eq!(get("MYSQL_USER"), "app");
    }
}
