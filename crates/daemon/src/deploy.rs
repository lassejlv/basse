//! The deploy pipeline: one background task per deployment, serialized per service.

use crate::db::{ServiceRecord, new_id};
use crate::docker::{self, ContainerSpec, RunState};
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::notify::Event;
use crate::state::AppState;
use anyhow::{Context as _, Result, anyhow, bail};
use bollard::Docker;
use bollard::query_parameters::{BuildImageOptions, RemoveContainerOptions, RemoveImageOptions};
use chrono::Utc;
use common::notify::NotifyEvent;
use common::*;
use futures_util::StreamExt as _;
use std::collections::HashMap;
use std::fs::File;
use std::future::Future;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt as _;
use tokio::sync::{broadcast, watch};

mod methods;

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
    phase: watch::Sender<Phase>,
    log: Arc<DeployLog>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Working,
    Cancelling,
    CuttingOver,
    Finishing,
}

#[derive(Debug, thiserror::Error)]
#[error("cancelled")]
struct Cancelled;

fn transition(phase: &watch::Sender<Phase>, next: Phase) -> bool {
    phase.send_if_modified(|phase| {
        if *phase != Phase::Working {
            return false;
        }
        *phase = next;
        true
    })
}

async fn cancellation_requested(phase: &watch::Sender<Phase>) {
    let _ = phase
        .subscribe()
        .wait_for(|p| *p == Phase::Cancelling)
        .await;
}

async fn interruptible<T>(
    phase: &watch::Sender<Phase>,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        () = cancellation_requested(phase) => Err(Cancelled.into()),
        result = work => result,
    }
}

