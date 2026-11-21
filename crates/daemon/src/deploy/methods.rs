//! Railpack and Docker Compose builds, run through their CLIs with the output streamed into the
//! deployment log.

use super::{Cancelled, Job, Phase, interruptible, transition};
use crate::build;
use crate::docker::{self, RunState};
use crate::replicas::{HEALTH_TIMEOUT, STABLE_FOR};
use anyhow::{Context as _, Result, anyhow, bail};
use bollard::Docker;
use chrono::Utc;
use common::*;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt as _, BufReader};

/// Longest a build or compose command may run.
const BUILD_TIMEOUT: Duration = Duration::from_secs(1800);

impl Job {
    /// Runs a command in `cwd`, streaming its output into the log line by line. Cancelling the
    /// deployment kills the command's whole process group.
    pub(super) async fn run_logged(
        &self,
        program: &Path,
        args: &[String],
        env: &[(String, String)],
        cwd: &Path,
    ) -> Result<String> {
        self.run_command(program, args, env, cwd, true).await
    }

    /// Like [`Job::run_logged`], but returns stdout without logging it, for output that can
    /// hold secrets such as a resolved compose file.
    async fn run_quiet(
        &self,
        program: &Path,
        args: &[String],
        env: &[(String, String)],
        cwd: &Path,
    ) -> Result<String> {
        self.run_command(program, args, env, cwd, false).await
    }

