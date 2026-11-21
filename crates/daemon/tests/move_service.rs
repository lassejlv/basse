//! Moving services between the daemon's own engine and a remote SSH server with its own Docker
//! engine (Docker-in-Docker with sshd). Run with
//! `cargo test -p daemon --test move_service -- --ignored --test-threads 1`.

mod support;

use client::Client;
use common::api::*;
use common::*;
use futures_util::StreamExt as _;
use std::path::Path;
use std::time::{Duration, Instant};
use support::Daemon;
use support::remote::{RemoteHost, run};

fn label_filter(service_id: &str) -> String {
    format!("label={}={service_id}", brand::label("service"))
}

/// Names of a service's running containers on the daemon's own engine.
fn local_running(service_id: &str) -> Vec<String> {
    run(
        Path::new("."),
        "docker",
        &[
            "ps",
            "--filter",
            &label_filter(service_id),
            "--format",
            "{{.Names}}",
        ],
    )
    .lines()
    .map(String::from)
    .collect()
}

/// Every container of a service on the daemon's own engine, running or not.
fn local_all(service_id: &str) -> Vec<String> {
    run(
        Path::new("."),
        "docker",
        &[
            "ps",
            "-a",
            "--filter",
            &label_filter(service_id),
            "--format",
            "{{.Names}}",
        ],
    )
    .lines()
    .map(String::from)
    .collect()
}

fn remote_running(remote: &RemoteHost, service_id: &str) -> Vec<String> {
    remote
        .docker(&[
            "ps",
            "--filter",
            &label_filter(service_id),
            "--format",
            "{{.Names}}",
        ])
        .lines()
        .map(String::from)
        .collect()
}

fn remote_all(remote: &RemoteHost, service_id: &str) -> Vec<String> {
    remote
        .docker(&[
            "ps",
            "-a",
            "--filter",
            &label_filter(service_id),
            "--format",
            "{{.Names}}",
        ])
        .lines()
        .map(String::from)
        .collect()
}

