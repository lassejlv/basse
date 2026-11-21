//! The three build methods against a real Docker engine, each with a small fixture repository:
//! a multi-stage Dockerfile with a target and build args, a compose project with a routed
//! service and a named volume, and a Node app built by Railpack; and a monorepo whose apps
//! live in subdirectories, one per method, deployed as services with different roots; and compose
//! bind mounts that keep their data and refresh their config across redeploys. Run with
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

/// A compose service with a config-file bind and a data-dir bind. Each boot counts itself in the
/// data directory and prints the config file and a file the repository seeded the data with.
const MOUNTS_COMPOSE: &str = r#"services:
  app:
    image: busybox:1.37
    command: ["sh", "-c", "n=$$(( $$(cat /data/boots 2>/dev/null || echo 0) + 1 )); echo $$n > /data/boots; echo boot=$$n greeting=$$(cat /etc/app/greeting.conf) seed=$$(cat /data/seed.txt); exec sleep 3600"]
    volumes:
      - ./config/greeting.conf:/etc/app/greeting.conf:ro
      - ./data:/data
"#;

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
            root: String::new(),
            watch_paths: true,
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

/// A monorepo with a decoy app at its root and one app per build method under `apps/`.
fn monorepo(root: &Path) -> String {
    fixture(
        root,
        &[
            (
                "Dockerfile",
                "FROM busybox:1.37\nCMD [\"sh\", \"-c\", \"echo the repository root was built; exec sleep 3600\"]\n",
            ),
            (
                "apps/api/Dockerfile",
                "FROM busybox:1.37\nARG GREETING=unset\nRUN echo \"api built with $GREETING\" > /greeting\n\
                 CMD [\"sh\", \"-c\", \"cat /greeting; exec httpd -f -p 8080 -h /\"]\n",
            ),
            (
                "apps/web/compose.yaml",
                "services:\n  web:\n    build: ./site\n    env_file: web.env\n",
            ),
            ("apps/web/web.env", "WEB_GREETING=from-the-env-file\n"),
            (
                "apps/web/site/Dockerfile",
                "FROM busybox:1.37\nCMD [\"sh\", \"-c\", \"echo web says $WEB_GREETING; exec httpd -f -p 8080 -h /\"]\n",
            ),
            (
                "apps/worker/package.json",
                "{\"name\":\"worker\",\"version\":\"1.0.0\",\"scripts\":{\"start\":\"node index.js\"}}\n",
            ),
            (
                "apps/worker/index.js",
                "const http = require('http');\nconsole.log('worker says ' + process.env.GREETING);\n\
                 http.createServer((_, res) => res.end('ok')).listen(process.env.PORT || 3000);\n",
            ),
        ],
    )
}