    async fn run_command(
        &self,
        program: &Path,
        args: &[String],
        env: &[(String, String)],
        cwd: &Path,
        log_stdout: bool,
    ) -> Result<String> {
        self.check_cancelled()?;
        let name = program.file_name().map_or_else(
            || program.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .envs(env.iter().cloned())
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("cannot run {name} (is it installed?)"))?;
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw)
            .with_context(|| format!("{name} has no process group ID"))?;
        let stdout = child.stdout.take().context("no stdout pipe")?;
        let stderr = child.stderr.take().context("no stderr pipe")?;
        let result = interruptible(&self.phase, async {
            let mut captured = String::new();
            let status = tokio::time::timeout(BUILD_TIMEOUT, async {
                let mut out = BufReader::new(stdout).lines();
                let mut err = BufReader::new(stderr).lines();
                let (mut out_done, mut err_done) = (false, false);
                while !(out_done && err_done) {
                    tokio::select! {
                        line = out.next_line(), if !out_done => match line? {
                            Some(l) => {
                                captured.push_str(&l);
                                captured.push('\n');
                                if log_stdout {
                                    self.push_line(LogStream::Stdout, &l);
                                }
                            }
                            None => out_done = true,
                        },
                        line = err.next_line(), if !err_done => match line? {
                            Some(l) => self.push_line(LogStream::Stdout, &l),
                            None => err_done = true,
                        },
                    }
                }
                child.wait().await
            })
            .await
            .map_err(|_| {
                anyhow!(
                    "{name} {} timed out",
                    args.first().map_or("", String::as_str)
                )
            })?
            .with_context(|| format!("cannot wait for {name}"))?;
            if !status.success() {
                bail!(
                    "{name} {} failed ({status})",
                    args.iter().take(2).cloned().collect::<Vec<_>>().join(" ")
                );
            }
            Ok(captured)
        })
        .await;
        if result.is_err() {
            use rustix::{io::Errno, process};
            if let Err(e) = process::kill_process_group(group, process::Signal::KILL)
                && e != Errno::SRCH
            {
                tracing::warn!("cannot terminate the {name} process group: {e}");
                let _ = child.start_kill();
            }
            let _ = child.wait().await;
        }
        result
    }

    fn push_line(&self, stream: LogStream, line: &str) {
        let line = line.rsplit('\r').next().unwrap_or(line).trim_end();
        if !line.is_empty() {
            self.log.push(stream, line);
        }
    }

    /// Settles `auto` on the method the service's root directory calls for and saves it on the
    /// service. `root_dir` is that directory inside the clone.
    pub(super) async fn resolve_build(
        &mut self,
        svc: &mut Service,
        root_dir: &Path,
    ) -> Result<Build> {
        let Source::Git {
            dockerfile,
            context,
            build,
            ..
        } = &mut svc.source
        else {
            bail!("not a git service");
        };
        if build.method != BuildMethod::Auto {
            return Ok(build.clone());
        }
        let found = build::detect(
            |p| root_dir.join(p).is_file(),
            &build::dockerfile_path(context, dockerfile),
            build.compose_file.as_deref(),
        );
        self.log.system(format!(
            "Detected the {} build method{}",
            found.method,
            found
                .file
                .as_deref()
                .map(|f| format!(" from {f}"))
                .unwrap_or_default()
        ));
        build.method = found.method;
        if found.method == BuildMethod::Compose {
            build.compose_file = found.file;
        }
        let build = build.clone();
        self.state.db.update_service(svc).await?;
        Ok(build)
    }

    /// Builds the image with Railpack: it writes a build plan for the detected language and
    /// BuildKit builds the plan through Railpack's frontend. It builds `context` inside the
    /// service's root directory, `root_dir`.
    pub(super) async fn build_railpack(
        &mut self,
        server: &Server,
        svc: &Service,
        root_dir: &Path,
        context: &str,
        tag: &str,
    ) -> Result<()> {
        self.begin(StepKind::Build, Some("railpack".into())).await;
        let log = self.log.clone();
        let railpack = build::ensure_railpack(&self.state.dirs, |l| log.system(l)).await?;
        let dir = super::safe_join(root_dir, context)?;
        let plan = dir.join("railpack-plan.json");
        let info = dir.join("railpack-info.json");
        let dir_arg = dir.to_string_lossy().into_owned();
        self.run_logged(
            &railpack,
            &[
                "prepare".into(),
                dir_arg.clone(),
                "--plan-out".into(),
                plan.to_string_lossy().into_owned(),
                "--info-out".into(),
                info.to_string_lossy().into_owned(),
            ],
            &[],
            &dir,
        )
        .await?;
        if let Some(provider) = std::fs::read(&info)
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
            .and_then(|v| v.get("detectedProviders").cloned())
            .and_then(|v| v.as_array().cloned())
            .and_then(|v| v.first().and_then(|p| p.as_str().map(String::from)))
        {
            self.log.system(format!("Railpack detected {provider}"));
            self.detail(StepKind::Build, format!("railpack · {provider}"))
                .await;
        }
        self.build_tag = Some(tag.to_string());
        let mut args: Vec<String> = [
            "buildx",
            "build",
            "--load",
            "--progress",
            "plain",
            "--tag",
            tag,
            "--build-arg",
        ]
        .map(String::from)
        .to_vec();
        args.push(format!("BUILDKIT_SYNTAX={}", build::railpack_frontend()));
        args.extend(["--file".into(), plan.to_string_lossy().into_owned()]);
        for (k, v) in [
            (brand::label("managed"), "true".to_string()),
            (brand::label("service"), svc.id.clone()),
        ] {
            args.extend(["--label".into(), format!("{k}={v}")]);
        }
        args.push(dir_arg);
        let env = self.state.docker.cli_env(server).await?;
        self.run_logged(Path::new("docker"), &args, &env, &dir)
            .await?;
        self.end(StepKind::Build, Some(tag.to_string())).await;
        Ok(())
    }

    /// Builds and starts a compose project as one group of containers. Every compose service
    /// gets the daemon's labels, and domains route to the compose service they name. The
    /// compose file is looked up in the service's root directory, `root_dir`, and a service
    /// with a `root` runs compose with it as the project directory.
    pub(super) async fn deploy_compose(
        &mut self,
        docker: &Docker,
        server: &Server,
        svc: &Service,
        root_dir: &Path,
        build: &Build,
    ) -> Result<()> {
        let file = match &build.compose_file {
            Some(f) => super::clean_relative(f)?.to_string_lossy().into_owned(),
            None => build::COMPOSE_FILES
                .iter()
                .find(|f| root_dir.join(f).is_file())
                .map(|f| (*f).to_string())
                .context("no compose file: add compose.yaml or set the compose file")?,
        };
        if !root_dir.join(&file).is_file() {
            bail!("no compose file at {file}");
        }
        let has_root = matches!(&svc.source, Source::Git { root, .. } if !root.is_empty());
        let project = build::compose_project(&svc.id);
        let mut env = self.state.docker.cli_env(server).await?;
        env.extend(
            self.state
                .db
                .env(&svc.id)
                .await?
                .into_iter()
                .map(|v| (v.key, v.value)),
        );
        let compose = |extra: &[&str]| -> Vec<String> {
            let mut args = vec!["compose".into(), "--project-name".into(), project.clone()];
            if has_root {
                args.extend([
                    "--project-directory".into(),
                    root_dir.to_string_lossy().into_owned(),
                ]);
            }
            args.extend(["--file".into(), file.clone()]);
            args.extend(extra.iter().map(|s| s.to_string()));
            args
        };
        self.begin(StepKind::Build, Some(format!("compose · {file}")))
            .await;
        let resolved: serde_json::Value = serde_json::from_str(
            &self
                .run_quiet(
                    Path::new("docker"),
                    &compose(&["config", "--format", "json"]),
                    &env,
                    root_dir,
                )
                .await?,
        )
        .context("cannot read the resolved compose file")?;
        let names: Vec<String> = resolved["services"]
            .as_object()
            .map(|s| s.keys().cloned().collect())
            .unwrap_or_default();
        if names.is_empty() {
            bail!("{file} has no services");
        }
        let domains = self.state.db.domains(&svc.id).await?;
        let mut overrides = build::compose_override(svc, &names, &domains)?;
        let repo = self.state.dirs.build_dir(&self.d.id);
        let plan = super::mounts::plan(&resolved, &repo, self.state.dirs.root())?;
        self.compose_mounts(docker, server, svc, &repo, &plan, &mut overrides)
            .await?;
        let override_file = format!(".{}-compose.json", brand::PREFIX);
        std::fs::write(root_dir.join(&override_file), overrides.to_string())?;
        if overrides.get("networks").is_some() {
            docker::ensure_network(docker, brand::PROXY_NETWORK).await?;
        }
        let with_override = |extra: &[&str]| -> Vec<String> {
            let mut args = compose(&[]);
            args.extend(["--file".into(), override_file.clone()]);
            args.extend(extra.iter().map(|s| s.to_string()));
            args
        };
        self.run_logged(
            Path::new("docker"),
            &with_override(&["build"]),
            &env,
            root_dir,
        )
        .await?;
        self.end(StepKind::Build, Some(format!("{} services", names.len())))
            .await;

        self.set_status(DeploymentStatus::Starting).await;
        self.begin(StepKind::Start, Some(project.clone())).await;
        self.run_logged(
            Path::new("docker"),
            &with_override(&["up", "--detach", "--remove-orphans"]),
            &env,
            root_dir,
        )
        .await?;
        self.end(StepKind::Start, None).await;

        self.begin(StepKind::Health, None).await;
        let phase = self.phase.clone();
        let first = interruptible(&phase, self.compose_health(docker, &project)).await?;
        self.end(StepKind::Health, Some(format!("{} running", names.len())))
            .await;

        if !transition(&self.phase, Phase::CuttingOver) {
            return Err(Cancelled.into());
        }
        self.begin(StepKind::Switch, None).await;
        self.d.container_id = Some(first);
        let now = Utc::now();
        let detail = format!("#{} live", self.d.number);
        if let Some(step) = self.step_mut(StepKind::Switch) {
            step.status = StepStatus::Done;
            step.detail = Some(detail);
            step.finished_at = Some(now);
        }
        self.d.status = DeploymentStatus::Live;
        self.d.finished_at = Some(now);
        self.state.db.switch_live(&self.d, &[]).await?;

        self.begin(StepKind::Cleanup, None).await;
        let mut removed = 0;
        for c in docker::list_service_containers(docker, &svc.id).await? {
            let in_project = c
                .labels
                .as_ref()
                .and_then(|l| l.get("com.docker.compose.project"))
                .is_some_and(|p| *p == project);
            if let (false, Some(id)) = (in_project, c.id) {
                docker::remove_container(docker, &id).await?;
                removed += 1;
            }
        }
        self.end(
            StepKind::Cleanup,
            Some(format!(
                "removed {removed} old container{}",
                if removed == 1 { "" } else { "s" }
            )),
        )
        .await;
        Ok(())
    }

    /// Waits until every container of the project runs (or exited cleanly, like a migration)
    /// for a few seconds. Returns the first running container.
    async fn compose_health(&self, docker: &Docker, project: &str) -> Result<String> {
        let label = format!("com.docker.compose.project={project}");
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        let mut stable_since: Option<Instant> = None;
        loop {
            let containers = docker::list_by_label(docker, &label).await?;
            let mut first = None;
            let mut settled = !containers.is_empty();
            for c in &containers {
                let name = c
                    .labels
                    .as_ref()
                    .and_then(|l| l.get("com.docker.compose.service"))
                    .cloned()
                    .unwrap_or_default();
                match RunState::from_summary(c) {
                    RunState::Running => {
                        if first.is_none() {
                            first = c.id.clone();
                        }
                    }
                    RunState::Exited { code: Some(0) } => {}
                    RunState::Exited { code } => {
                        if let Some(id) = &c.id {
                            for line in docker::tail_logs(docker, id, 30).await {
                                self.log
                                    .push(line.stream, format!("{name} | {}", line.line));
                            }
                        }
                        bail!(
                            "compose service {name} exited with code {}",
                            code.map_or("unknown".into(), |c| c.to_string())
                        );
                    }
                    RunState::Restarting | RunState::Other => settled = false,
                }
            }
            match (settled, first) {
                (true, Some(id)) => {
                    let since = *stable_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= STABLE_FOR {
                        return Ok(id);
                    }
                }
                _ => stable_since = None,
            }
            if Instant::now() > deadline {
                bail!(
                    "the compose services did not settle within {}s",
                    HEALTH_TIMEOUT.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