/// Waits for a deployment to finish; returns it with its log.
async fn settle(api: &Client, d: Deployment, timeout: Duration) -> (Deployment, Vec<String>) {
    let deadline = Instant::now() + timeout;
    let d = loop {
        let d = api.deployment(&d.id).await.unwrap();
        let settled = d
            .steps
            .iter()
            .all(|s| !matches!(s.status, StepStatus::Pending | StepStatus::Running));
        if d.status.is_finished() && settled {
            break d;
        }
        assert!(
            Instant::now() < deadline,
            "#{} did not finish: {d:?}",
            d.number
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    let mut stream = api.deployment_logs(&d.id, false).await.unwrap();
    let mut log = Vec::new();
    while let Some(line) = stream.next().await {
        log.push(line.unwrap().line);
    }
    (d, log)
}

async fn live(api: &Client, d: Deployment) -> (Deployment, Vec<String>) {
    let (d, log) = settle(api, d, Duration::from_secs(300)).await;
    assert_eq!(
        d.status,
        DeploymentStatus::Live,
        "{d:#?}\n{}",
        log.join("\n")
    );
    (d, log)
}

fn step(d: &Deployment, kind: StepKind) -> &Step {
    d.steps.iter().find(|s| s.kind == kind).unwrap()
}

/// Runs `sh -c script` in a container on the daemon's engine, retrying until it succeeds.
fn local_sh(container: &str, script: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let out = std::process::Command::new("docker")
            .args(["exec", container, "sh", "-c", script])
            .output()
            .unwrap();
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
        assert!(
            Instant::now() < deadline,
            "{script}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn remote_sh(remote: &RemoteHost, container: &str, script: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match remote.try_docker(&["exec", container, "sh", "-c", script]) {
            Ok(out) => return out,
            Err(e) => assert!(Instant::now() < deadline, "{script}: {e}"),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[tokio::test]
#[ignore = "needs Docker with privileged containers"]
async fn move_service() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let remote = RemoteHost::start(&daemon.dir.path().join("ssh/id_ed25519.pub"));
    let far = api
        .create_server(&CreateServer {
            name: "remote".into(),
            host: "127.0.0.1".into(),
            user: "root".into(),
            port: remote.port,
            ssh_key_secret_id: None,
        })
        .await
        .unwrap();
    assert_eq!(
        api.check_server(&far.id).await.unwrap().status,
        ServerStatus::Online
    );
    let home = api
        .servers()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.kind == ServerKind::Local)
        .unwrap();
    api.create_project(&CreateProject {
        name: "moving".into(),
        description: None,
    })
    .await
    .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();

    // An image service whose page lives on a volume, seeded once with a random marker.
    let web = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: Some(home.id.clone()),
            name: "web".into(),
            source: Source::Image {
                image: "busybox:1.37".into(),
            },
            port: Some(8080),
            command: Some(
                "[ -f /www/index.html ] || echo \"seed-$RANDOM-$RANDOM\" > /www/index.html; \
                 exec httpd -f -p 8080 -h /www"
                    .into(),
            ),
            restart: RestartPolicy::UnlessStopped,
            health_path: Some("/".into()),
            env: Vec::new(),
            domains: Vec::new(),
            volumes: vec![VolumeSpec {
                name: "www".into(),
                mount_path: "/www".into(),
            }],
            deploy: false,
        })
        .await
        .unwrap()
        .service;
    live(&api, api.deploy(&web.id, "test").await.unwrap()).await;
    let here = local_running(&web.id);
    assert_eq!(here.len(), 1, "{here:?}");
    let marker = local_sh(&here[0], "cat /www/index.html");
    assert!(marker.starts_with("seed-"), "{marker}");

    // Same server: refused.
    let err = api
        .move_service(
            &web.id,
            &MoveService {
                server_id: home.id.clone(),
                copy_volumes: true,
            },
        )
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("already runs on"), "{err:#}");

    // Move to the remote engine with the volume.
    let d = api
        .move_service(
            &web.id,
            &MoveService {
                server_id: far.id.clone(),
                copy_volumes: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(d.trigger, "move");
    let (d, log) = live(&api, d).await;
    let kinds: Vec<StepKind> = d.steps.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            StepKind::Transfer,
            StepKind::Start,
            StepKind::Health,
            StepKind::Switch,
            StepKind::Cleanup
        ]
    );
    let transfer = step(&d, StepKind::Transfer).detail.clone().unwrap();
    assert_eq!(transfer, "image + volumes");
    assert!(
        log.iter().any(|l| l.starts_with("Copied 1 volume (")),
        "{log:#?}"
    );
    assert!(
        log.iter().any(|l| l.contains("Moving web from server")),
        "{log:#?}"
    );
    let detail = api.service(&web.id).await.unwrap();
    assert_eq!(detail.service.server_id, far.id);
    assert_eq!(detail.server.id, far.id);
    assert!(local_all(&web.id).is_empty(), "{:?}", local_all(&web.id));
    let there = remote_running(&remote, &web.id);
    assert_eq!(there.len(), 1, "{there:?}");
    assert_eq!(remote_sh(&remote, &there[0], "cat /www/index.html"), marker);
    assert_eq!(
        remote_sh(&remote, &there[0], "wget -q -O - http://127.0.0.1:8080/"),
        marker
    );

    // A move back that fails after stopping the remote container: the volume of that name on
    // the daemon's engine is held by another container, so it cannot be replaced. The move is
    // undone and the remote container serves again.
    remote_sh(
        &remote,
        &there[0],
        "echo changed-on-remote > /www/index.html",
    );
    let volume = format!("{}-{}-www", brand::PREFIX, web.id);
    let blocker = format!("{}-test-blocker-{}", brand::PREFIX, std::process::id());
    run(
        Path::new("."),
        "docker",
        &[
            "run",
            "-d",
            "--name",
            &blocker,
            "-v",
            &format!("{volume}:/held"),
            "busybox:1.37",
            "sleep",
            "600",
        ],
    );
    let d = api
        .move_service(
            &web.id,
            &MoveService {
                server_id: home.id.clone(),
                copy_volumes: true,
            },
        )
        .await
        .unwrap();
    let (d, log) = settle(&api, d, Duration::from_secs(300)).await;
    run(Path::new("."), "docker", &["rm", "-f", &blocker]);
    assert_eq!(d.status, DeploymentStatus::Failed, "{}", log.join("\n"));
    assert_eq!(step(&d, StepKind::Transfer).status, StepStatus::Failed);
    assert!(
        log.iter().any(|l| l.contains("Stopping 1 container")),
        "{log:#?}"
    );
    assert!(log.iter().any(|l| l.contains("Move undone")), "{log:#?}");
    assert_eq!(
        api.service(&web.id).await.unwrap().service.server_id,
        far.id
    );
    assert!(local_all(&web.id).is_empty(), "{:?}", local_all(&web.id));
    assert_eq!(remote_running(&remote, &web.id), there);
    assert_eq!(
        remote_sh(&remote, &there[0], "wget -q -O - http://127.0.0.1:8080/"),
        "changed-on-remote"
    );
    let detail = api.service(&web.id).await.unwrap();
    assert_eq!(detail.service.state, ServiceState::Running, "{detail:?}");

    // With the blocker gone, the same move replaces the old volume left on the daemon's
    // engine with the remote's current data.
    let d = api
        .move_service(
            &web.id,
            &MoveService {
                server_id: home.id.clone(),
                copy_volumes: true,
            },
        )
        .await
        .unwrap();
    let (_, log) = live(&api, d).await;
    assert!(
        log.iter()
            .any(|l| l.contains(&format!("Replacing volume {volume}"))),
        "{log:#?}"
    );
    assert_eq!(
        api.service(&web.id).await.unwrap().service.server_id,
        home.id
    );
    assert!(remote_all(&remote, &web.id).is_empty());
    let here = local_running(&web.id);
    assert_eq!(
        local_sh(&here[0], "cat /www/index.html"),
        "changed-on-remote"
    );

    // A database always brings its data, and the files keep the owner Postgres demands.
    let pg = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: Some(home.id.clone()),
            name: "pg".into(),
            source: Source::Database {
                kind: DatabaseKind::Postgres,
                version: DatabaseKind::Postgres.default_version().into(),
            },
            port: None,
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: None,
            env: Vec::new(),
            domains: Vec::new(),
            volumes: Vec::new(),
            deploy: true,
        })
        .await
        .unwrap()
        .service;
    let first = api.deployments(&pg.id, 1).await.unwrap().remove(0);
    live(&api, first).await;
    let psql = "psql -v ON_ERROR_STOP=1 -U \"$POSTGRES_USER\" -d \"$POSTGRES_DB\" -tAc";
    let here = local_running(&pg.id);
    local_sh(
        &here[0],
        &format!("{psql} \"CREATE TABLE moved (v text); INSERT INTO moved VALUES ('kept')\""),
    );
    let d = api
        .move_service(
            &pg.id,
            &MoveService {
                server_id: far.id.clone(),
                copy_volumes: false,
            },
        )
        .await
        .unwrap();
    let (d, _) = live(&api, d).await;
    assert!(
        step(&d, StepKind::Transfer)
            .detail
            .as_deref()
            .is_some_and(|t| t.ends_with("volumes")),
        "{d:?}"
    );
    assert!(local_all(&pg.id).is_empty());
    let there = remote_running(&remote, &pg.id);
    assert_eq!(
        remote_sh(
            &remote,
            &there[0],
            &format!("{psql} \"SELECT v FROM moved\"")
        ),
        "kept"
    );
}
