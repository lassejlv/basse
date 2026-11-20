//! The deploy pipeline: one background task per deployment, serialized per service.

use crate::db::{ServiceRecord, new_id};
use crate::docker::{self, ContainerSpec, RunState};
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::state::AppState;
use anyhow::{Context as _, Result, anyhow, bail};
use bollard::Docker;
use bollard::query_parameters::BuildImageOptions;
use chrono::Utc;
use common::*;
use futures_util::StreamExt as _;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);
const STABLE_FOR: Duration = Duration::from_secs(5);
const GIT_TIMEOUT: Duration = Duration::from_secs(600);
/// Pause after attaching a container to the proxy so the proxy has loaded its labels (first
/// deploy, changed domains) before old containers are stopped.
const PROXY_DRAIN: Duration = Duration::from_secs(2);
/// Seconds a database gets to shut down before its replacement starts on the same volume.
const DATABASE_STOP_SECS: i32 = 30;

/// Where the image of a deployment comes from.
pub enum Plan {
    /// Clone and build, or pull, from the service's source.
    Source,
    /// Reuse the image of an earlier deployment.
    Rollback(Box<Deployment>),
    /// Recreate the live container with fresh configuration (domains, env).
    Reconfigure,
}

/// Running deployments and the per-service locks that serialize them.
#[derive(Default)]
pub struct Deploys {
    locks: Mutex<HashMap<Id, Arc<tokio::sync::Mutex<()>>>>,
    running: Mutex<HashMap<Id, Running>>,
}

struct Running {
    handle: JoinHandle<()>,
    log: Arc<DeployLog>,
    server_id: Id,
}

