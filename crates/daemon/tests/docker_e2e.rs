//! End-to-end flow against a real Docker engine. Binds ports 80/443 for the proxy.
//! Run with `cargo test -p daemon --test docker_e2e -- --ignored --nocapture`.

mod support;

use client::Client;
use common::api::*;
use common::*;
use futures_util::StreamExt as _;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use support::Daemon;

const WHOAMI_HOST: &str = "whoami.localhost";

async fn wait_finished(api: &Client, deployment_id: &str, timeout: Duration) -> Deployment {
    let deadline = Instant::now() + timeout;
    loop {
        let d = api.deployment(deployment_id).await.unwrap();
        let settled = d
            .steps
            .iter()
            .all(|s| !matches!(s.status, StepStatus::Pending | StepStatus::Running));
        if d.status.is_finished() && settled {
            return d;
        }
        assert!(
            Instant::now() < deadline,
            "deployment #{} did not finish: {d:?}",
            d.number
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn wait_state(api: &Client, service_id: &str, want: ServiceState) -> ServiceDetail {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let detail = api.service(service_id).await.unwrap();
        if detail.service.state == want {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "service never reached {want:?}: {:?}",
            detail.service.state
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn latest_deployment(api: &Client, service_id: &str) -> Deployment {
    api.deployments(service_id, 1).await.unwrap().remove(0)
}

async fn collect_logs(mut stream: client::LogStreamBox) -> Vec<LogLine> {
    let mut lines = Vec::new();
    while let Some(line) = stream.next().await {
        lines.push(line.unwrap());
    }
    lines
}

async fn whoami(http: &reqwest::Client) -> Option<String> {
    let res = http
        .get("http://127.0.0.1/")
        .header("Host", WHOAMI_HOST)
        .send()
        .await
        .ok()?;
    res.status().is_success().then_some(())?;
    res.text().await.ok()
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn service_containers(service_id: &str) -> Vec<String> {
    let filter = format!("label={}={service_id}", brand::label("service"));
    docker(&["ps", "-a", "-q", "--filter", &filter])
        .lines()
        .map(str::to_string)
        .collect()
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A bare repository with a tiny static site, served over `file://`.
fn local_repo(root: &Path) -> String {
    let work = root.join("work");
    std::fs::create_dir_all(work.join("site")).unwrap();
    std::fs::write(
        work.join("Dockerfile"),
        "FROM busybox:1.36\nCOPY site /www\nCMD [\"httpd\", \"-f\", \"-p\", \"8080\", \"-h\", \"/www\"]\n",
    )
    .unwrap();
    std::fs::write(work.join("site/index.html"), "hello from git\n").unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "Initial site"]);
    let bare = root.join("site.git");
    git(
        root,
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

#[tokio::test]
#[ignore = "needs Docker and free ports 80/443"]
async fn docker_end_to_end() {
    let daemon = Daemon::start().await;
    let api = daemon.client();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let local = api.servers().await.unwrap().remove(0);

    let project = api
        .create_project(&CreateProject {
            name: "e2e".into(),
            description: None,
        })
        .await
        .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();

    // Image service deployed on create, then a domain added live (reconfigure deployment).
    println!("== whoami: create + deploy");
    let created = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: None,
            name: "whoami".into(),
            source: Source::Image {
                image: "traefik/whoami".into(),
            },
            port: Some(80),
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: Some("/health".into()),
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: true,
        })
        .await
        .unwrap();
    let sid = created.service.id.clone();
    let first = wait_finished(
        &api,
        &latest_deployment(&api, &sid).await.id,
        Duration::from_secs(180),
    )
    .await;
    assert_eq!(first.status, DeploymentStatus::Live, "{first:?}");
    assert_eq!(first.image.as_deref(), Some("traefik/whoami:latest"));
    assert!(
        first.steps.iter().all(|s| s.status == StepStatus::Done),
        "{:?}",
        first.steps
    );

    println!("== proxy");
    let server = api.setup_proxy(&local.id).await.unwrap();
    assert_eq!(server.proxy, ProxyStatus::Running);
    assert_eq!(server.status, ServerStatus::Online);
    assert_eq!(api.info().await.unwrap().proxy, ProxyStatus::Running);

    println!("== domain added live");
    api.add_domain(
        &sid,
        &CreateDomain {
            host: WHOAMI_HOST.into(),
            port: None,
            tls: false,
        },
    )
    .await
    .unwrap();
    let reconfig = latest_deployment(&api, &sid).await;
    assert_eq!(reconfig.trigger, "config");
    let reconfig = wait_finished(&api, &reconfig.id, Duration::from_secs(120)).await;
    assert_eq!(reconfig.status, DeploymentStatus::Live, "{reconfig:?}");
    let deadline = Instant::now() + Duration::from_secs(30);
    let body = loop {
        if let Some(body) = whoami(&http).await {
            break body;
        }
        assert!(
            Instant::now() < deadline,
            "proxy never routed {WHOAMI_HOST}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert!(body.contains("Hostname:"), "{body}");
    println!(
        "proxy answered:\n{}",
        body.lines().take(2).collect::<Vec<_>>().join("\n")
    );
    assert_eq!(service_containers(&sid).len(), 1);
    let detail = wait_state(&api, &sid, ServiceState::Running).await;
    let container = detail.container.unwrap();
    assert!(
        container
            .networks
            .contains(&brand::PROXY_NETWORK.to_string()),
        "{container:?}"
    );

    println!("== service logs");
    let lines = collect_logs(api.service_logs(&sid, 50, false).await.unwrap()).await;
    assert!(
        lines
            .iter()
            .any(|l| l.line.contains("Starting up on port 80")),
        "{lines:?}"
    );
    assert!(lines.iter().all(|l| l.ts.is_some()));
    let mut follow = api.service_logs(&sid, 5, true).await.unwrap();
    let first_line = tokio::time::timeout(Duration::from_secs(5), follow.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    println!("followed: {first_line:?}");
    drop(follow);

    println!("== redeploy with traffic running");
    let stop = Arc::new(AtomicBool::new(false));
    let (ok, failed) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let prober = tokio::spawn({
        let (stop, ok, failed, http) = (stop.clone(), ok.clone(), failed.clone(), http.clone());
        async move {
            while !stop.load(Ordering::Relaxed) {
                let res = http
                    .get("http://127.0.0.1/")
                    .header("Host", WHOAMI_HOST)
                    .send()
                    .await;
                match res {
                    Ok(r) if r.status().is_success() => ok.fetch_add(1, Ordering::Relaxed),
                    other => {
                        println!("probe failed: {:?}", other.map(|r| r.status()));
                        failed.fetch_add(1, Ordering::Relaxed)
                    }
                };
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });
    let redeploy = api.deploy(&sid, "cli").await.unwrap();
    let log = collect_logs(api.deployment_logs(&redeploy.id, true).await.unwrap()).await;
    let redeploy = api.deployment(&redeploy.id).await.unwrap();
    assert_eq!(redeploy.status, DeploymentStatus::Live, "{redeploy:?}");
    assert!(
        log.iter().any(|l| l.line == "==> pull") && log.iter().any(|l| l.line.contains("is live"))
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    stop.store(true, Ordering::Relaxed);
    prober.await.unwrap();
    let (ok, failed) = (ok.load(Ordering::Relaxed), failed.load(Ordering::Relaxed));
    println!("requests during redeploy: {ok} ok, {failed} failed");
    assert!(ok > 10);
    assert_eq!(failed, 0, "zero-downtime redeploy dropped requests");
    assert_eq!(
        service_containers(&sid).len(),
        1,
        "old container was not removed"
    );
    let previous = api.deployment(&reconfig.id).await.unwrap();
    assert_eq!(previous.status, DeploymentStatus::Superseded);

    println!("== rollback");
    let rollback = api.rollback(&previous.id).await.unwrap();
    assert_eq!(rollback.trigger, "rollback");
    let rollback = wait_finished(&api, &rollback.id, Duration::from_secs(120)).await;
    assert_eq!(rollback.status, DeploymentStatus::Live, "{rollback:?}");
    assert_eq!(rollback.image, previous.image);
    assert!(whoami(&http).await.is_some());

    println!("== stop / start");
    assert_eq!(
        api.stop(&sid).await.unwrap().service.state,
        ServiceState::Stopped
    );
    assert_eq!(
        api.start(&sid).await.unwrap().service.state,
        ServiceState::Running
    );
    assert_eq!(
        api.restart(&sid).await.unwrap().service.state,
        ServiceState::Running
    );

    println!("== git service");
    let repos = tempfile::tempdir().unwrap();
    let repo = local_repo(repos.path());
    let site = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: None,
            name: "site".into(),
            source: Source::Git {
                repo,
                branch: "main".into(),
                dockerfile: "Dockerfile".into(),
                context: ".".into(),
            },
            port: Some(8080),
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: Some("/".into()),
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: true,
        })
        .await
        .unwrap();
    let build = latest_deployment(&api, &site.service.id).await;
    let log = collect_logs(api.deployment_logs(&build.id, true).await.unwrap()).await;
    let build = api.deployment(&build.id).await.unwrap();
    assert_eq!(build.status, DeploymentStatus::Live, "{build:?}\n{log:#?}");
    assert_eq!(build.commit_message.as_deref(), Some("Initial site"));
    assert_eq!(build.commit.as_ref().map(String::len), Some(40));
    assert!(
        log.iter().any(|l| l.line.starts_with("Step 1/")),
        "{log:#?}"
    );
    let build_step = build
        .steps
        .iter()
        .find(|s| s.kind == StepKind::Build)
        .unwrap();
    println!(
        "build step detail: {:?}, image {:?}",
        build_step.detail, build.image
    );
    let health = build
        .steps
        .iter()
        .find(|s| s.kind == StepKind::Health)
        .unwrap();
    assert_eq!(health.detail.as_deref(), Some("GET / -> 200"));

    println!("== cancel");
    let doomed = api.deploy(&site.service.id, "cli").await.unwrap();
    let cancelled = api.cancel_deployment(&doomed.id).await.unwrap();
    assert_eq!(cancelled.status, DeploymentStatus::Cancelled);
    assert_eq!(
        api.service(&site.service.id)
            .await
            .unwrap()
            .service
            .live_deployment_id
            .as_deref(),
        Some(build.id.as_str())
    );
    assert_eq!(service_containers(&site.service.id).len(), 1);

    println!("== postgres");
    let db = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: None,
            name: "db".into(),
            source: Source::Database {
                kind: DatabaseKind::Postgres,
                version: String::new(),
            },
            port: None,
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: None,
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: true,
        })
        .await
        .unwrap();
    let pg = wait_finished(
        &api,
        &latest_deployment(&api, &db.service.id).await.id,
        Duration::from_secs(240),
    )
    .await;
    assert_eq!(pg.status, DeploymentStatus::Live, "{pg:?}");
    let pg_detail = wait_state(&api, &db.service.id, ServiceState::Running).await;
    let user = pg_detail
        .env
        .iter()
        .find(|v| v.key == "POSTGRES_USER")
        .unwrap()
        .value
        .clone();
    let probe = docker(&[
        "exec",
        &pg_detail.container.unwrap().id,
        "pg_isready",
        "-U",
        &user,
    ]);
    assert!(probe.contains("accepting connections"), "{probe}");

    println!("== failing deploy keeps nothing behind");
    let broken = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: None,
            name: "broken".into(),
            source: Source::Image {
                image: "busybox:1.36".into(),
            },
            port: None,
            command: Some("echo boom >&2; exit 3".into()),
            restart: RestartPolicy::No,
            health_path: None,
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: true,
        })
        .await
        .unwrap();
    let failed_deploy = wait_finished(
        &api,
        &latest_deployment(&api, &broken.service.id).await.id,
        Duration::from_secs(120),
    )
    .await;
    assert_eq!(failed_deploy.status, DeploymentStatus::Failed);
    assert!(
        failed_deploy
            .error
            .as_deref()
            .unwrap()
            .contains("exited with code 3"),
        "{failed_deploy:?}"
    );
    let log = collect_logs(api.deployment_logs(&failed_deploy.id, false).await.unwrap()).await;
    assert!(log.iter().any(|l| l.line == "boom"), "{log:#?}");
    assert!(service_containers(&broken.service.id).is_empty());
    assert_eq!(
        api.service(&broken.service.id).await.unwrap().service.state,
        ServiceState::Failed
    );
    api.delete_service(&broken.service.id, true).await.unwrap();

    println!("== crash after going live is detected");
    let crasher = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: None,
            name: "crasher".into(),
            source: Source::Image {
                image: "busybox:1.36".into(),
            },
            port: None,
            command: Some("sleep 7; exit 4".into()),
            restart: RestartPolicy::No,
            health_path: None,
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: true,
        })
        .await
        .unwrap();
    let live = wait_finished(
        &api,
        &latest_deployment(&api, &crasher.service.id).await.id,
        Duration::from_secs(60),
    )
    .await;
    assert_eq!(live.status, DeploymentStatus::Live, "{live:?}");
    let detail = wait_state(&api, &crasher.service.id, ServiceState::Failed).await;
    assert_eq!(detail.container.unwrap().exit_code, Some(4));
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let activity = api.activity(50).await.unwrap();
        if let Some(a) = activity.iter().find(|a| a.message == "Container crashed") {
            assert_eq!(a.detail.as_deref(), Some("exit code 4"));
            break;
        }
        assert!(Instant::now() < deadline, "crash was not recorded");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    api.delete_service(&crasher.service.id, true).await.unwrap();

    println!("== tree and activity");
    let tree = api.tree().await.unwrap();
    let states: Vec<_> = tree.projects[0].environments[0]
        .services
        .iter()
        .map(|s| (s.name.clone(), s.state))
        .collect();
    println!("{states:?}");
    assert!(states.iter().all(|(_, s)| *s == ServiceState::Running));
    let activity = api.activity(200).await.unwrap();
    assert!(activity.iter().any(|a| a.message == "Domain added"));
    assert!(
        activity
            .iter()
            .any(|a| a.message.starts_with("Proxy installed"))
    );
    let stats = api.service_stats(&sid).await.unwrap();
    println!(
        "whoami stats: {:.1}% cpu, {} bytes",
        stats.cpu_percent, stats.mem_used
    );
    assert!(stats.mem_used > 0);

    println!("== delete project");
    let ids = [sid.clone(), site.service.id.clone(), db.service.id.clone()];
    api.delete_project(&project.id).await.unwrap();
    for id in &ids {
        assert!(
            service_containers(id).is_empty(),
            "containers of {id} survived"
        );
    }
    let network = format!("{}-env-{}", brand::PREFIX, env.id);
    assert!(
        docker(&[
            "network",
            "ls",
            "-q",
            "--filter",
            &format!("name={network}")
        ])
        .trim()
        .is_empty()
    );
    assert!(api.tree().await.unwrap().projects.is_empty());

    // Leave the machine as we found it.
    let _ = Command::new("docker")
        .args(["rm", "-f", brand::PROXY_CONTAINER])
        .output();
}

/// A throwaway sshd on localhost that trusts the daemon's key.
struct Sshd {
    child: std::process::Child,
    port: u16,
    _dir: tempfile::TempDir,
}

impl Sshd {
    fn start(authorized_key: &Path) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let host_key = dir.path().join("host_key");
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&host_key)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::copy(authorized_key, dir.path().join("authorized_keys")).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = dir.path().join("sshd_config");
        std::fs::write(
            &config,
            format!(
                "Port {port}
ListenAddress 127.0.0.1
HostKey {}
AuthorizedKeysFile {}
PidFile {}
StrictModes no
UsePAM no
PasswordAuthentication no
",
                host_key.display(),
                dir.path().join("authorized_keys").display(),
                dir.path().join("sshd.pid").display(),
            ),
        )
        .unwrap();
        let child = Command::new("/usr/sbin/sshd")
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "sshd did not start");
            std::thread::sleep(Duration::from_millis(100));
        }
        Self {
            child,
            port,
            _dir: dir,
        }
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
#[ignore = "needs Docker and an sshd binary"]
async fn ssh_server_end_to_end() {
    let daemon = Daemon::start().await;
    let api = daemon.client();
    let sshd = Sshd::start(&daemon.dir.path().join("ssh/id_ed25519.pub"));
    let user = String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap();

    let server = api
        .create_server(&CreateServer {
            name: "remote".into(),
            host: "127.0.0.1".into(),
            user: user.trim().into(),
            port: sshd.port,
        })
        .await
        .unwrap();
    let checked = api.check_server(&server.id).await.unwrap();
    assert_eq!(checked.status, ServerStatus::Online, "{checked:?}");
    assert!(checked.docker_version.is_some());
    println!("remote docker {:?}", checked.docker_version);

    api.create_project(&CreateProject {
        name: "remote".into(),
        description: None,
    })
    .await
    .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();
    let svc = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: Some(server.id.clone()),
            name: "static".into(),
            source: Source::Image {
                image: "busybox:1.36".into(),
            },
            port: Some(8080),
            command: Some(
                "mkdir -p /www && echo hi > /www/index.html && exec httpd -f -p 8080 -h /www"
                    .into(),
            ),
            restart: RestartPolicy::UnlessStopped,
            health_path: Some("/".into()),
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: true,
        })
        .await
        .unwrap();
    let d = wait_finished(
        &api,
        &latest_deployment(&api, &svc.service.id).await.id,
        Duration::from_secs(120),
    )
    .await;
    assert_eq!(d.status, DeploymentStatus::Live, "{d:?}");
    let health = d.steps.iter().find(|s| s.kind == StepKind::Health).unwrap();
    assert_eq!(health.detail.as_deref(), Some("GET / -> ok"));
    let detail = wait_state(&api, &svc.service.id, ServiceState::Running).await;
    assert_eq!(detail.server.id, server.id);
    let lines = collect_logs(api.service_logs(&svc.service.id, 10, false).await.unwrap()).await;
    println!("remote logs: {lines:?}");

    let project = api.projects().await.unwrap().remove(0);
    api.delete_project(&project.id).await.unwrap();
    assert!(service_containers(&svc.service.id).is_empty());
    api.delete_server(&server.id).await.unwrap();
}
