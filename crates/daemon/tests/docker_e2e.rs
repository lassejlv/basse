//! End-to-end flow against a real Docker engine. Binds ports 80/443 for the proxy.
//! Run with `cargo test -p daemon --test docker_e2e -- --ignored --nocapture`.

mod support;

use client::Client;
use common::api::*;
use common::*;
use futures_util::StreamExt as _;
use std::net::TcpListener;
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
    let daemon = Daemon::start().await.expect("these tests need Docker");
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
                source_id: None,
                auto_deploy: true,
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
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let sshd = Sshd::start(&daemon.dir.path().join("ssh/id_ed25519.pub"));
    let user = String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap();

    let server = api
        .create_server(&CreateServer {
            name: "remote".into(),
            host: "127.0.0.1".into(),
            user: user.trim().into(),
            port: sshd.port,
            ssh_key_secret_id: None,
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

    // Backups, restores and rotation exec through the SSH-tunnelled Docker connection.
    let redis =
        exercise_database(&api, &env.id, Some(server.id.clone()), DatabaseKind::Redis).await;
    assert_eq!(redis.server.id, server.id);

    let project = api.projects().await.unwrap().remove(0);
    api.delete_project(&project.id).await.unwrap();
    assert!(service_containers(&svc.service.id).is_empty());
    api.delete_server(&server.id).await.unwrap();
}

// Databases: backup, restore, password rotation and public access against real engines.

/// Runs `docker <args>`, returning trimmed stdout when it exits with 0.
fn docker_ok(args: &[String]) -> Result<String, String> {
    let out = Command::new("docker").args(args).output().unwrap();
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Retries a docker command until it succeeds; databases take a while to accept connections.
async fn until_ok(args: &[String], timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        match docker_ok(args) {
            Ok(out) => return out,
            Err(e) if Instant::now() >= deadline => panic!("docker {args:?} kept failing: {e}"),
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

async fn wait_backup(api: &Client, id: &str) -> Backup {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let b = api.backup(id).await.unwrap();
        if b.status != BackupStatus::Running {
            return b;
        }
        assert!(Instant::now() < deadline, "backup never finished: {b:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Waits for the newest deployment of a service to go live and the service to run.
async fn wait_live(api: &Client, service_id: &str) -> ServiceDetail {
    let d = wait_finished(
        api,
        &latest_deployment(api, service_id).await.id,
        Duration::from_secs(300),
    )
    .await;
    assert_eq!(d.status, DeploymentStatus::Live, "{d:?}");
    wait_state(api, service_id, ServiceState::Running).await
}

/// Commands that touch one row of test data and probe a connection, per engine.
struct Probe {
    /// Exec'd in the database container.
    write: Vec<String>,
    mutate: Vec<String>,
    read: Vec<String>,
    /// Run in a fresh client container; prints `ping_ok` when connected.
    ping: Vec<String>,
    ping_ok: &'static str,
}

fn probe(info: &DatabaseInfo, client_image: &str) -> Probe {
    let user = info.username.clone().unwrap_or_default();
    let pass = info.password.clone();
    let db = info.database.clone().unwrap_or_default();
    match info.kind {
        DatabaseKind::Postgres => {
            let psql = |sql: &str| {
                strings(&[
                    "psql",
                    "-U",
                    &user,
                    "-d",
                    &db,
                    "-v",
                    "ON_ERROR_STOP=1",
                    "-tAc",
                    sql,
                ])
            };
            Probe {
                write: psql(
                    "CREATE TABLE IF NOT EXISTS notes (t text PRIMARY KEY); \
                     INSERT INTO notes VALUES ('kept') ON CONFLICT DO NOTHING",
                ),
                mutate: psql("DROP TABLE notes"),
                read: psql("SELECT t FROM notes"),
                ping: strings(&[client_image, "psql", &info.internal_url, "-tAc", "SELECT 1"]),
                ping_ok: "1",
            }
        }
        DatabaseKind::Mysql | DatabaseKind::Mariadb => {
            let client = if info.kind == DatabaseKind::Mysql {
                "mysql"
            } else {
                "mariadb"
            };
            let password = format!("-p{pass}");
            let sql =
                |sql: &str| strings(&[client, "-u", &user, &password, &db, "-N", "-B", "-e", sql]);
            Probe {
                write: sql(
                    "CREATE TABLE IF NOT EXISTS notes (t VARCHAR(16) PRIMARY KEY); \
                     REPLACE INTO notes VALUES ('kept')",
                ),
                mutate: sql("DROP TABLE notes"),
                read: sql("SELECT t FROM notes"),
                ping: strings(&[
                    client_image,
                    client,
                    "-h",
                    &info.host,
                    "-u",
                    &user,
                    &password,
                    &db,
                    "-N",
                    "-B",
                    "-e",
                    "SELECT 1",
                ]),
                ping_ok: "1",
            }
        }
        DatabaseKind::Redis => {
            let cli = |args: &[&str]| {
                let mut cmd = strings(&["redis-cli", "--no-auth-warning", "-a", &pass]);
                cmd.extend(strings(args));
                cmd
            };
            Probe {
                write: cli(&["SET", "note", "kept"]),
                mutate: cli(&["SET", "note", "changed"]),
                read: cli(&["GET", "note"]),
                ping: strings(&[
                    client_image,
                    "redis-cli",
                    "--no-auth-warning",
                    "-u",
                    &info.internal_url,
                    "PING",
                ]),
                ping_ok: "PONG",
            }
        }
        DatabaseKind::Mongo => {
            let eval = |js: &str| {
                strings(&[
                    "mongosh",
                    "--quiet",
                    "-u",
                    &user,
                    "-p",
                    &pass,
                    "--authenticationDatabase",
                    "admin",
                    "app",
                    "--eval",
                    js,
                ])
            };
            Probe {
                write: eval(
                    "db.notes.updateOne({ _id: 1 }, { $set: { t: 'kept' } }, { upsert: true })",
                ),
                mutate: eval("db.notes.drop()"),
                read: eval("db.notes.findOne({ _id: 1 }).t"),
                ping: strings(&[
                    client_image,
                    "mongosh",
                    &info.internal_url,
                    "--quiet",
                    "--eval",
                    "db.runCommand({ ping: 1 }).ok",
                ]),
                ping_ok: "1",
            }
        }
    }
}

fn exec_in(container: &str, cmd: &[String]) -> Result<String, String> {
    let mut args = strings(&["exec", container]);
    args.extend_from_slice(cmd);
    docker_ok(&args)
}

/// Connects from a fresh container on the environment network, as another service would.
fn ping(env_id: &str, p: &Probe) -> Result<String, String> {
    let mut args = strings(&[
        "run",
        "--rm",
        "--network",
        &format!("{}-env-{env_id}", brand::PREFIX),
    ]);
    args.extend_from_slice(&p.ping);
    docker_ok(&args)
}

async fn wait_ping(env_id: &str, p: &Probe) {
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        match ping(env_id, p) {
            Ok(out) if out.ends_with(p.ping_ok) => return,
            other if Instant::now() >= deadline => panic!("cannot connect: {other:?}"),
            _ => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }
}

/// Creates a database, writes a row, backs it up, breaks the row, restores it, rotates the
/// password and checks the new internal URL works and the old one does not.
async fn exercise_database(
    api: &Client,
    env_id: &str,
    server_id: Option<Id>,
    kind: DatabaseKind,
) -> ServiceDetail {
    let name = kind.image();
    println!("== {name}: create + deploy");
    let created = api
        .create_service(&CreateService {
            environment_id: env_id.into(),
            server_id,
            name: name.into(),
            source: Source::Database {
                kind,
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
    let sid = created.service.id.clone();
    let image = format!("{}:{}", kind.image(), kind.default_version());
    let detail = wait_live(api, &sid).await;
    let info = detail.database.clone().unwrap();
    assert_eq!(info.version, kind.default_version());
    assert_eq!(info.host, name);
    let container = detail.container.unwrap().id;
    let p = probe(&info, &image);
    wait_ping(env_id, &p).await;
    until_ok(
        &[strings(&["exec", &container]), p.write.clone()].concat(),
        Duration::from_secs(60),
    )
    .await;
    assert_eq!(exec_in(&container, &p.read).unwrap(), "kept");

    println!("== {name}: backup");
    let started = api.create_backup(&sid, None).await.unwrap();
    assert_eq!(started.status, BackupStatus::Running);
    let backup = wait_backup(api, &started.id).await;
    assert_eq!(backup.status, BackupStatus::Done, "{backup:?}");
    assert!(backup.size_bytes > 0);
    assert!(backup.file_name.ends_with(".gz"), "{}", backup.file_name);
    let mut download = api.download_backup(&backup.id).await.unwrap();
    let mut bytes = Vec::new();
    while let Some(chunk) = download.next().await {
        bytes.extend(chunk.unwrap());
    }
    assert_eq!(bytes.len() as u64, backup.size_bytes);
    assert_eq!(&bytes[..2], [0x1f, 0x8b], "not gzip");

    println!("== {name}: restore");
    exec_in(&container, &p.mutate).unwrap();
    assert_ne!(exec_in(&container, &p.read).ok().as_deref(), Some("kept"));
    api.restore_backup(&backup.id).await.unwrap();
    wait_ping(env_id, &p).await;
    assert_eq!(exec_in(&container, &p.read).unwrap(), "kept");

    println!("== {name}: rotate password");
    let rotated = api.rotate_database_password(&sid).await.unwrap();
    assert_ne!(rotated.password, info.password);
    assert_ne!(rotated.internal_url, info.internal_url);
    let detail = if kind == DatabaseKind::Redis {
        wait_live(api, &sid).await
    } else {
        api.service(&sid).await.unwrap()
    };
    let url_key = if kind == DatabaseKind::Redis {
        "REDIS_URL"
    } else {
        "DATABASE_URL"
    };
    let stored = detail.env.iter().find(|v| v.key == url_key).unwrap();
    assert_eq!(stored.value, rotated.internal_url);
    let fresh = probe(&rotated, &image);
    assert!(ping(env_id, &fresh).unwrap().ends_with(fresh.ping_ok));
    assert!(
        ping(env_id, &p)
            .map(|out| !out.ends_with(p.ping_ok))
            .unwrap_or(true),
        "the old password still works"
    );
    let container = detail.container.clone().unwrap().id;
    assert_eq!(exec_in(&container, &fresh.read).unwrap(), "kept");

    println!("== {name}: restart keeps the new password");
    docker_ok(&strings(&["restart", &container])).unwrap();
    wait_ping(env_id, &fresh).await;
    assert_eq!(exec_in(&container, &fresh.read).unwrap(), "kept");

    let activity = api.activity(200).await.unwrap();
    for message in [
        "Backup done",
        "Backup restored",
        "Database password rotated",
    ] {
        assert!(
            activity
                .iter()
                .any(|a| a.service_id.as_deref() == Some(sid.as_str()) && a.message == message),
            "no '{message}' activity for {name}"
        );
    }
    detail
}

async fn database_project(api: &Client, name: &str) -> (Project, Environment) {
    let project = api
        .create_project(&CreateProject {
            name: name.into(),
            description: None,
        })
        .await
        .unwrap();
    let env = api
        .tree()
        .await
        .unwrap()
        .projects
        .into_iter()
        .find(|p| p.project.id == project.id)
        .unwrap()
        .environments
        .remove(0)
        .environment;
    (project, env)
}

#[tokio::test]
#[ignore = "needs Docker and the postgres:18-alpine image"]
async fn postgres_backup_restore_rotate_expose() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let (project, env) = database_project(&api, "pg").await;
    let detail = exercise_database(&api, &env.id, None, DatabaseKind::Postgres).await;
    let sid = detail.service.id.clone();
    assert_eq!(
        detail.volumes[0].mount_path, "/var/lib/postgresql",
        "postgres 18 keeps PGDATA under the parent directory"
    );

    println!("== postgres: public access");
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let exposed = api
        .update_database(
            &sid,
            &UpdateDatabase {
                public_port: Some(port),
            },
        )
        .await
        .unwrap();
    assert_eq!(exposed.public_port, Some(port));
    let live = wait_live(&api, &sid).await;
    assert_ne!(
        live.container.as_ref().unwrap().id,
        detail.container.as_ref().unwrap().id
    );
    let user = exposed.username.clone().unwrap();
    let db = exposed.database.clone().unwrap();
    let local_url = format!(
        "postgres://{user}:{}@127.0.0.1:{port}/{db}",
        exposed.password
    );
    let read = until_ok(
        &strings(&[
            "run",
            "--rm",
            "--network",
            "host",
            "postgres:18-alpine",
            "psql",
            &local_url,
            "-tAc",
            "SELECT t FROM notes",
        ]),
        Duration::from_secs(60),
    )
    .await;
    assert_eq!(
        read, "kept",
        "data survives the redeploy and is reachable from outside"
    );

    api.update_database(&sid, &UpdateDatabase { public_port: None })
        .await
        .unwrap();
    let live = wait_live(&api, &sid).await;
    assert!(live.database.unwrap().public_url.is_none());

    let backups = api.backups(&sid).await.unwrap();
    api.delete_backup(&backups[0].id).await.unwrap();
    assert!(api.backups(&sid).await.unwrap().is_empty());
    api.delete_project(&project.id).await.unwrap();
}

#[tokio::test]
#[ignore = "needs Docker and the mysql, mariadb, redis and mongo images"]
async fn other_databases_backup_restore_rotate() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let (project, env) = database_project(&api, "dbs").await;
    for kind in [
        DatabaseKind::Mysql,
        DatabaseKind::Mariadb,
        DatabaseKind::Redis,
        DatabaseKind::Mongo,
    ] {
        exercise_database(&api, &env.id, None, kind).await;
    }
    let backups_dir = daemon.dir.path().join("backups");
    api.delete_project(&project.id).await.unwrap();
    let left: Vec<_> = std::fs::read_dir(&backups_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != ".staging")
        .filter(|e| std::fs::read_dir(e.path()).is_ok_and(|mut d| d.next().is_some()))
        .collect();
    assert!(
        left.is_empty(),
        "backup files survived the project: {left:?}"
    );
}

#[tokio::test]
#[ignore = "needs Docker and an sshd binary"]
async fn ssh_server_with_a_secret_key() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let key = api.generate_ssh_key("edge-key").await.unwrap();
    let public = daemon.dir.path().join("edge-key.pub");
    std::fs::write(&public, key.public_part.clone().unwrap()).unwrap();
    let sshd = Sshd::start(&public);
    let user = String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap();

    let server = api
        .create_server(&CreateServer {
            name: "keyed".into(),
            host: "127.0.0.1".into(),
            user: user.trim().into(),
            port: sshd.port,
            ssh_key_secret_id: Some(key.id.clone()),
        })
        .await
        .unwrap();
    let checked = api.check_server(&server.id).await.unwrap();
    assert_eq!(checked.status, ServerStatus::Online, "{checked:?}");
    let keys_dir = daemon.dir.path().join("ssh/keys");
    let leftovers: Vec<_> = std::fs::read_dir(&keys_dir)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(
        leftovers.is_empty(),
        "temporary keys left behind: {leftovers:?}"
    );

    let own_key = api
        .update_server(
            &server.id,
            &UpdateServer {
                ssh_key_secret_id: Some(String::new()),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        own_key.status,
        ServerStatus::Offline,
        "sshd only trusts edge-key"
    );
    let back = api
        .update_server(
            &server.id,
            &UpdateServer {
                ssh_key_secret_id: Some(key.id.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(back.status, ServerStatus::Online, "{back:?}");
}