impl Deploys {
    pub(crate) fn lock_for(&self, service_id: &str) -> Arc<tokio::sync::Mutex<()>> {
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
}

struct Registration {
    state: Arc<AppState>,
    id: Id,
}

impl Drop for Registration {
    fn drop(&mut self) {
        lock(&self.state.deploys.running).remove(&self.id);
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
pub async fn enqueue(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    trigger: &str,
    plan: Plan,
) -> ApiResult<Deployment> {
    let service = &rec.service;
    let server = state.server(&service.server_id).await?;
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
    let log = Arc::new(DeployLog::create(&state.dirs.deployment_log(&d.id)));
    let (phase, _) = watch::channel(Phase::Working);
    let registration = Registration {
        state: state.clone(),
        id: d.id.clone(),
    };
    lock(&state.deploys.running).insert(
        d.id.clone(),
        Running {
            phase: phase.clone(),
            log: log.clone(),
        },
    );
    if let Err(e) = state.db.insert_deployment(&mut d, &server.id).await {
        let _ = tokio::fs::remove_file(state.dirs.deployment_log(&d.id)).await;
        return Err(e.into());
    }
    log.system(format!(
        "Deployment #{} queued (trigger: {trigger})",
        d.number
    ));
    state
        .notify_service(
            service,
            Event::new(NotifyEvent::DeployQueued)
                .deploy(d.number)
                .field("Trigger", trigger),
        )
        .await;
    tokio::spawn(run(
        Job {
            state: state.clone(),
            d: d.clone(),
            phase,
            log,
            service: None,
            docker: None,
            stopped_previous: None,
            build_tag: None,
            creating_container: false,
        },
        plan,
        state.deploys.lock_for(&service.id),
        registration,
    ));
    Ok(d)
}

/// Queues a redeploy of the live image when a live container must pick up new configuration.
pub async fn reconfigure(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
) -> ApiResult<Option<Deployment>> {
    if rec.stopped || (rec.service.live_deployment_id.is_none() && rec.busy.is_none()) {
        return Ok(None);
    }
    if let Source::Git { build, .. } = &rec.service.source
        && build.method == BuildMethod::Compose
    {
        return enqueue(state, rec, "config", Plan::Source).await.map(Some);
    }
    enqueue(state, rec, "config", Plan::Reconfigure)
        .await
        .map(Some)
}

/// Queues a new deployment that runs the image of `target`.
pub async fn rollback(state: &Arc<AppState>, target: &Deployment) -> ApiResult<Deployment> {
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
        .service(&target.service_id)
        .await?
        .ok_or_else(|| AppError::not_found("service", &target.service_id))?;
    enqueue(
        state,
        &rec,
        "rollback",
        Plan::Rollback(Box::new(target.clone())),
    )
    .await
}

/// Aborts a queued or running deployment and removes what it created.
pub async fn cancel(state: &Arc<AppState>, deployment_id: &str) -> ApiResult<Deployment> {
    let d = state
        .db
        .deployment(deployment_id)
        .await?
        .ok_or_else(|| AppError::not_found("deployment", deployment_id))?;
    if d.status.is_finished() {
        conflict!(
            "deployment #{} already finished ({})",
            d.number,
            d.status.as_str()
        );
    }
    let mut phase = {
        let running = lock(&state.deploys.running);
        let Some(running) = running.get(deployment_id) else {
            conflict!("deployment #{} is no longer running", d.number);
        };
        if !transition(&running.phase, Phase::Cancelling) {
            match *running.phase.borrow() {
                Phase::Cancelling => {}
                Phase::CuttingOver => {
                    conflict!("deployment #{} has started switching traffic", d.number);
                }
                _ => conflict!("deployment #{} is already finishing", d.number),
            }
        }
        running.phase.subscribe()
    };
    while phase.changed().await.is_ok() {}
    let d = state
        .db
        .deployment(deployment_id)
        .await?
        .ok_or_else(|| AppError::not_found("deployment", deployment_id))?;
    if d.status != DeploymentStatus::Cancelled {
        return Err(anyhow!("could not finish cancelling deployment #{}", d.number).into());
    }
    Ok(d)
}

async fn remove_build_dir(state: &AppState, deployment_id: &str) {
    let dir = state.dirs.build_dir(deployment_id);
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("cannot remove {}: {e}", dir.display());
    }
}

async fn run(
    mut job: Job,
    plan: Plan,
    service_lock: Arc<tokio::sync::Mutex<()>>,
    _registration: Registration,
) {
    let guard = interruptible(&job.phase, async { Ok(service_lock.lock_owned().await) }).await;
    let started = Instant::now();
    let (_guard, result) = match guard {
        Ok(guard) => (Some(guard), job.pipeline(plan).await),
        Err(e) => (None, Err(e)),
    };
    let result =
        if !transition(&job.phase, Phase::Finishing) && *job.phase.borrow() == Phase::Cancelling {
            Err(Cancelled.into())
        } else {
            result
        };
    job.finish(result, started).await;
}

/// One deployment in flight: its row, its log and the step being executed.
struct Job {
    state: Arc<AppState>,
    d: Deployment,
    phase: watch::Sender<Phase>,
    log: Arc<DeployLog>,
    service: Option<Service>,
    docker: Option<Docker>,
    /// Live container stopped before the new one started; restarted if the deploy fails.
    stopped_previous: Option<String>,
    build_tag: Option<String>,
    creating_container: bool,
}

impl Job {
    async fn pipeline(&mut self, plan: Plan) -> Result<()> {
        self.check_cancelled()?;
        let db = &self.state.db;
        let rec = db
            .service(&self.d.service_id)
            .await?
            .context("the service was deleted")?;
        // Two database servers must never run on one data volume, so databases are replaced
        // by stopping the live container first instead of overlapping old and new.
        let previous = rec
            .live_container
            .filter(|_| matches!(rec.service.source, Source::Database { .. }));
        let mut svc = rec.service;
        let env = db
            .environment(&svc.environment_id)
            .await?
            .context("the environment was deleted")?;
        let server = db
            .server(&svc.server_id)
            .await?
            .context("the server was deleted")?;
        let docker = interruptible(&self.phase, self.state.docker.get(&server)).await?;
        self.service = Some(svc.clone());
        self.docker = Some(docker.clone());
        self.log.system(format!(
            "Deploying {} to server {}",
            svc.source.label(),
            server.name
        ));
        self.state
            .service_activity(
                &svc,
                ActivityLevel::Info,
                format!("Deploy #{} started", self.d.number),
                Some(self.d.trigger.clone()),
            )
            .await;
        self.state
            .notify_service(
                &svc,
                Event::new(NotifyEvent::DeployStarted)
                    .deploy(self.d.number)
                    .detail(format!("{} to {}", svc.source.label(), server.name))
                    .field("Trigger", self.d.trigger.clone())
                    .hint(format!("{} deployments {}", brand::CLI_BIN, svc.name)),
            )
            .await;

        let image = match (&plan, &svc.source) {
            (
                Plan::Source,
                Source::Git {
                    repo,
                    branch,
                    source_id,
                    root,
                    ..
                },
            ) => {
                self.set_status(DeploymentStatus::Building).await;
                let root = root.clone();
                let dir = self
                    .clone_repo(&repo.clone(), &branch.clone(), source_id.clone().as_deref())
                    .await?;
                let dir = root_dir(&dir, &root)?;
                if !root.is_empty() {
                    self.log.system(format!("Building in {root}"));
                }
                let build = self.resolve_build(&mut svc, &dir).await?;
                self.service = Some(svc.clone());
                let Source::Git { context, .. } = &svc.source else {
                    unreachable!("checked above");
                };
                let tag = docker::build_tag(&svc.id, self.d.number);
                match build.method {
                    BuildMethod::Compose => {
                        return self
                            .deploy_compose(&docker, &server, &svc, &dir, &build)
                            .await;
                    }
                    BuildMethod::Railpack => {
                        self.build_railpack(&server, &svc, &dir, context, &tag)
                            .await?
                    }
                    BuildMethod::Dockerfile | BuildMethod::Auto => {
                        self.build(&docker, &svc, &dir, &build, &tag).await?
                    }
                }
                tag
            }
            (Plan::Source, Source::Image { image }) => {
                self.set_status(DeploymentStatus::Building).await;
                let image = docker::normalize_image(image);
                self.pull(&docker, &image).await?;
                image
            }
            (Plan::Source, Source::Database { kind, version }) => {
                self.set_status(DeploymentStatus::Building).await;
                let image = format!("{}:{version}", kind.image());
                self.pull(&docker, &image).await?;
                image
            }
            (Plan::Rollback(target), _) => target
                .image
                .clone()
                .context("rollback target has no image")?,
            (Plan::Reconfigure, _) => db
                .live_deployment(&svc.id)
                .await?
                .and_then(|d| d.image)
                .context("the service has no live deployment to reconfigure")?,
        };
        self.d.image = Some(image.clone());
        self.set_status(DeploymentStatus::Starting).await;

        let reused = !matches!(plan, Plan::Source);
        let container = self
            .start(&docker, &svc, &env, &image, reused, previous)
            .await?;
        let phase = self.phase.clone();
        if let Err(e) = interruptible(
            &phase,
            self.health(&docker, &server, &svc, &env, &container),
        )
        .await
        {
            let tail = interruptible(&phase, async {
                Ok(docker::tail_logs(&docker, &container, 30).await)
            })
            .await?;
            if !tail.is_empty() {
                self.log.system("Last container output:");
                for line in tail {
                    self.log.push(line.stream, line.line);
                }
            }
            return Err(e);
        }
        let routed =
            !docker::proxy_labels(&self.state.db.domains(&svc.id).await?, svc.port, &svc.id)
                .is_empty();
        self.switch(&docker, &svc, &container, routed).await?;
        self.cleanup(&svc, &container).await;
        Ok(())
    }

