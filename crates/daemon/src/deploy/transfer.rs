//! Moving a service to another server: the move is a deployment on the target that brings the
//! live image and, when asked, the named volumes along, and only takes the service off the old
//! server once the target is live.

use super::{Job, Move, Plan, enqueue, interruptible};
use crate::build;
use crate::db::ServiceRecord;
use crate::docker::{self, RunState};
use crate::error::{ApiResult, bad_request, conflict};
use crate::state::AppState;
use anyhow::{Context as _, Result};
use bollard::Docker;
use common::api::MoveService;
use common::*;
use std::collections::HashMap;
use std::sync::Arc;

/// Seconds a container gets to shut down before its volumes are copied; databases get longer
/// to flush.
const STOP_SECS: i32 = 10;
const DATABASE_STOP_SECS: i32 = 30;

/// The old server of a move in flight, and what the move changed so a failure can undo it.
pub(super) struct Moving {
    from: Server,
    docker: Docker,
    to: Id,
    copy_volumes: bool,
    /// Containers on the old server stopped for the volume copy; started again on failure.
    stopped: Vec<String>,
    /// Volumes the move created on the target; removed on failure.
    created: Vec<String>,
}

impl Moving {
    pub(super) fn new(from: Server, docker: Docker, to: Id, copy_volumes: bool) -> Self {
        Self {
            from,
            docker,
            to,
            copy_volumes,
            stopped: Vec::new(),
            created: Vec::new(),
        }
    }
}

/// Queues a move of the service to `body.server_id`.
pub async fn relocate(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    body: MoveService,
) -> ApiResult<Deployment> {
    let svc = &rec.service;
    let to = state.server(&body.server_id).await?;
    if to.id == svc.server_id {
        bad_request!("'{}' already runs on {}", svc.name, to.name);
    }
    if rec.stopped {
        conflict!("'{}' is stopped; start it before moving it", svc.name);
    }
    if rec.busy.is_some() {
        conflict!(
            "'{}' is deploying; wait for that deployment or cancel it, then move it",
            svc.name
        );
    }
    let compose =
        matches!(&svc.source, Source::Git { build, .. } if build.method == BuildMethod::Compose);
    let database = matches!(svc.source, Source::Database { .. });
    let plan = Move {
        to,
        copy_volumes: body.copy_volumes || database,
        from_source: compose || svc.live_deployment_id.is_none(),
    };
    enqueue(state, rec, "move", Plan::Move(Box::new(plan))).await
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / f64::from(1u32 << 30)),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / f64::from(1u32 << 20)),
        b => format!("{:.1} kB", b as f64 / 1024.0),
    }
}

fn is_compose(svc: &Service) -> bool {
    matches!(&svc.source, Source::Git { build, .. } if build.method == BuildMethod::Compose)
}

impl Job {
    /// The server a move goes to, once it is in flight.
    pub(super) fn moved_to(&self) -> Option<&str> {
        self.moving.as_ref().map(|m| m.to.as_str())
    }