fn in_root(mut spec: CreateService, dir: &str) -> CreateService {
    if let Source::Git { root, .. } = &mut spec.source {
        *root = dir.into();
    }
    spec
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
    let daemon = Daemon::start_on_host()
        .await
        .expect("these tests need Docker");
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
    let daemon = Daemon::start_on_host()
        .await
        .expect("these tests need Docker");
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
    let daemon = Daemon::start_on_host()
        .await
        .expect("these tests need Docker");
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

#[tokio::test]
#[ignore = "needs Docker"]
async fn monorepo_services_build_the_app_in_their_root() {
    let daemon = Daemon::start_on_host()
        .await
        .expect("these tests need Docker");
    let api = daemon.client();
    let env = environment(&api).await;
    let root = tempfile::tempdir().unwrap();
    let repo = monorepo(root.path());

    let app = api
        .create_service(&in_root(
            git_service(&env, "api", repo.clone(), BuildMethod::Auto, 8080),
            "./apps/api/",
        ))
        .await
        .unwrap();
    let Source::Git { root: saved, .. } = &app.service.source else {
        panic!("not git");
    };
    assert_eq!(saved, "apps/api", "the root is normalized");
    let (_, log) = deploy(&api, &app.service.id, Duration::from_secs(300)).await;
    assert!(
        log.iter().any(|l| l.contains("Building in apps/api"))
            && log
                .iter()
                .any(|l| l.contains("Detected the dockerfile build method from Dockerfile")),
        "{log:#?}"
    );
    let out = logs(&api, &app.service.id, None, "built with").await;
    assert!(out.contains("api built with hello-from-env"), "{out}");
    assert!(!out.contains("repository root"), "{out}");

    let web = api
        .create_service(&in_root(
            git_service(&env, "web", repo.clone(), BuildMethod::Auto, 8080),
            "apps/web",
        ))
        .await
        .unwrap();
    let (_, log) = deploy(&api, &web.service.id, Duration::from_secs(300)).await;
    assert!(
        log.iter()
            .any(|l| l.contains("Detected the compose build method from compose.yaml")),
        "{log:#?}"
    );
    let detail = api.service(&web.service.id).await.unwrap();
    let names: Vec<_> = detail.compose.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["web"]);
    let out = logs(&api, &web.service.id, Some("web"), "web says").await;
    assert!(
        out.contains("web says from-the-env-file"),
        "the build context and env file resolve in the root: {out}"
    );

    // The Dockerfile method with an explicit context and Dockerfile, both relative to the root.
    let mut spec = in_root(
        git_service(&env, "site", repo.clone(), BuildMethod::Dockerfile, 8080),
        "apps/web",
    );
    if let Source::Git { context, .. } = &mut spec.source {
        *context = "site".into();
    }
    let site = api.create_service(&spec).await.unwrap();
    deploy(&api, &site.service.id, Duration::from_secs(300)).await;
    let out = logs(&api, &site.service.id, None, "web says").await;
    assert!(out.contains("web says"), "{out}");

    // A root the branch does not have fails the deployment before building.
    let missing = api
        .create_service(&in_root(
            git_service(&env, "missing", repo, BuildMethod::Auto, 8080),
            "apps/nope",
        ))
        .await
        .unwrap();
    let d = api.deploy(&missing.service.id, "test").await.unwrap();
    let d = wait_finished(&api, &d.id, Duration::from_secs(120)).await;
    assert_eq!(d.status, DeploymentStatus::Failed, "{d:#?}");
    let mut stream = api.deployment_logs(&d.id, false).await.unwrap();
    let mut log = Vec::new();
    while let Some(line) = stream.next().await {
        log.push(line.unwrap().line);
    }
    assert!(
        log.iter()
            .any(|l| l.contains("root directory 'apps/nope' does not exist")),
        "{log:#?}"
    );

    for svc in [&app, &web, &site, &missing] {
        api.delete_service(&svc.service.id, true).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "needs Docker and network access for Railpack"]
async fn railpack_builds_the_app_in_a_monorepo_root() {
    let daemon = Daemon::start_on_host()
        .await
        .expect("these tests need Docker");
    let api = daemon.client();
    let env = environment(&api).await;
    let root = tempfile::tempdir().unwrap();
    let repo = monorepo(root.path());
    let svc = api
        .create_service(&in_root(
            git_service(&env, "worker", repo, BuildMethod::Auto, 3000),
            "apps/worker",
        ))
        .await
        .unwrap();
    let (_, log) = deploy(&api, &svc.service.id, Duration::from_secs(900)).await;
    assert!(
        log.iter()
            .any(|l| l.contains("Detected the railpack build method")),
        "{log:#?}"
    );
    let out = logs(&api, &svc.service.id, None, "worker says").await;
    assert!(out.contains("worker says hello-from-env"), "{out}");
    api.delete_service(&svc.service.id, true).await.unwrap();
}

/// Commits `files` in the fixture's work tree and pushes them to its bare repository.
fn commit(root: &Path, files: &[(&str, &str)]) {
    let work = root.join("work");
    for (path, body) in files {
        std::fs::write(work.join(path), body).unwrap();
    }
    run(&work, "git", &["commit", "-q", "-am", "change"]);
    let bare = root.join("repo.git");
    run(
        &work,
        "git",
        &["push", "-q", bare.to_str().unwrap(), "main"],
    );
}

fn docker(args: &[&str]) -> Result<String, String> {
    let out = Command::new("docker").args(args).output().unwrap();
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn compose_bind_mounts_keep_data_and_refresh_config() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let env = environment(&api).await;
    let root = tempfile::tempdir().unwrap();
    let repo = fixture(
        root.path(),
        &[
            ("compose.yaml", MOUNTS_COMPOSE),
            ("config/greeting.conf", "hello-v1\n"),
            ("data/seed.txt", "from-the-repository\n"),
        ],
    );
    let svc = api
        .create_service(&git_service(
            &env,
            "stack",
            repo,
            BuildMethod::Compose,
            8080,
        ))
        .await
        .unwrap()
        .service;

    let (d, log) = deploy(&api, &svc.id, Duration::from_secs(300)).await;
    for want in [
        "Copying ./config/greeting.conf from the repository",
        "Mounting ./data from volume",
        "filled from the repository",
    ] {
        assert!(log.iter().any(|l| l.contains(want)), "{want}: {log:#?}");
    }
    assert!(
        !daemon.dir.path().join("builds").join(&d.id).exists(),
        "the clone outlived the deploy"
    );
    let out = logs(&api, &svc.id, Some("app"), "boot=1").await;
    assert!(
        out.contains("boot=1 greeting=hello-v1 seed=from-the-repository"),
        "{out}"
    );

    let (_, log) = deploy(&api, &svc.id, Duration::from_secs(300)).await;
    assert!(
        log.iter().any(|l| l.contains("kept from earlier deploys")),
        "{log:#?}"
    );
    let out = logs(&api, &svc.id, Some("app"), "boot=2").await;
    assert!(out.contains("boot=2 greeting=hello-v1"), "{out}");

    // A config change shows up; the data directory keeps what the service wrote, and the
    // repository's copy of it is not copied over it again.
    commit(
        root.path(),
        &[
            ("config/greeting.conf", "hello-v2\n"),
            ("data/seed.txt", "changed-in-the-repository\n"),
        ],
    );
    deploy(&api, &svc.id, Duration::from_secs(300)).await;
    let out = logs(&api, &svc.id, Some("app"), "boot=3").await;
    assert!(
        out.contains("boot=3 greeting=hello-v2 seed=from-the-repository"),
        "{out}"
    );

    let detail = api.service(&svc.id).await.unwrap();
    let rows: Vec<_> = detail
        .volumes
        .iter()
        .map(|v| (v.name.as_str(), v.mount_path.as_str(), v.compose.clone()))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "data",
                "/data",
                Some(ComposeVolume::Data {
                    path: "data".into()
                })
            ),
            (
                "files",
                "/etc/app/greeting.conf",
                Some(ComposeVolume::Files {
                    paths: vec!["config/greeting.conf".into()]
                })
            ),
        ]
    );
    let (data, files) = (
        detail.volumes[0].docker_name(),
        detail.volumes[1].docker_name(),
    );
    let copies = docker(&[
        "run",
        "--rm",
        "-v",
        &format!("{files}:/files"),
        "busybox:1.37",
        "ls",
        "/files",
    ])
    .unwrap();
    assert_eq!(
        copies.split_whitespace().collect::<Vec<_>>(),
        ["2", "3"],
        "only the live deploy's and the newest copy are kept"
    );

    api.delete_service(&svc.id, false).await.unwrap();
    assert!(
        docker(&["volume", "inspect", &files]).is_err(),
        "the config copies outlived the service"
    );
    assert!(
        docker(&["volume", "inspect", &data]).is_ok(),
        "data is kept unless the delete asks for it"
    );
    docker(&["volume", "rm", &data]).unwrap();
}