impl Deploys {
    fn lock_for(&self, service_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.locks)
            .entry(service_id.to_string())
            .or_default()
            .clone()
    }

    /// Live log of a deployment that is still queued or running.
    pub fn log(&self, deployment_id: &str) -> Option<Arc<DeployLog>> {
        lock(&self.running)
            .get(deployment_id)
            .map(|r| r.log.clone())
    }

    pub fn forget_service(&self, service_id: &str) {
        lock(&self.locks).remove(service_id);
    }

    fn take(&self, deployment_id: &str) -> Option<Running> {
        lock(&self.running).remove(deployment_id)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A deployment's log: appended to `deployments/<id>.log` as NDJSON, kept in memory while
/// running, and broadcast to live followers.
pub struct DeployLog {
    inner: Mutex<LogInner>,
    tx: broadcast::Sender<LogLine>,
}

struct LogInner {
    history: Vec<LogLine>,
    file: Option<File>,
}

impl DeployLog {
    fn create(path: &Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .inspect_err(|e| tracing::warn!("cannot open {}: {e}", path.display()))
            .ok();
        let (tx, _) = broadcast::channel(4096);
        Self {
            inner: Mutex::new(LogInner {
                history: Vec::new(),
                file,
            }),
            tx,
        }
    }

    pub fn push(&self, stream: LogStream, line: impl Into<String>) {
        let line = LogLine {
            ts: Some(Utc::now()),
            stream,
            line: line.into(),
        };
        let mut inner = lock(&self.inner);
        if let Some(file) = &mut inner.file
            && let Err(e) = serde_json::to_writer(&mut *file, &line)
                .map_err(std::io::Error::from)
                .and_then(|()| file.write_all(b"\n"))
        {
            tracing::warn!("cannot write deployment log: {e}");
        }
        inner.history.push(line.clone());
        let _ = self.tx.send(line);
    }

    pub fn system(&self, line: impl Into<String>) {
        self.push(LogStream::System, line);
    }

    /// Lines so far plus a receiver for every later line, without gaps or duplicates.
    pub fn subscribe(&self) -> (Vec<LogLine>, broadcast::Receiver<LogLine>) {
        let inner = lock(&self.inner);
        (inner.history.clone(), self.tx.subscribe())
    }
}

/// Queues a deployment and starts its background task. Returns the queued row at once.
pub fn enqueue(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    trigger: &str,
    plan: Plan,
) -> ApiResult<Deployment> {
    let service = &rec.service;
    let server = state.server(&service.server_id)?;
    let kinds: &[StepKind] = match (&plan, &service.source) {
        (Plan::Source, Source::Git { .. }) => &[
            StepKind::Clone,
            StepKind::Build,
            StepKind::Start,
            StepKind::Health,
            StepKind::Switch,
            StepKind::Cleanup,
        ],
        (Plan::Source, _) => &[
            StepKind::Pull,
            StepKind::Start,
            StepKind::Health,
            StepKind::Switch,
            StepKind::Cleanup,
        ],
        _ => &[
            StepKind::Start,
            StepKind::Health,
            StepKind::Switch,
            StepKind::Cleanup,
        ],
    };
    let (image, commit, commit_message) = match &plan {
        Plan::Rollback(target) => (
            target.image.clone(),
            target.commit.clone(),
            target.commit_message.clone(),
        ),
        _ => (None, None, None),
    };
    let mut d = Deployment {
        id: new_id(),
        service_id: service.id.clone(),
        number: 0,
        status: DeploymentStatus::Queued,
        trigger: trigger.to_string(),
        commit,
        commit_message,
        image,
        container_id: None,
        steps: kinds
            .iter()
            .map(|&kind| Step {
                kind,
                status: StepStatus::Pending,
                detail: None,
                started_at: None,
                finished_at: None,
            })
            .collect(),
        error: None,
        created_at: Utc::now(),
        finished_at: None,
    };
    state.db.insert_deployment(&mut d, &server.id)?;

    let log = Arc::new(DeployLog::create(&state.dirs.deployment_log(&d.id)));
    log.system(format!(
        "Deployment #{} queued (trigger: {trigger})",
        d.number
    ));
    let service_lock = state.deploys.lock_for(&service.id);
    // Holding the registry lock while spawning guarantees the entry exists before the task
    // can try to remove it.
    let mut running = lock(&state.deploys.running);
    let handle = tokio::spawn(run(
        state.clone(),
        d.clone(),
        plan,
        log.clone(),
        service_lock,
    ));
    running.insert(
        d.id.clone(),
        Running {
            handle,
            log,
            server_id: server.id,
        },
    );
    Ok(d)
}

/// Queues a redeploy of the live image when a live container must pick up new configuration.
pub fn reconfigure(state: &Arc<AppState>, rec: &ServiceRecord) -> ApiResult<Option<Deployment>> {
    if rec.stopped || (rec.service.live_deployment_id.is_none() && rec.busy.is_none()) {
        return Ok(None);
    }
    enqueue(state, rec, "config", Plan::Reconfigure).map(Some)
}

/// Queues a new deployment that runs the image of `target`.
pub fn rollback(state: &Arc<AppState>, target: &Deployment) -> ApiResult<Deployment> {
    if target.image.is_none()
        || !matches!(
            target.status,
            DeploymentStatus::Live | DeploymentStatus::Superseded
        )
    {
        bad_request!(
            "deployment #{} never went live, so there is no image to roll back to",
            target.number
        );
    }
    if target.status == DeploymentStatus::Live {
        bad_request!("deployment #{} is already live", target.number);
    }
    let rec = state
        .db
        .service(&target.service_id)?
        .ok_or_else(|| AppError::not_found("service", &target.service_id))?;
    enqueue(
        state,
        &rec,
        "rollback",
        Plan::Rollback(Box::new(target.clone())),
    )
}

/// Aborts a queued or running deployment and removes what it created.
pub async fn cancel(state: &Arc<AppState>, deployment_id: &str) -> ApiResult<Deployment> {
    let d = state
        .db
        .deployment(deployment_id)?
        .ok_or_else(|| AppError::not_found("deployment", deployment_id))?;
    if d.status.is_finished() {
        conflict!(
            "deployment #{} already finished ({})",
            d.number,
            d.status.as_str()
        );
    }
    let running = state.deploys.take(deployment_id);
    let log = match running {
        Some(Running {
            handle,
            log,
            server_id,
        }) => {
            handle.abort();
            let _ = handle.await;
            remove_deployment_containers(state, &server_id, deployment_id).await;
            Some(log)
        }
        None => None,
    };
    let mut d = state
        .db
        .deployment(deployment_id)?
        .ok_or_else(|| AppError::not_found("deployment", deployment_id))?;
    remove_build_dir(state, deployment_id);
    if d.status.is_finished() {
        return Ok(d);
    }
    let now = Utc::now();
    for step in &mut d.steps {
        match step.status {
            StepStatus::Running => {
                step.status = StepStatus::Failed;
                step.detail = Some("cancelled".into());
                step.finished_at = Some(now);
            }
            StepStatus::Pending => step.status = StepStatus::Skipped,
            _ => {}
        }
    }
    d.status = DeploymentStatus::Cancelled;
    d.error = Some("cancelled".into());
    d.finished_at = Some(now);
    state.db.update_deployment(&d)?;
    if let Some(log) = log {
        log.system(format!("Deployment #{} cancelled", d.number));
    }
    if let Some(rec) = state.db.service(&d.service_id)? {
        state.service_activity(
            &rec.service,
            ActivityLevel::Info,
            format!("Deploy #{} cancelled", d.number),
            None,
        );
    }
    Ok(d)
}

async fn remove_deployment_containers(state: &AppState, server_id: &str, deployment_id: &str) {
    let Ok((_, docker)) = state.docker_for(server_id).await else {
        return;
    };
    let label = format!("{}={deployment_id}", brand::label("deployment"));
    let Ok(containers) = docker::list_by_label(&docker, &label).await else {
        return;
    };
    for id in containers.into_iter().filter_map(|c| c.id) {
        if let Err(e) = docker::remove_container(&docker, &id).await {
            tracing::warn!("cannot remove container of cancelled deployment: {e:#}");
        }
    }
}

fn remove_build_dir(state: &AppState, deployment_id: &str) {
    let dir = state.dirs.build_dir(deployment_id);
    if dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&dir)
    {
        tracing::warn!("cannot remove {}: {e}", dir.display());
    }
}