    /// Before a move starts the service on the target: brings `image` over when the target
    /// lacks it, and with `copy_volumes` stops the old containers and copies the volumes.
    /// Does nothing outside a move.
    pub(super) async fn transfer(
        &mut self,
        target: &Docker,
        svc: &Service,
        image: Option<&str>,
    ) -> Result<()> {
        let Some((from, source, copy_volumes)) = self
            .moving
            .as_ref()
            .map(|m| (m.from.name.clone(), m.docker.clone(), m.copy_volumes))
        else {
            return Ok(());
        };
        self.check_cancelled()?;
        if self.d.status == DeploymentStatus::Queued {
            self.set_status(DeploymentStatus::Building).await;
        }
        self.begin(StepKind::Transfer, None).await;
        let phase = self.phase.clone();
        let mut done = Vec::new();
        if let Some(image) = image {
            if interruptible(&phase, docker::image_exists(target, image)).await? {
                self.log
                    .system(format!("{image} is already on the target server"));
            } else if image.starts_with(&format!("{}/", brand::PREFIX)) {
                self.detail(StepKind::Transfer, "image".into()).await;
                self.log
                    .system(format!("Copying image {image} from {from}"));
                let size =
                    interruptible(&phase, docker::copy_image(&source, target, image)).await?;
                self.log
                    .system(format!("Copied image {image} ({})", human_size(size)));
            } else {
                self.detail(StepKind::Transfer, "image".into()).await;
                let pulled = interruptible(&phase, async {
                    docker::pull_image(target, image, |l| {
                        self.log.push(LogStream::Stdout, l);
                    })
                    .await
                })
                .await;
                if let Err(e) = pulled {
                    self.log.push(
                        LogStream::Stderr,
                        format!("{e:#}; copying the image from {from} instead"),
                    );
                    interruptible(&phase, docker::copy_image(&source, target, image)).await?;
                }
            }
            done.push("image".to_string());
        }
        if copy_volumes {
            let volumes = self.source_volumes(&source, svc).await?;
            if volumes.is_empty() {
                self.log.system("No volumes to copy");
            } else {
                self.stop_source(&source, svc).await?;
                let mut total = 0;
                for (i, (name, labels)) in volumes.iter().enumerate() {
                    self.check_cancelled()?;
                    self.detail(
                        StepKind::Transfer,
                        format!("volume {}/{}", i + 1, volumes.len()),
                    )
                    .await;
                    total += self.copy_volume(&source, target, svc, name, labels).await?;
                }
                let count = format!(
                    "{} volume{}",
                    volumes.len(),
                    if volumes.len() == 1 { "" } else { "s" }
                );
                self.log
                    .system(format!("Copied {count} ({})", human_size(total)));
                done.push("volumes".to_string());
            }
        }
        let detail = if done.is_empty() {
            "nothing to copy".to_string()
        } else {
            done.join(" + ")
        };
        self.end(StepKind::Transfer, Some(detail)).await;
        Ok(())
    }

    /// Named volumes of the service on the old server, with their labels.
    async fn source_volumes(
        &self,
        source: &Docker,
        svc: &Service,
    ) -> Result<Vec<(String, HashMap<String, String>)>> {
        if is_compose(svc) {
            let label = format!(
                "com.docker.compose.project={}",
                build::compose_project(&svc.id)
            );
            return docker::volumes_by_label(source, &label).await;
        }
        let mut out = Vec::new();
        for v in self.state.db.volumes(&svc.id).await? {
            let name = docker::volume_name(&svc.id, &v.name);
            match docker::volume_labels(source, &name).await? {
                Some(labels) => out.push((name, labels)),
                None => self.log.system(format!(
                    "Volume {} does not exist yet; nothing to copy",
                    v.name
                )),
            }
        }
        Ok(out)
    }

    /// Stops the service's running containers on the old server so its volumes hold still.
    async fn stop_source(&mut self, source: &Docker, svc: &Service) -> Result<()> {
        let running: Vec<String> = docker::list_service_containers(source, &svc.id)
            .await?
            .into_iter()
            .filter(|c| {
                matches!(
                    RunState::from_summary(c),
                    RunState::Running | RunState::Restarting
                )
            })
            .filter_map(|c| c.id)
            .collect();
        if running.is_empty() {
            return Ok(());
        }
        let from = self.moving.as_ref().map(|m| m.from.name.clone());
        self.log.system(format!(
            "Stopping {} container{} on {} while the volumes copy; a failed move starts them again",
            running.len(),
            if running.len() == 1 { "" } else { "s" },
            from.unwrap_or_default()
        ));
        for id in running {
            self.check_cancelled()?;
            let secs = if matches!(svc.source, Source::Database { .. }) {
                DATABASE_STOP_SECS
            } else {
                STOP_SECS
            };
            docker::stop_container(source, &id, secs).await?;
            if let Some(m) = &mut self.moving {
                m.stopped.push(id);
            }
        }
        Ok(())
    }

    /// Copies one volume into a fresh volume of the same name on the target. A volume of that
    /// name already there is a leftover of this service, such as from an earlier move away,
    /// and is replaced.
    async fn copy_volume(
        &mut self,
        source: &Docker,
        target: &Docker,
        svc: &Service,
        name: &str,
        labels: &HashMap<String, String>,
    ) -> Result<u64> {
        let phase = self.phase.clone();
        if docker::volume_labels(target, name).await?.is_some() {
            self.log.system(format!(
                "Replacing volume {name}, which the target server already had"
            ));
            docker::remove_volume(target, name).await.with_context(|| {
                format!("volume {name} on the target server is in use; remove what uses it first")
            })?;
        }
        docker::create_volume(target, name, labels.clone()).await?;
        if let Some(m) = &mut self.moving {
            m.created.push(name.to_string());
        }
        for engine in [source, target] {
            if !docker::image_exists(engine, docker::HELPER_IMAGE).await? {
                interruptible(&phase, async {
                    docker::pull_image(engine, docker::HELPER_IMAGE, |_| {}).await
                })
                .await?;
            }
        }
        let helper = format!("{}-move-{}", brand::PREFIX, svc.id);
        let size =
            interruptible(&phase, docker::copy_volume(source, target, name, &helper)).await?;
        self.log
            .system(format!("Copied volume {name} ({})", human_size(size)));
        Ok(size)
    }

