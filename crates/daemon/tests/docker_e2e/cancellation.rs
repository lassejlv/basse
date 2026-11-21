use super::{collect_logs, docker_ok, git, strings, wait_finished};
use crate::support::Daemon;
use client::{Client, LogStreamBox};
use common::api::{CreateProject, CreateService, UpdateService};
use common::{
    API_PREFIX, Deployment, DeploymentStatus, RestartPolicy, Source, StepKind, StepStatus, brand,
};
use futures_util::StreamExt as _;
use std::time::Duration;
use std::{io::ErrorKind, net::Ipv4Addr};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
use tokio::net::TcpListener;
use tokio::time::timeout;

const IMAGE: &str = "busybox:1.36";
const PREVIOUS_BODY: &str = "previous deployment";
const SETUP_TIMEOUT: Duration = Duration::from_secs(90);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(25);

#[tokio::test]
async fn cancelling_during_build_stops_docker_and_preserves_the_live_deployment() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let app = LiveApp::start(&daemon, vec![]).await;
    let gateway: Ipv4Addr = docker_ok(&strings(&[
        "network",
        "inspect",
        "--format",
        "{{(index .IPAM.Config 0).Gateway}}",
        "bridge",
    ]))
    .expect("Docker bridge gateway")
    .parse()
    .expect("IPv4 Docker bridge gateway");
    let listener = TcpListener::bind((gateway, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join("Dockerfile"),
        format!("FROM {IMAGE}\nRUN wget -T 120 -O /dev/null http://{gateway}:{port}/build\n"),
    )
    .unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    git(repo.path(), &["add", "Dockerfile"]);
    git(repo.path(), &["commit", "-q", "-m", "Blocking build"]);

    let candidate = app
        .deploy(UpdateService {
            source: Some(Source::Git {
                repo: format!("file://{}", repo.path().display()),
                branch: "main".into(),
                dockerfile: "Dockerfile".into(),
                context: ".".into(),
                source_id: None,
                auto_deploy: true,
            }),
            ..UpdateService::default()
        })
        .await;
    let logs = app.api.deployment_logs(&candidate.id, true).await.unwrap();
    let build = timeout(SETUP_TIMEOUT, async {
        let (connection, _) = listener.accept().await.unwrap();
        let mut build = BufReader::new(connection);
        let mut request = String::new();
        build.read_line(&mut request).await.unwrap();
        assert!(
            request.starts_with("GET /build "),
            "unexpected build request: {request:?}"
        );
        build
    })
    .await;
    let mut build = match build {
        Ok(build) => build,
        Err(_) => {
            let logs = app.api.deployment_logs(&candidate.id, false).await.unwrap();
            panic!(
                "Docker's RUN never opened its HTTP request: {:?}",
                collect_logs(logs).await
            );
        }
    };
    app.assert_in_step(&candidate.id, StepKind::Build).await;
    app.assert_previous_serving().await;

    cancel(&app.api, &candidate.id).await;
    let closed = timeout(Duration::from_secs(5), build.read_to_end(&mut Vec::new()))
        .await
        .expect("Docker's in-flight RUN was still waiting after cancel returned");
    if let Err(error) = closed {
        assert_eq!(error.kind(), ErrorKind::ConnectionReset, "{error}");
    }
    app.assert_cancelled(&candidate.id, logs).await;
}

#[tokio::test]
async fn cancelling_during_health_removes_the_candidate_and_preserves_the_live_deployment() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let app = LiveApp::start(&daemon, vec![]).await;
    let candidate = app
        .deploy(UpdateService {
            command: Some("mkdir -p /www; exec httpd -f -p 8080 -h /www".into()),
            ..UpdateService::default()
        })
        .await;
    let mut logs = app.api.deployment_logs(&candidate.id, true).await.unwrap();
    wait_for_log(&mut logs, "Waiting for GET /health on port 8080: HTTP 404").await;
    let health = app.assert_in_step(&candidate.id, StepKind::Health).await;
    let container = health.container_id.as_deref().expect("started candidate");
    assert_eq!(
        docker_ok(&strings(&[
            "inspect",
            "--format",
            "{{.State.Running}}",
            container
        ]))
        .expect("inspect health candidate"),
        "true",
        "candidate was not running before cancellation"
    );
    app.assert_previous_serving().await;

    cancel(&app.api, &candidate.id).await;
    app.assert_cancelled(&candidate.id, logs).await;
}

#[tokio::test]
async fn cancellation_is_rejected_after_traffic_cutover_begins() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let domain = format!("cancel-{}.localhost", rand::random::<u64>());
    let app = LiveApp::start(&daemon, vec![domain]).await;
    let candidate = app.api.deploy(&app.service_id, "api").await.unwrap();
    let mut logs = app.api.deployment_logs(&candidate.id, true).await.unwrap();
    wait_for_log(&mut logs, "Attached to the proxy as").await;
    app.assert_in_step(&candidate.id, StepKind::Switch).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{0}{API_PREFIX}/deployments/{1}/cancel",
            daemon.url, candidate.id
        ))
        .bearer_auth(&daemon.token)
        .timeout(CANCEL_TIMEOUT)
        .send()
        .await
        .expect("send cancel request during cutover");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::CONFLICT,
        "cancel was not rejected after attaching the candidate: {}",
        response.text().await.unwrap()
    );
    let finished = wait_finished(&app.api, &candidate.id, SETUP_TIMEOUT).await;
    assert_eq!(finished.status, DeploymentStatus::Live, "{finished:?}");
    let service = app.api.service(&app.service_id).await.unwrap().service;
    assert_eq!(
        service.live_deployment_id.as_deref(),
        Some(finished.id.as_str())
    );
}