    async fn finish(&mut self, result: Result<()>, started: Instant) {
        let elapsed = format!("{}s", started.elapsed().as_secs());
        remove_build_dir(&self.state, &self.d.id).await;
        match result {
            Ok(()) => {
                self.log
                    .system(format!("Deployment #{} is live ({elapsed})", self.d.number));
                if let Some(svc) = &self.service {
                    self.state
                        .service_activity(
                            svc,
                            ActivityLevel::Success,
                            format!("Deploy #{} is live", self.d.number),
                            Some(elapsed.clone()),
                        )
                        .await;
                    let kind = if self.d.trigger == "rollback" {
                        NotifyEvent::DeployRolledBack
                    } else {
                        NotifyEvent::DeploySucceeded
                    };
                    let detail = match (&self.d.commit, &self.d.commit_message) {
                        (Some(sha), Some(msg)) => format!("{} {msg}", &sha[..sha.len().min(7)]),
                        _ => self.d.image.clone().unwrap_or_default(),
                    };
                    self.state
                        .notify_service(
                            svc,
                            Event::new(kind)
                                .deploy(self.d.number)
                                .detail(detail)
                                .field("Took", elapsed),
                        )
                        .await;
                }
            }
            Err(e) => {
                let cancelled = e.is::<Cancelled>();
                let message = format!("{e:#}");
                let failed_step = self
                    .d
                    .steps
                    .iter()
                    .find(|s| s.status == StepStatus::Running)
                    .map(|s| s.kind);
                let now = Utc::now();
                for step in &mut self.d.steps {
                    match step.status {
                        StepStatus::Running => {
                            step.status = StepStatus::Failed;
                            step.finished_at = Some(now);
                            if cancelled {
                                step.detail = Some(message.clone());
                            }
                        }
                        StepStatus::Pending => step.status = StepStatus::Skipped,
                        _ => {}
                    }
                }
                if let Some(docker) = &self.docker {
                    match self.remove_candidate(docker, cancelled).await {
                        Ok(()) => {
                            if let Some(previous) = &self.stopped_previous {
                                match docker::start_container(docker, previous).await {
                                    Ok(()) => self.log.system("Restarted the previous container"),
                                    Err(e) => self.log.push(
                                        LogStream::Stderr,
                                        format!("Cannot restart the previous container: {e:#}"),
                                    ),
                                }
                            }
                        }
                        Err(e) => self.log.push(
                            LogStream::Stderr,
                            format!("Cannot remove candidate container: {e:#}"),
                        ),
                    }
                    if cancelled && let Some(tag) = &self.build_tag {
                        self.remove_build_image(docker, tag).await;
                    }
                }
                self.d.status = if cancelled {
                    DeploymentStatus::Cancelled
                } else {
                    DeploymentStatus::Failed
                };
                self.d.error = Some(message.clone());
                self.d.finished_at = Some(Utc::now());
                self.persist().await;
                let (level, activity, detail) = if cancelled {
                    self.log
                        .system(format!("Deployment #{} cancelled", self.d.number));
                    (
                        ActivityLevel::Info,
                        format!("Deploy #{} cancelled", self.d.number),
                        None,
                    )
                } else {
                    self.log
                        .push(LogStream::Stderr, format!("Deployment failed: {message}"));
                    (
                        ActivityLevel::Error,
                        format!("Deploy #{} failed", self.d.number),
                        Some(message.clone()),
                    )
                };
                if cancelled && self.service.is_none() {
                    self.service = self
                        .state
                        .db
                        .service(&self.d.service_id)
                        .await
                        .ok()
                        .flatten()
                        .map(|rec| rec.service);
                }
                match &self.service {
                    Some(svc) => {
                        self.state
                            .service_activity(svc, level, activity, detail)
                            .await;
                        let kind = match failed_step {
                            _ if cancelled => NotifyEvent::DeployCancelled,
                            Some(StepKind::Health) => NotifyEvent::DeployHealthCheckFailed,
                            _ => NotifyEvent::DeployFailed,
                        };
                        let mut event = Event::new(kind).deploy(self.d.number);
                        if !cancelled {
                            event = event.detail(message).hint(format!(
                                "{} deployments {}",
                                brand::CLI_BIN,
                                svc.name
                            ));
                        }
                        if let Some(step) = failed_step {
                            event = event.field("Step", step.as_str());
                        }
                        self.state.notify_service(svc, event).await;
                    }
                    None => self.state.activity(level, activity, detail).await,
                }
            }
        }
    }