async fn run(
    state: Arc<AppState>,
    deployment: Deployment,
    plan: Plan,
    log: Arc<DeployLog>,
    service_lock: Arc<tokio::sync::Mutex<()>>,
) {
    let _guard = service_lock.lock_owned().await;
    let id = deployment.id.clone();
    let mut job = Job {
        state: state.clone(),
        d: deployment,
        log,
        current: None,
        service: None,
        docker: None,
        stopped_previous: None,
    };
    let started = Instant::now();
    let result = job.pipeline(plan).await;
    job.finish(result, started).await;
    drop(job);
    state.deploys.take(&id);
}

/// One deployment in flight: its row, its log and the step being executed.
struct Job {
    state: Arc<AppState>,
    d: Deployment,
    log: Arc<DeployLog>,
    current: Option<StepKind>,
    service: Option<Service>,
    docker: Option<Docker>,
    /// Live container stopped before the new one started; restarted if the deploy fails.
    stopped_previous: Option<String>,
}

impl Job {
    async fn pipeline(&mut self, plan: Plan) -> Result<()> {
        let db = &self.state.db;
        let rec = db
            .service(&self.d.service_id)?
            .context("the service was deleted")?;
        // Two database servers must never run on one data volume, so databases are replaced
        // by stopping the live container first instead of overlapping old and new.
        let previous = rec
            .live_container
            .filter(|_| matches!(rec.service.source, Source::Database { .. }));
        let svc = rec.service;
        let env = db
            .environment(&svc.environment_id)?
            .context("the environment was deleted")?;
        let server = db
            .server(&svc.server_id)?
            .context("the server was deleted")?;
        let docker = self.state.docker.get(&server).await?;
        self.service = Some(svc.clone());
        self.docker = Some(docker.clone());
        self.log.system(format!(
            "Deploying {} to server {}",
            svc.source.label(),
            server.name
        ));
        self.state.service_activity(
            &svc,
            ActivityLevel::Info,
            format!("Deploy #{} started", self.d.number),
            Some(self.d.trigger.clone()),
        );

        let image = match (&plan, &svc.source) {
            (
                Plan::Source,
                Source::Git {
                    repo,
                    branch,
                    dockerfile,
                    context,
                },
            ) => {
                self.set_status(DeploymentStatus::Building);
                let dir = self.clone_repo(repo, branch).await?;
                let tag = docker::build_tag(&svc.id, self.d.number);
                self.build(&docker, &svc, &dir, context, dockerfile, &tag)
                    .await?;
                tag
            }
            (Plan::Source, Source::Image { image }) => {
                self.set_status(DeploymentStatus::Building);
                let image = docker::normalize_image(image);
                self.pull(&docker, &image).await?;
                image
            }
            (Plan::Source, Source::Database { kind, version }) => {
                self.set_status(DeploymentStatus::Building);
                let image = format!("{}:{version}", kind.image());
                self.pull(&docker, &image).await?;
                image
            }
            (Plan::Rollback(target), _) => target
                .image
                .clone()
                .context("rollback target has no image")?,
            (Plan::Reconfigure, _) => db
                .live_deployment(&svc.id)?
                .and_then(|d| d.image)
                .context("the service has no live deployment to reconfigure")?,
        };
        self.d.image = Some(image.clone());
        self.set_status(DeploymentStatus::Starting);

        let reused = !matches!(plan, Plan::Source);
        let container = self
            .start(&docker, &svc, &env, &image, reused, previous)
            .await?;
        if let Err(e) = self.health(&docker, &server, &svc, &env, &container).await {
            let tail = docker::tail_logs(&docker, &container, 30).await;
            if !tail.is_empty() {
                self.log.system("Last container output:");
                for line in tail {
                    self.log.push(line.stream, line.line);
                }
            }
            return Err(e);
        }
        let routed =
            !docker::proxy_labels(&self.state.db.domains(&svc.id)?, svc.port, &svc.id).is_empty();
        self.switch(&docker, &svc, &container, routed).await?;
        self.cleanup(&svc, &container).await;
        Ok(())
    }