async fn wait_for_log(logs: &mut LogStreamBox, expected: &str) {
    timeout(SETUP_TIMEOUT, async {
        while let Some(line) = logs.next().await {
            if line.unwrap().line.contains(expected) {
                return;
            }
        }
        panic!("deployment log closed before {expected:?}");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for deployment log {expected:?}"));
}

async fn cancel(api: &Client, id: &str) {
    let cancelled = timeout(CANCEL_TIMEOUT, api.cancel_deployment(id))
        .await
        .expect("cancel API did not return")
        .expect("cancel API rejected an in-progress deployment");
    assert_eq!(
        cancelled.status,
        DeploymentStatus::Cancelled,
        "{cancelled:?}"
    );
}

struct LiveApp {
    api: Client,
    service_id: String,
    network: String,
    previous: Deployment,
}

impl LiveApp {
    async fn start(daemon: &Daemon, domains: Vec<String>) -> Self {
        let api = daemon.client();
        api.create_project(&CreateProject {
            name: "cancel".into(),
            description: None,
        })
        .await
        .unwrap();
        let tree = api.tree().await.unwrap();
        let env = &tree.projects[0].environments[0].environment.id;
        let created = api
            .create_service(&CreateService {
                environment_id: env.clone(),
                server_id: None,
                name: "site".into(),
                source: Source::Image {
                    image: IMAGE.into(),
                },
                port: Some(8080),
                command: Some(format!(
                    "mkdir -p /www; printf '%s' '{PREVIOUS_BODY}' > /www/index.html; \
                     cp /www/index.html /www/health; exec httpd -f -p 8080 -h /www"
                )),
                restart: RestartPolicy::No,
                health_path: Some("/health".into()),
                env: vec![],
                domains,
                volumes: vec![],
                deploy: false,
            })
            .await
            .unwrap();
        let previous = api.deploy(&created.service.id, "api").await.unwrap();
        let mut app = Self {
            api,
            service_id: created.service.id,
            network: format!("{}-env-{env}", brand::PREFIX),
            previous,
        };
        app.previous = wait_finished(&app.api, &app.previous.id, SETUP_TIMEOUT).await;
        assert_eq!(
            app.previous.status,
            DeploymentStatus::Live,
            "{:?}",
            app.previous
        );
        app
    }

    async fn deploy(&self, patch: UpdateService) -> Deployment {
        self.api
            .update_service(&self.service_id, &patch)
            .await
            .unwrap();
        self.api.deploy(&self.service_id, "api").await.unwrap()
    }

    async fn assert_in_step(&self, id: &str, kind: StepKind) -> Deployment {
        let deployment = self.api.deployment(id).await.unwrap();
        assert!(
            deployment
                .steps
                .iter()
                .any(|step| step.kind == kind && step.status == StepStatus::Running),
            "deployment was not in {kind:?}: {deployment:?}"
        );
        deployment
    }

    async fn assert_previous_serving(&self) {
        let first = &self.previous;
        let container = first.container_id.as_deref().expect("live container");
        let detail = self.api.service(&self.service_id).await.unwrap();
        assert_eq!(
            detail.service.live_deployment_id.as_deref(),
            Some(first.id.as_str()),
            "the previous deployment lost live traffic: {detail:?}"
        );
        assert_eq!(
            self.api.deployment(&first.id).await.unwrap().status,
            DeploymentStatus::Live
        );
        let ip = docker_ok(&strings(&[
            "inspect",
            "--format",
            &format!(
                "{{{{(index .NetworkSettings.Networks {:?}).IPAddress}}}}",
                self.network
            ),
            container,
        ]))
        .expect("inspect IP of live container");
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
            .get(format!("http://{ip}:8080/"))
            .send()
            .await
            .expect("the previous container should still serve HTTP")
            .error_for_status()
            .unwrap();
        assert_eq!(response.text().await.unwrap(), PREVIOUS_BODY);
    }

    async fn assert_cancelled(&self, id: &str, logs: LogStreamBox) {
        let stored = self.api.deployment(id).await.unwrap();
        assert_eq!(stored.status, DeploymentStatus::Cancelled, "{stored:?}");
        assert!(stored.finished_at.is_some(), "{stored:?}");
        assert!(
            stored
                .steps
                .iter()
                .all(|step| !matches!(step.status, StepStatus::Running | StepStatus::Pending)),
            "cancel left unsettled steps: {stored:?}"
        );
        timeout(Duration::from_secs(5), collect_logs(logs))
            .await
            .expect("deployment log follower never closed after cancellation");
        let label = format!("label={}={id}", brand::label("deployment"));
        let containers = docker_ok(&strings(&["ps", "-aq", "--filter", &label]))
            .expect("list Docker containers for cancelled deployment");
        assert!(
            containers.is_empty(),
            "cancellation left candidate containers: {containers}"
        );
        self.assert_previous_serving().await;
    }
}

impl Drop for LiveApp {
    fn drop(&mut self) {
        let label = format!("label={}={}", brand::label("service"), self.service_id);
        if let Ok(containers) = docker_ok(&strings(&["ps", "-aq", "--filter", &label])) {
            for id in containers.lines() {
                let _ = docker_ok(&strings(&["rm", "-f", id]));
            }
        }
        let _ = docker_ok(&strings(&["network", "rm", &self.network]));
    }
}