    async fn remove_build_image(&self, docker: &Docker, tag: &str) {
        let options = RemoveImageOptions {
            noprune: true,
            ..Default::default()
        };
        if let Err(e) = docker.remove_image(tag, Some(options), None).await
            && !docker::is_not_found(&e)
        {
            self.log.push(
                LogStream::Stderr,
                format!("Cannot remove build image {tag}: {e}"),
            );
        }
    }

    async fn remove_candidate(&self, docker: &Docker, cancelled: bool) -> Result<()> {
        let containers = match &self.d.container_id {
            Some(id) => vec![id.clone()],
            None if self.creating_container => {
                let label = format!("{}={}", brand::label("deployment"), self.d.id);
                docker::list_by_label(docker, &label)
                    .await?
                    .into_iter()
                    .filter_map(|container| container.id)
                    .collect()
            }
            None => return Ok(()),
        };
        for id in containers {
            if !cancelled {
                docker::remove_container(docker, &id).await?;
                continue;
            }
            let options = RemoveContainerOptions {
                force: true,
                v: true,
                ..Default::default()
            };
            if let Err(e) = docker.remove_container(&id, Some(options)).await
                && !docker::is_not_found(&e)
            {
                return Err(e.into());
            }
        }
        Ok(())
    }

    fn check_cancelled(&self) -> Result<()> {
        if *self.phase.borrow() == Phase::Cancelling {
            return Err(Cancelled.into());
        }
        Ok(())
    }