    async fn finish(&mut self, result: Result<()>, started: Instant) {
        let elapsed = format!("{}s", started.elapsed().as_secs());
        remove_build_dir(&self.state, &self.d.id);
        match result {
            Ok(()) => {
                self.log
                    .system(format!("Deployment #{} is live ({elapsed})", self.d.number));
                if let Some(svc) = &self.service {
                    self.state.service_activity(
                        svc,
                        ActivityLevel::Success,
                        format!("Deploy #{} is live", self.d.number),
                        Some(elapsed),
                    );
                }
            }
            Err(e) => {
                let message = format!("{e:#}");
                if let Some(kind) = self.current.take() {
                    self.mark(kind, StepStatus::Failed, None);
                }
                for step in &mut self.d.steps {
                    if step.status == StepStatus::Pending {
                        step.status = StepStatus::Skipped;
                    }
                }
                if let (Some(docker), Some(id)) = (&self.docker, &self.d.container_id)
                    && let Err(e) = docker::remove_container(docker, id).await
                {
                    tracing::warn!("cannot remove failed container: {e:#}");
                }
                if let (Some(docker), Some(previous)) = (&self.docker, self.stopped_previous.take())
                {
                    match docker::start_container(docker, &previous).await {
                        Ok(()) => self.log.system("Restarted the previous container"),
                        Err(e) => self.log.push(
                            LogStream::Stderr,
                            format!("Cannot restart the previous container: {e:#}"),
                        ),
                    }
                }
                self.d.status = DeploymentStatus::Failed;
                self.d.error = Some(message.clone());
                self.d.finished_at = Some(Utc::now());
                self.persist();
                self.log
                    .push(LogStream::Stderr, format!("Deployment failed: {message}"));
                match &self.service {
                    Some(svc) => self.state.service_activity(
                        svc,
                        ActivityLevel::Error,
                        format!("Deploy #{} failed", self.d.number),
                        Some(message),
                    ),
                    None => self.state.activity(
                        ActivityLevel::Error,
                        format!("Deploy #{} failed", self.d.number),
                        Some(message),
                    ),
                }
            }
        }
    }

    // Steps

