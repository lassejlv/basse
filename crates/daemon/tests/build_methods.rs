//! The three build methods against a real Docker engine, each with a small fixture repository:
//! a multi-stage Dockerfile with a target and build args, a compose project with a routed
//! service and a named volume, and a Node app built by Railpack. Run with
//! `cargo test -p daemon --test build_methods -- --ignored --test-threads 1`.

mod support;

use client::Client;
use common::api::*;
use common::*;
use futures_util::StreamExt as _;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use support::Daemon;

fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A bare repository holding `files` on `main`, as a `file://` URL.
fn fixture(root: &Path, files: &[(&str, &str)]) -> String {
    let work = root.join("work");
    for (path, body) in files {
        let path = work.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    run(&work, "git", &["init", "-q", "-b", "main"]);
    run(&work, "git", &["add", "."]);
    run(&work, "git", &["commit", "-q", "-m", "fixture"]);
    let bare = root.join("repo.git");
    run(
        root,
        "git",
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    format!("file://{}", bare.display())
}

async fn environment(api: &Client) -> Environment {
    api.create_project(&CreateProject {
        name: "builds".into(),
        description: None,
    })
    .await
    .unwrap();
    api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone()
}

fn git_service(
    env: &Environment,
    name: &str,
    repo: String,
    method: BuildMethod,
    port: u16,
) -> CreateService {
    CreateService {
        environment_id: env.id.clone(),
        server_id: None,
        name: name.into(),
        source: Source::Git {
            repo,
            branch: "main".into(),
            dockerfile: "Dockerfile".into(),
            context: ".".into(),
            source_id: None,
            auto_deploy: true,
            build: Build {
                method,
                ..Build::default()
            },
        },
        port: Some(port),
        command: None,
        restart: RestartPolicy::UnlessStopped,
        health_path: None,
        env: vec![EnvVar {
            key: "GREETING".into(),
            value: "hello-from-env".into(),
            secret: false,
        }],
        domains: vec![],
        volumes: vec![],
        deploy: false,
    }
}

async fn wait_finished(api: &Client, id: &str, timeout: Duration) -> Deployment {
    let deadline = Instant::now() + timeout;
    loop {
        let d = api.deployment(id).await.unwrap();
        if d.status.is_finished() {
            return d;
        }
        assert!(
            Instant::now() < deadline,
            "deployment #{} did not finish",
            d.number
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Deploys and waits for the outcome; returns the deployment and its log.
async fn deploy(api: &Client, service_id: &str, timeout: Duration) -> (Deployment, Vec<String>) {
    let d = api.deploy(service_id, "test").await.unwrap();
    let d = wait_finished(api, &d.id, timeout).await;
    let mut stream = api.deployment_logs(&d.id, false).await.unwrap();
    let mut log = Vec::new();
    while let Some(line) = stream.next().await {
        log.push(line.unwrap().line);
    }
    assert_eq!(
        d.status,
        DeploymentStatus::Live,
        "{d:#?}\n{}",
        log.join("\n")
    );
    (d, log)
}

async fn logs(api: &Client, service_id: &str, container: Option<&str>, want: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut stream = api
            .service_logs(service_id, 100, false, container)
            .await
            .unwrap();
        let mut out = String::new();
        while let Some(line) = stream.next().await {
            out.push_str(&line.unwrap().line);
            out.push('\n');
        }
        if out.contains(want) || Instant::now() > deadline {
            return out;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn dockerfile_builds_the_target_stage_with_env_as_build_args() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let env = environment(&api).await;
    let root = tempfile::tempdir().unwrap();
    let repo = fixture(
        root.path(),
        &[(
            "Dockerfile",
            "FROM busybox:1.37 AS base\nARG GREETING=unset\nRUN echo \"built with $GREETING\" > /greeting\n\
             FROM base AS prod\nCMD [\"sh\", \"-c\", \"cat /greeting; exec httpd -f -p 8080 -h /\"]\n\
             FROM base AS broken\nCMD [\"false\"]\n",
        )],
    );
    let mut spec = git_service(&env, "web", repo, BuildMethod::Auto, 8080);
    if let Source::Git { build, .. } = &mut spec.source {
        build.target = Some("prod".into());
    }
    let svc = api.create_service(&spec).await.unwrap();
    let (d, log) = deploy(&api, &svc.service.id, Duration::from_secs(300)).await;
    assert!(
        log.iter()
            .any(|l| l.contains("Detected the dockerfile build method")),
        "{log:#?}"
    );
    assert!(d.image.is_some());
    let detail = api.service(&svc.service.id).await.unwrap();
    let Source::Git { build, .. } = &detail.service.source else {
        panic!()
    };
    assert_eq!(
        build.method,
        BuildMethod::Dockerfile,
        "auto was saved as what it found"
    );
    assert_eq!(build.target.as_deref(), Some("prod"));
    assert!(detail.compose.is_empty());
    let out = logs(&api, &svc.service.id, None, "built with").await;
    assert!(out.contains("built with hello-from-env"), "{out}");
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn compose_deploys_a_group_with_routes_and_named_volumes() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let env = environment(&api).await;
    let root = tempfile::tempdir().unwrap();
    let repo = fixture(
        root.path(),
        &[
            (
                "compose.yaml",
                "services:\n  web:\n    build: ./web\n    environment:\n      GREETING: ${GREETING}\n  \
                 worker:\n    image: busybox:1.37\n    command: [\"sh\", \"-c\", \"[ -f /data/first ] || date +%s%N > /data/first; echo first=$$(cat /data/first); exec sleep 3600\"]\n    \
                 volumes:\n      - data:/data\nvolumes:\n  data: {}\n",
            ),
            (
                "web/Dockerfile",
                "FROM busybox:1.37\nCMD [\"sh\", \"-c\", \"echo web says $GREETING; exec httpd -f -p 8080 -h /\"]\n",
            ),
        ],
    );
    let svc = api
        .create_service(&git_service(&env, "stack", repo, BuildMethod::Auto, 8080))
        .await
        .unwrap();
    let sid = svc.service.id.clone();
    let domain = |compose_service: Option<&str>| CreateDomain {
        host: "stack.localhost".into(),
        port: None,
        tls: false,
        compose_service: compose_service.map(Into::into),
    };

    let err = api
        .add_domain(&sid, &domain(Some("web")))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not a compose build"), "{err}");

    let (_, log) = deploy(&api, &sid, Duration::from_secs(300)).await;
    assert!(
        log.iter()
            .any(|l| l.contains("Detected the compose build method from compose.yaml")),
        "{log:#?}"
    );
    let detail = api.service(&sid).await.unwrap();
    let names: Vec<_> = detail.compose.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["web", "worker"]);
    assert!(
        detail
            .compose
            .iter()
            .all(|c| c.container.as_ref().is_some_and(|c| c.state == "running")),
        "{:#?}",
        detail.compose
    );
    assert_eq!(detail.service.state, ServiceState::Running);
    let web = logs(&api, &sid, Some("web"), "web says").await;
    assert!(web.contains("web says hello-from-env"), "{web}");
    let first = logs(&api, &sid, Some("worker"), "first=").await;
    assert!(first.contains("first="), "{first}");

    // A domain names the compose service it routes to and redeploys the group.
    let err = api.add_domain(&sid, &domain(None)).await.unwrap_err();
    assert!(err.to_string().contains("compose service"), "{err}");
    api.add_domain(&sid, &domain(Some("web"))).await.unwrap();
    let redeploy = api.deployments(&sid, 1).await.unwrap().remove(0);
    assert_eq!(redeploy.trigger, "config");
    let redeploy = wait_finished(&api, &redeploy.id, Duration::from_secs(300)).await;
    assert_eq!(redeploy.status, DeploymentStatus::Live, "{redeploy:#?}");
    let project = format!("{}-{sid}", brand::PREFIX);
    let web_id = run(
        Path::new("."),
        "docker",
        &[
            "ps",
            "-q",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
            "--filter",
            "label=com.docker.compose.service=web",
        ],
    );
    let labels = run(
        Path::new("."),
        "docker",
        &[
            "inspect",
            "--format",
            "{{json .Config.Labels}}",
            web_id.trim(),
        ],
    );
    assert!(
        labels.contains("\"caddy_0\":\"http://stack.localhost\""),
        "{labels}"
    );
    assert!(labels.contains("{{upstreams 8080}}"), "{labels}");
    assert!(
        labels.contains(&format!("\"{}\":\"{sid}\"", brand::label("service"))),
        "{labels}"
    );
    let again = logs(&api, &sid, Some("worker"), "first=").await;
    let marker = |s: &str| {
        s.lines()
            .find(|l| l.starts_with("first="))
            .map(String::from)
    };
    assert_eq!(
        marker(&again),
        marker(&first),
        "the named volume survived the redeploy"
    );

    // Stop and start act on every container of the group.
    api.stop(&sid).await.unwrap();
    let stopped = api.service(&sid).await.unwrap();
    assert!(
        stopped
            .compose
            .iter()
            .all(|c| c.container.as_ref().is_some_and(|c| c.state == "exited")),
        "{:#?}",
        stopped.compose
    );
    api.start(&sid).await.unwrap();

    api.delete_service(&sid, true).await.unwrap();
    let left = run(
        Path::new("."),
        "docker",
        &[
            "volume",
            "ls",
            "-q",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ],
    );
    assert!(left.trim().is_empty(), "volumes left: {left}");
}

#[tokio::test]
#[ignore = "needs Docker and network access for Railpack"]
async fn railpack_builds_a_node_app_without_a_dockerfile() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let env = environment(&api).await;
    let root = tempfile::tempdir().unwrap();
    let repo = fixture(
        root.path(),
        &[
            (
                "package.json",
                "{\"name\":\"hello\",\"version\":\"1.0.0\",\"scripts\":{\"start\":\"node index.js\"}}\n",
            ),
            (
                "index.js",
                "const http = require('http');\nconsole.log('node says ' + process.env.GREETING);\n\
                 http.createServer((_, res) => res.end('ok')).listen(process.env.PORT || 3000);\n",
            ),
        ],
    );
    let svc = api
        .create_service(&git_service(&env, "node", repo, BuildMethod::Auto, 3000))
        .await
        .unwrap();
    let (d, log) = deploy(&api, &svc.service.id, Duration::from_secs(900)).await;
    assert!(
        log.iter()
            .any(|l| l.contains("Detected the railpack build method")),
        "{log:#?}"
    );
    assert!(
        log.iter().any(|l| l.contains("Railpack detected")),
        "{log:#?}"
    );
    assert!(
        d.image
            .as_deref()
            .is_some_and(|i| i.starts_with(brand::PREFIX)),
        "{d:?}"
    );
    let out = logs(&api, &svc.service.id, None, "node says").await;
    assert!(out.contains("node says hello-from-env"), "{out}");
}