    /// After the target went live: takes what is left of the service off the old server and
    /// says what still points there. Failures are logged; the move stands.
    pub(super) async fn finish_move(&mut self, svc: &Service) {
        let Some(m) = self.moving.take() else { return };
        let mut removed = 0;
        let mut errors = Vec::new();
        match docker::remove_service_containers(&m.docker, &svc.id, None).await {
            Ok(n) => removed = n,
            Err(e) => errors.push(format!("{e:#}")),
        }
        if is_compose(svc)
            && let Err(e) =
                docker::remove_compose_leftovers(&m.docker, &build::compose_project(&svc.id), false)
                    .await
        {
            errors.push(format!("{e:#}"));
        }
        for e in errors {
            self.log.push(
                LogStream::Stderr,
                format!("Cleanup on {}: {e}", m.from.name),
            );
        }
        if removed > 0 {
            self.log.system(format!(
                "Removed {removed} container{} from {}",
                if removed == 1 { "" } else { "s" },
                m.from.name
            ));
        }
        if m.copy_volumes {
            self.log.system(format!(
                "The volumes on {} are kept; nothing uses them any more",
                m.from.name
            ));
        }
        let to = self.state.server(&m.to).await.ok();
        let to_name = to.as_ref().map_or_else(|| m.to.clone(), |s| s.name.clone());
        let hosts: Vec<String> = self
            .state
            .db
            .domains(&svc.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.host)
            .collect();
        if !hosts.is_empty() {
            let address = match to.and_then(|s| s.host) {
                Some(host) => host,
                None => self
                    .state
                    .public_ips()
                    .await
                    .0
                    .map_or_else(|| "its address".to_string(), |ip| ip.to_string()),
            };
            self.log.system(format!(
                "{to_name} serves {} now; point their DNS records at {address} if they still name {}",
                hosts.join(", "),
                m.from.name
            ));
        }
        self.state
            .service_activity(
                svc,
                ActivityLevel::Success,
                format!("Moved to server {to_name}"),
                Some(format!("from {}", m.from.name)),
            )
            .await;
    }

    /// After a failed or cancelled move: removes what it put on the target and starts what it
    /// stopped on the old server, which keeps serving.
    pub(super) async fn undo_move(&mut self) {
        let Some(m) = self.moving.take() else { return };
        let Some(svc) = self.service.clone() else {
            return;
        };
        if let Some(target) = self.docker.clone() {
            if let Err(e) = docker::remove_service_containers(&target, &svc.id, None).await {
                self.log.push(
                    LogStream::Stderr,
                    format!("Cannot remove the containers on the target: {e:#}"),
                );
            }
            if is_compose(&svc)
                && let Err(e) = docker::remove_compose_leftovers(
                    &target,
                    &build::compose_project(&svc.id),
                    false,
                )
                .await
            {
                self.log.push(LogStream::Stderr, format!("{e:#}"));
            }
            for name in &m.created {
                if let Err(e) = docker::remove_volume(&target, name).await {
                    self.log.push(LogStream::Stderr, format!("{e:#}"));
                }
            }
        }
        let mut started = 0;
        for id in &m.stopped {
            match docker::start_container(&m.docker, id).await {
                Ok(()) => started += 1,
                Err(e) => self.log.push(
                    LogStream::Stderr,
                    format!(
                        "Cannot start {} on {} again: {e:#}",
                        docker::short_id(id),
                        m.from.name
                    ),
                ),
            }
        }
        self.log.system(format!(
            "Move undone: {} keeps serving {}{}",
            m.from.name,
            svc.name,
            if started > 0 {
                format!(", {started} stopped container(s) started again")
            } else {
                String::new()
            }
        ));
    }
}