    async fn clone_repo(&mut self, repo: &str, branch: &str) -> Result<PathBuf> {
        self.begin(StepKind::Clone, Some(format!("{repo}@{branch}")));
        let dir = self.state.dirs.build_dir(&self.d.id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        let dir_arg = dir.to_string_lossy().into_owned();
        let out = self
            .git(&[
                "clone",
                "--depth",
                "1",
                "--single-branch",
                "--branch",
                branch,
                "--",
                repo,
                &dir_arg,
            ])
            .await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let reason = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("unknown error");
            bail!("git clone failed: {reason}");
        }
        let out = self
            .git(&["-C", &dir_arg, "log", "-1", "--format=%H%n%s"])
            .await?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let sha = lines.next().unwrap_or_default().to_string();
        let subject = lines.next().unwrap_or_default().to_string();
        let short = sha.get(..7).unwrap_or(&sha).to_string();
        self.log.system(format!("Checked out {short} {subject}"));
        self.d.commit = Some(sha);
        self.d.commit_message = Some(subject);
        let _ = std::fs::remove_dir_all(dir.join(".git"));
        self.end(StepKind::Clone, Some(short));
        Ok(dir)
    }

    async fn git(&self, args: &[&str]) -> Result<std::process::Output> {
        let ssh = crate::ssh::paths(&self.state.dirs.ssh_dir()).git_ssh_command();
        let child = tokio::process::Command::new("git")
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_SSH_COMMAND", ssh)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(GIT_TIMEOUT, child)
            .await
            .map_err(|_| anyhow!("git {} timed out", args[0]))?
            .context("cannot run git (is it installed?)")?;
        for line in String::from_utf8_lossy(&out.stderr)
            .lines()
            .chain(String::from_utf8_lossy(&out.stdout).lines())
        {
            let line = line.rsplit('\r').next().unwrap_or(line).trim_end();
            if !line.is_empty() && args[0] == "clone" {
                self.log.push(LogStream::Stdout, line);
            }
        }
        Ok(out)
    }

    async fn build(
        &mut self,
        docker: &Docker,
        svc: &Service,
        repo_dir: &Path,
        context: &str,
        dockerfile: &str,
        tag: &str,
    ) -> Result<()> {
        self.begin(StepKind::Build, None);
        let context_dir = safe_join(repo_dir, context)?;
        let dockerfile_rel = clean_relative(dockerfile)?;
        if !context_dir.join(&dockerfile_rel).is_file() {
            bail!(
                "no Dockerfile at {}",
                Path::new(context).join(&dockerfile_rel).display()
            );
        }
        let tar = tokio::task::spawn_blocking(move || tar_dir(&context_dir)).await??;
        self.log.system(format!(
            "Sending build context ({:.1} MB)",
            tar.len() as f64 / 1_048_576.0
        ));
        let options = BuildImageOptions {
            dockerfile: dockerfile_rel.to_string_lossy().into_owned(),
            t: Some(tag.to_string()),
            rm: true,
            forcerm: true,
            labels: Some(HashMap::from([
                (brand::label("managed"), "true".to_string()),
                (brand::label("service"), svc.id.clone()),
            ])),
            ..Default::default()
        };
        let mut stream = docker.build_image(options, None, Some(bollard::body_full(tar.into())));
        while let Some(item) = stream.next().await {
            let info = item.map_err(|e| anyhow!("image build failed: {e}"))?;
            if let Some(err) = info.error_detail.and_then(|e| e.message) {
                self.log.push(LogStream::Stderr, err.clone());
                bail!("image build failed: {err}");
            }
            for line in info
                .stream
                .iter()
                .chain(info.status.iter())
                .flat_map(|s| s.lines())
            {
                let line = line.trim_end();
                if line.is_empty() {
                    continue;
                }
                if let Some(progress) = build_progress(line) {
                    self.detail(StepKind::Build, progress);
                }
                self.log.push(LogStream::Stdout, line);
            }
        }
        if !docker::image_exists(docker, tag).await? {
            bail!("image build finished without producing {tag}");
        }
        self.end(StepKind::Build, Some(tag.to_string()));
        Ok(())
    }

    async fn pull(&mut self, docker: &Docker, image: &str) -> Result<()> {
        self.begin(StepKind::Pull, Some(image.to_string()));
        let log = self.log.clone();
        match docker::pull_image(docker, image, |line| log.push(LogStream::Stdout, line)).await {
            Ok(()) => {}
            Err(e) if docker::image_exists(docker, image).await.unwrap_or(false) => {
                self.log
                    .push(LogStream::Stderr, format!("{e:#}; using the cached image"));
            }
            Err(e) => return Err(e),
        }
        self.end(StepKind::Pull, Some(image.to_string()));
        Ok(())
    }