    // Steps

    async fn clone_repo(
        &mut self,
        repo: &str,
        branch: &str,
        source_id: Option<&str>,
    ) -> Result<PathBuf> {
        self.begin(StepKind::Clone, Some(format!("{repo}@{branch}")))
            .await;
        let dir = self.state.dirs.build_dir(&self.d.id);
        if let Err(e) = tokio::fs::remove_dir_all(&dir).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e.into());
        }
        let (url, auth) = match source_id {
            Some(id) => {
                let (url, token) = crate::sources::clone_access(&self.state, id, repo).await?;
                self.log
                    .system("Cloning with a short-lived token from the git source");
                (url, crate::github::git_auth_env(&token))
            }
            None => (repo.to_string(), Vec::new()),
        };
        let dir_arg = dir.to_string_lossy().into_owned();
        let out = self
            .git_with(
                &[
                    "clone",
                    "--depth",
                    "1",
                    "--single-branch",
                    "--branch",
                    branch,
                    "--",
                    &url,
                    &dir_arg,
                ],
                auth,
            )
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
        let _ = tokio::fs::remove_dir_all(dir.join(".git")).await;
        self.end(StepKind::Clone, Some(short)).await;
        Ok(dir)
    }

    async fn git(&self, args: &[&str]) -> Result<Output> {
        self.git_with(args, Vec::new()).await
    }