    async fn start(
        &mut self,
        docker: &Docker,
        svc: &Service,
        env: &Environment,
        image: &str,
        reused: bool,
        previous: Option<String>,
    ) -> Result<String> {
        self.begin(StepKind::Start, None);
        if reused && !docker::image_exists(docker, image).await? {
            if image.starts_with(&format!("{}/", brand::PREFIX)) {
                bail!("image {image} no longer exists on the server; deploy from source instead");
            }
            let log = self.log.clone();
            docker::pull_image(docker, image, |line| log.push(LogStream::Stdout, line)).await?;
        }
        let db = &self.state.db;
        let domains = db.domains(&svc.id)?;
        let volumes = db.volumes(&svc.id)?;
        let vars = db.env(&svc.id)?;
        let published = match (db.public_port(&svc.id)?, svc.port) {
            (Some(host_port), Some(port)) => {
                self.log.system(format!(
                    "Publishing port {port} on the server as {host_port}"
                ));
                vec![(format!("{port}/tcp"), host_port)]
            }
            _ => Vec::new(),
        };

        let env_net = docker::env_network(&env.id);
        docker::ensure_network(docker, &env_net).await?;
        let mut labels = docker::proxy_labels(&domains, svc.port, &svc.id);
        for d in domains.iter().filter(|d| d.port.or(svc.port).is_none()) {
            self.log.push(
                LogStream::Stderr,
                format!(
                    "Domain {} is not routed: set a port on the service or the domain",
                    d.host
                ),
            );
        }
        let networks = vec![(env_net, vec![svc.name.clone()])];
        labels.insert(brand::label("managed"), "true".into());
        labels.insert(brand::label("service"), svc.id.clone());
        labels.insert(brand::label("deployment"), self.d.id.clone());
        let mut binds = Vec::with_capacity(volumes.len());
        for v in &volumes {
            let name = docker::volume_name(&svc.id, &v.name);
            docker::ensure_volume(docker, &name).await?;
            binds.push(format!("{name}:{}", v.mount_path));
        }
        let spec = ContainerSpec {
            name: docker::container_name(&svc.name, &env.id, self.d.number),
            image: image.to_string(),
            env: vars
                .iter()
                .map(|v| format!("{}={}", v.key, v.value))
                .collect(),
            cmd: svc
                .command
                .clone()
                .map(|c| vec!["sh".into(), "-c".into(), c]),
            labels,
            binds,
            restart: svc.restart,
            networks,
            published,
        };
        docker::remove_container(docker, &spec.name).await?;
        if let Some(previous) = previous {
            self.log
                .system("Stopping the previous container: a database cannot share its data volume");
            docker::stop_container(docker, &previous, DATABASE_STOP_SECS).await?;
            self.stopped_previous = Some(previous);
        }
        let id = docker::create_container(docker, &spec).await?;
        self.d.container_id = Some(id.clone());
        self.persist();
        docker::start_container(docker, &id).await?;
        self.log.system(format!("Started container {}", spec.name));
        self.end(StepKind::Start, Some(spec.name));
        Ok(id)
    }

    async fn health(
        &mut self,
        docker: &Docker,
        server: &Server,
        svc: &Service,
        env: &Environment,
        container: &str,
    ) -> Result<()> {
        let (Some(path), Some(port)) = (svc.health_path.as_deref(), svc.port) else {
            self.begin(
                StepKind::Health,
                Some(format!("running for {}s", STABLE_FOR.as_secs())),
            );
            let deadline = Instant::now() + STABLE_FOR;
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(1)).await;
                ensure_running(docker, container).await?;
            }
            self.end(
                StepKind::Health,
                Some(format!("running for {}s", STABLE_FOR.as_secs())),
            );
            return Ok(());
        };
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        self.begin(StepKind::Health, Some(format!("GET {path}")));
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        let mut last_error = String::from("no response");
        loop {
            let inspected = ensure_running(docker, container).await?;
            if server.kind == ServerKind::Local {
                if let Some(ip) = docker::container_ip(&inspected, &docker::env_network(&env.id)) {
                    match self
                        .state
                        .http
                        .get(format!("http://{ip}:{port}{path}"))
                        .send()
                        .await
                    {
                        Ok(res) if res.status().is_success() || res.status().is_redirection() => {
                            let detail = format!("GET {path} -> {}", res.status().as_u16());
                            self.log.system(format!("Health check passed: {detail}"));
                            self.end(StepKind::Health, Some(detail));
                            return Ok(());
                        }
                        Ok(res) => last_error = format!("HTTP {}", res.status()),
                        Err(e) => last_error = describe_reqwest_error(&e),
                    }
                }
            } else {
                let url = format!("http://127.0.0.1:{port}{path}");
                let cmd = ["wget", "-q", "-O", "/dev/null", "-T", "3", &url]
                    .map(String::from)
                    .to_vec();
                match docker::exec(docker, container, cmd).await? {
                    0 => {
                        self.end(StepKind::Health, Some(format!("GET {path} -> ok")));
                        return Ok(());
                    }
                    126 | 127 => {
                        self.log
                            .system("Health check skipped: the image has no wget");
                        self.mark(
                            StepKind::Health,
                            StepStatus::Skipped,
                            Some("skipped (remote)".into()),
                        );
                        self.current = None;
                        return Ok(());
                    }
                    code => last_error = format!("wget exited with {code}"),
                }
            }
            if Instant::now() >= deadline {
                bail!(
                    "health check GET {path} on port {port} failed for {}s: {last_error}",
                    HEALTH_TIMEOUT.as_secs()
                );
            }
            self.log.system(format!(
                "Waiting for GET {path} on port {port}: {last_error}"
            ));
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Puts the healthy container behind the proxy, then marks it live. Old containers keep
    /// serving until cleanup stops them.
    async fn switch(
        &mut self,
        docker: &Docker,
        svc: &Service,
        container: &str,
        routed: bool,
    ) -> Result<()> {
        self.begin(StepKind::Switch, None);
        if routed {
            let alias = docker::proxy_alias(&svc.id);
            docker::connect_to_proxy(docker, container, &alias).await?;
            self.log.system(format!("Attached to the proxy as {alias}"));
            tokio::time::sleep(PROXY_DRAIN).await;
        }
        let now = Utc::now();
        let detail = format!("#{} live", self.d.number);
        if let Some(step) = self.step_mut(StepKind::Switch) {
            step.status = StepStatus::Done;
            step.detail = Some(detail);
            step.finished_at = Some(now);
        }
        self.current = None;
        self.d.status = DeploymentStatus::Live;
        self.d.finished_at = Some(now);
        self.state.db.switch_live(&self.d)?;
        self.log.system("Traffic switched to the new container");
        Ok(())
    }

    /// Removes previous containers. Failures are logged; the deployment stays live.
    async fn cleanup(&mut self, svc: &Service, keep: &str) {
        self.begin(StepKind::Cleanup, None);
        let mut removed = 0;
        let mut errors = Vec::new();
        let servers = self
            .state
            .db
            .deployment_servers(&svc.id)
            .unwrap_or_default();
        for server_id in servers {
            let Ok((_, docker)) = self.state.docker_for(&server_id).await else {
                continue;
            };
            match docker::remove_service_containers(&docker, &svc.id, Some(keep)).await {
                Ok(n) => removed += n,
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        remove_build_dir(&self.state, &self.d.id);
        let detail = format!(
            "removed {removed} old container{}",
            if removed == 1 { "" } else { "s" }
        );
        self.log.system(format!("Cleanup: {detail}"));
        if errors.is_empty() {
            self.end(StepKind::Cleanup, Some(detail));
        } else {
            for e in &errors {
                self.log.push(LogStream::Stderr, format!("Cleanup: {e}"));
            }
            self.mark(
                StepKind::Cleanup,
                StepStatus::Failed,
                Some(errors.join("; ")),
            );
            self.current = None;
        }
    }

    // Bookkeeping

    fn set_status(&mut self, status: DeploymentStatus) {
        self.d.status = status;
        self.persist();
    }

    fn step_mut(&mut self, kind: StepKind) -> Option<&mut Step> {
        self.d.steps.iter_mut().find(|s| s.kind == kind)
    }

    fn begin(&mut self, kind: StepKind, detail: Option<String>) {
        if let Some(step) = self.step_mut(kind) {
            step.status = StepStatus::Running;
            step.started_at = Some(Utc::now());
            step.detail = detail;
        }
        self.current = Some(kind);
        self.log.system(format!("==> {}", kind.as_str()));
        self.persist();
    }

    fn detail(&mut self, kind: StepKind, detail: String) {
        if let Some(step) = self.step_mut(kind) {
            step.detail = Some(detail);
        }
        self.persist();
    }

    fn end(&mut self, kind: StepKind, detail: Option<String>) {
        self.mark(kind, StepStatus::Done, detail);
        self.current = None;
    }

    fn mark(&mut self, kind: StepKind, status: StepStatus, detail: Option<String>) {
        if let Some(step) = self.step_mut(kind) {
            step.status = status;
            step.finished_at = Some(Utc::now());
            if detail.is_some() {
                step.detail = detail;
            }
        }
        self.persist();
    }

    fn persist(&self) {
        if let Err(e) = self.state.db.update_deployment(&self.d) {
            tracing::warn!(deployment = %self.d.id, "cannot save deployment: {e:#}");
        }
    }
}

/// Fails unless the container is running and has not restarted.
async fn ensure_running(
    docker: &Docker,
    container: &str,
) -> Result<bollard::models::ContainerInspectResponse> {
    let inspected = docker::inspect(docker, container)
        .await?
        .context("the container disappeared")?;
    match RunState::from_inspect(&inspected) {
        RunState::Running if inspected.restart_count.unwrap_or(0) == 0 => Ok(inspected),
        RunState::Running | RunState::Restarting => bail!("the container is crash-looping"),
        RunState::Exited { code } => {
            bail!(
                "the container exited with code {}",
                code.map_or("?".into(), |c| c.to_string())
            )
        }
        RunState::Other => bail!("the container is not running"),
    }
}

fn describe_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection refused".into()
    } else {
        e.to_string()
    }
}