    /// Runs git with extra environment, such as the token header of
    /// [`crate::github::git_auth_env`].
    async fn git_with(&self, args: &[&str], env: Vec<(String, String)>) -> Result<Output> {
        self.check_cancelled()?;
        let ssh = crate::ssh::paths(&self.state.dirs.ssh_dir()).git_ssh_command();
        let mut child = tokio::process::Command::new("git")
            .args(args)
            .envs(env)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_SSH_COMMAND", ssh)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .context("cannot run git (is it installed?)")?;
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw)
            .context("git has no process group ID")?;
        let mut stdout = child.stdout.take().context("git has no stdout pipe")?;
        let mut stderr = child.stderr.take().context("git has no stderr pipe")?;
        let result = interruptible(&self.phase, async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let status = tokio::time::timeout(GIT_TIMEOUT, async {
                tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))?;
                child.wait().await
            })
            .await
            .map_err(|_| anyhow!("git {} timed out", args[0]))?
            .context("cannot read git result")?;
            Ok(Output {
                status,
                stdout: out,
                stderr: err,
            })
        })
        .await;
        if result.is_err() {
            use rustix::{io::Errno, process};
            if let Err(e) = process::kill_process_group(group, process::Signal::KILL)
                && e != Errno::SRCH
            {
                tracing::warn!("cannot terminate git process group: {e}");
                let _ = child.start_kill();
            }
            let _ = child.wait().await;
        }
        let out = result?;
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
        root_dir: &Path,
        build: &Build,
        tag: &str,
    ) -> Result<()> {
        let Source::Git {
            dockerfile,
            context,
            ..
        } = &svc.source
        else {
            bail!("not a git service");
        };
        self.begin(StepKind::Build, None).await;
        let build_args: HashMap<String, String> = self
            .state
            .db
            .env(&svc.id)
            .await?
            .into_iter()
            .map(|v| (v.key, v.value))
            .collect();
        let context_dir = safe_join(root_dir, context)?;
        let dockerfile_rel = clean_relative(dockerfile)?;
        if !context_dir.join(&dockerfile_rel).is_file() {
            bail!(
                "no Dockerfile at {}",
                Path::new(context).join(&dockerfile_rel).display()
            );
        }
        self.check_cancelled()?;
        let phase = self.phase.clone();
        let tar = tokio::task::spawn_blocking(move || tar_dir(&context_dir, &phase)).await??;
        self.check_cancelled()?;
        self.log.system(format!(
            "Sending build context ({:.1} MB)",
            tar.len() as f64 / 1_048_576.0
        ));
        let options = BuildImageOptions {
            dockerfile: dockerfile_rel.to_string_lossy().into_owned(),
            t: Some(tag.to_string()),
            target: build.target.clone().unwrap_or_default(),
            buildargs: (!build_args.is_empty()).then_some(build_args),
            rm: true,
            forcerm: true,
            labels: Some(HashMap::from([
                (brand::label("managed"), "true".to_string()),
                (brand::label("service"), svc.id.clone()),
            ])),
            ..Default::default()
        };
        self.build_tag = Some(tag.to_string());
        let phase = self.phase.clone();
        interruptible(&phase, async {
            let mut stream =
                docker.build_image(options, None, Some(bollard::body_full(tar.into())));
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
                        self.detail(StepKind::Build, progress).await;
                    }
                    self.log.push(LogStream::Stdout, line);
                }
            }
            if !docker::image_exists(docker, tag).await? {
                bail!("image build finished without producing {tag}");
            }
            Ok(())
        })
        .await?;
        self.end(StepKind::Build, Some(tag.to_string())).await;
        Ok(())
    }

    async fn pull(&mut self, docker: &Docker, image: &str) -> Result<()> {
        self.begin(StepKind::Pull, Some(image.to_string())).await;
        interruptible(&self.phase, async {
            match self.pull_image(docker, image).await {
                Ok(()) => Ok(()),
                Err(e) if docker::image_exists(docker, image).await.unwrap_or(false) => {
                    self.log
                        .push(LogStream::Stderr, format!("{e:#}; using the cached image"));
                    Ok(())
                }
                Err(e) => Err(e),
            }
        })
        .await?;
        self.end(StepKind::Pull, Some(image.to_string())).await;
        Ok(())
    }

    async fn pull_image(&self, docker: &Docker, image: &str) -> Result<()> {
        docker::pull_image(docker, image, |line| self.log.push(LogStream::Stdout, line)).await
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
        self.check_cancelled()?;
        self.begin(StepKind::Start, None).await;
        if reused {
            interruptible(&self.phase, async {
                if !docker::image_exists(docker, image).await? {
                    if image.starts_with(&format!("{}/", brand::PREFIX)) {
                        bail!(
                            "image {image} no longer exists on the server; deploy from source instead"
                        );
                    }
                    self.pull_image(docker, image).await?;
                }
                Ok(())
            })
            .await?;
        }
        let db = &self.state.db;
        let domains = db.domains(&svc.id).await?;
        let volumes = db.volumes(&svc.id).await?;
        let vars = db.env(&svc.id).await?;
        let published = match (db.public_port(&svc.id).await?, svc.port) {
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
            publish_ip: None,
            extra_hosts: Vec::new(),
        };
        self.check_cancelled()?;
        docker::remove_container(docker, &spec.name).await?;
        self.check_cancelled()?;
        if let Some(previous) = previous
            && interruptible(&self.phase, docker::inspect(docker, &previous))
                .await?
                .is_some_and(|c| {
                    matches!(
                        RunState::from_inspect(&c),
                        RunState::Running | RunState::Restarting
                    )
                })
        {
            self.check_cancelled()?;
            self.log
                .system("Stopping the previous container: a database cannot share its data volume");
            docker::stop_container(docker, &previous, DATABASE_STOP_SECS).await?;
            self.stopped_previous = Some(previous);
        }
        self.check_cancelled()?;
        self.creating_container = true;
        let id = docker::create_container(docker, &spec).await?;
        self.d.container_id = Some(id.clone());
        self.persist().await;
        self.check_cancelled()?;
        docker::start_container(docker, &id).await?;
        self.log.system(format!("Started container {}", spec.name));
        self.end(StepKind::Start, Some(spec.name)).await;
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
            )
            .await;
            let deadline = Instant::now() + STABLE_FOR;
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(1)).await;
                ensure_running(docker, container).await?;
            }
            self.end(
                StepKind::Health,
                Some(format!("running for {}s", STABLE_FOR.as_secs())),
            )
            .await;
            return Ok(());
        };
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        self.begin(StepKind::Health, Some(format!("GET {path}")))
            .await;
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
                            self.end(StepKind::Health, Some(detail)).await;
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
                        self.end(StepKind::Health, Some(format!("GET {path} -> ok")))
                            .await;
                        return Ok(());
                    }
                    126 | 127 => {
                        self.log
                            .system("Health check skipped: the image has no wget");
                        self.mark(
                            StepKind::Health,
                            StepStatus::Skipped,
                            Some("skipped (remote)".into()),
                        )
                        .await;
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
        if !transition(&self.phase, Phase::CuttingOver) {
            return Err(Cancelled.into());
        }
        self.begin(StepKind::Switch, None).await;
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
        self.d.status = DeploymentStatus::Live;
        self.d.finished_at = Some(now);
        self.state.db.switch_live(&self.d).await?;
        self.log.system("Traffic switched to the new container");
        Ok(())
    }

    /// Removes previous containers. Failures are logged; the deployment stays live.
    async fn cleanup(&mut self, svc: &Service, keep: &str) {
        self.begin(StepKind::Cleanup, None).await;
        let mut removed = 0;
        let mut errors = Vec::new();
        let servers = self
            .state
            .db
            .deployment_servers(&svc.id)
            .await
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
        let detail = format!(
            "removed {removed} old container{}",
            if removed == 1 { "" } else { "s" }
        );
        self.log.system(format!("Cleanup: {detail}"));
        if errors.is_empty() {
            self.end(StepKind::Cleanup, Some(detail)).await;
        } else {
            for e in &errors {
                self.log.push(LogStream::Stderr, format!("Cleanup: {e}"));
            }
            self.mark(
                StepKind::Cleanup,
                StepStatus::Failed,
                Some(errors.join("; ")),
            )
            .await;
        }
    }

    // Bookkeeping

    async fn set_status(&mut self, status: DeploymentStatus) {
        self.d.status = status;
        self.persist().await;
    }

    fn step_mut(&mut self, kind: StepKind) -> Option<&mut Step> {
        self.d.steps.iter_mut().find(|s| s.kind == kind)
    }

    async fn begin(&mut self, kind: StepKind, detail: Option<String>) {
        if let Some(step) = self.step_mut(kind) {
            step.status = StepStatus::Running;
            step.started_at = Some(Utc::now());
            step.detail = detail;
        }
        self.log.system(format!("==> {}", kind.as_str()));
        self.persist().await;
    }

    async fn detail(&mut self, kind: StepKind, detail: String) {
        if let Some(step) = self.step_mut(kind) {
            step.detail = Some(detail);
        }
        self.persist().await;
    }

    async fn end(&mut self, kind: StepKind, detail: Option<String>) {
        self.mark(kind, StepStatus::Done, detail).await;
    }

    async fn mark(&mut self, kind: StepKind, status: StepStatus, detail: Option<String>) {
        if let Some(step) = self.step_mut(kind) {
            step.status = status;
            step.finished_at = Some(Utc::now());
            if detail.is_some() {
                step.detail = detail;
            }
        }
        self.persist().await;
    }

    async fn persist(&self) {
        if let Err(e) = self.state.db.update_deployment(&self.d).await {
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

/// The service's root directory inside the clone, which must not leave it through a symlink.
fn root_dir(repo_dir: &Path, root: &str) -> Result<PathBuf> {
    let dir = repo_dir.join(clean_relative(root)?);
    if !dir.is_dir() {
        bail!("root directory '{root}' does not exist in the repository");
    }
    if !dir.canonicalize()?.starts_with(repo_dir.canonicalize()?) {
        bail!("root directory '{root}' must stay inside the repository");
    }
    Ok(dir)
}

fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let dir = base.join(clean_relative(rel)?);
    if !dir.is_dir() {
        bail!("build context '{rel}' does not exist in the repository");
    }
    Ok(dir)
}

struct BuildContext<'a> {
    bytes: Vec<u8>,
    phase: &'a watch::Sender<Phase>,
}

impl std::io::Write for BuildContext<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if *self.phase.borrow() == Phase::Cancelling {
            return Err(std::io::Error::other(Cancelled));
        }
        self.bytes.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn tar_dir(dir: &Path, phase: &watch::Sender<Phase>) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(BuildContext {
        bytes: Vec::new(),
        phase,
    });
    builder.follow_symlinks(false);
    builder.append_dir_all(".", dir)?;
    Ok(builder.into_inner()?.bytes)
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
    fn root_directories_stay_inside_the_clone() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("apps/api")).unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join("escape")).unwrap();
        assert_eq!(root_dir(repo.path(), "").unwrap(), repo.path());
        assert_eq!(
            root_dir(repo.path(), "apps/api").unwrap(),
            repo.path().join("apps/api")
        );
        assert!(root_dir(repo.path(), "apps/web").is_err());
        assert!(root_dir(repo.path(), "escape").is_err());
        assert!(root_dir(repo.path(), "../x").is_err());
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