/// `Step 7/12 : RUN ...` becomes `step 7/12`.
fn build_progress(line: &str) -> Option<String> {
    let rest = line.strip_prefix("Step ")?;
    let counter = rest.split_whitespace().next()?;
    let (current, total) = counter.split_once('/')?;
    (current.parse::<u32>().is_ok() && total.parse::<u32>().is_ok())
        .then(|| format!("step {counter}"))
}

/// A relative path without `..` or root components.
fn clean_relative(rel: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => bail!("path '{rel}' must stay inside the repository"),
        }
    }
    Ok(out)
}

fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let dir = base.join(clean_relative(rel)?);
    if !dir.is_dir() {
        bail!("build context '{rel}' does not exist in the repository");
    }
    Ok(dir)
}

fn tar_dir(dir: &Path) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    builder.follow_symlinks(false);
    builder.append_dir_all(".", dir)?;
    Ok(builder.into_inner()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_progress_parses_classic_builder_steps() {
        assert_eq!(
            build_progress("Step 7/12 : RUN make").as_deref(),
            Some("step 7/12")
        );
        assert_eq!(build_progress("Step one"), None);
        assert_eq!(build_progress(" ---> abc"), None);
    }

    #[test]
    fn relative_paths_cannot_escape() {
        assert_eq!(
            clean_relative("./docker/Dockerfile").unwrap(),
            PathBuf::from("docker/Dockerfile")
        );
        assert_eq!(clean_relative(".").unwrap(), PathBuf::new());
        assert!(clean_relative("../etc").is_err());
        assert!(clean_relative("/etc").is_err());
    }

    #[test]
    fn deploy_log_subscribers_see_history_then_new_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.log");
        let log = DeployLog::create(&path);
        log.system("one");
        let (history, mut rx) = log.subscribe();
        log.system("two");
        assert_eq!(history.len(), 1);
        assert_eq!(rx.try_recv().unwrap().line, "two");
        let file = std::fs::read_to_string(&path).unwrap();
        assert_eq!(file.lines().count(), 2);
        let first: LogLine = serde_json::from_str(file.lines().next().unwrap()).unwrap();
        assert_eq!(first.line, "one");
    }
}
