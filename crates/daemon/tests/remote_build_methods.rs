//! Every build method deployed to a remote SSH server that has its own Docker engine: a
//! privileged Docker-in-Docker container running sshd, reached through the daemon's `ssh -L`
//! tunnel. Each deployment must land in that engine and never in the one the daemon runs next
//! to. Run with
//! `cargo test -p daemon --test remote_build_methods -- --ignored --test-threads 1`.

mod support;

use client::Client;
use common::api::*;
use common::*;
use futures_util::StreamExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use support::Daemon;

const DIND_IMAGE: &str = "docker:29-dind";

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

/// A second Docker host: Docker-in-Docker with sshd on a loopback port, letting the daemon's
/// key in as root. Alpine's sshd ships with `AllowTcpForwarding no`, which also refuses the
/// tunnel's Unix socket forward, so the image turns it on.
struct RemoteHost {
    container: String,
    port: u16,
}

impl RemoteHost {
    fn start(authorized_key: &Path) -> Self {
        let image = format!("{}-test-remote-host", brand::PREFIX);
        let dockerfile = format!(
            "FROM {DIND_IMAGE}\nRUN apk add --no-cache openssh-server && ssh-keygen -A \
             && echo 'AllowTcpForwarding yes' > /etc/ssh/sshd_config.d/forwarding.conf \
             && mkdir -p /root/.ssh && chmod 700 /root/.ssh\n"
        );
        let mut build = Command::new("docker")
            .args(["build", "-q", "-t", &image, "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut build.stdin.take().unwrap(), dockerfile.as_bytes()).unwrap();
        assert!(build.wait().unwrap().success(), "cannot build {image}");
        let container = format!("{}-test-remote-{}", brand::PREFIX, std::process::id());
        let key = std::fs::read_to_string(authorized_key).unwrap();
        let here = Path::new(".");
        run(
            here,
            "docker",
            &[
                "run",
                "-d",
                "--privileged",
                "--name",
                &container,
                "-p",
                "127.0.0.1::22",
                "-e",
                &format!("AUTHORIZED_KEY={}", key.trim()),
                "--entrypoint",
                "sh",
                &image,
                "-c",
                "echo \"$AUTHORIZED_KEY\" > /root/.ssh/authorized_keys \
                 && chmod 600 /root/.ssh/authorized_keys && /usr/sbin/sshd \
                 && exec dockerd-entrypoint.sh",
            ],
        );
        let mapped = run(here, "docker", &["port", &container, "22/tcp"]);
        let port = mapped
            .lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or_else(|| panic!("no port for sshd: {mapped}"));
        let remote = Self { container, port };
        let deadline = Instant::now() + Duration::from_secs(60);
        while remote.try_docker(&["info"]).is_err() {
            assert!(Instant::now() < deadline, "the remote engine did not start");
            std::thread::sleep(Duration::from_millis(500));
        }
        remote
    }

    fn exec(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("docker")
            .args(["exec", &self.container])
            .args(args)
            .output()
            .unwrap();
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    fn try_docker(&self, args: &[&str]) -> Result<String, String> {
        self.exec(&[&["docker"], args].concat())
    }

    /// `docker <args>` against the remote engine.
    fn docker(&self, args: &[&str]) -> String {
        self.try_docker(args)
            .unwrap_or_else(|e| panic!("remote docker {args:?}: {e}"))
    }
}

impl Drop for RemoteHost {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn label_filter(service_id: &str) -> String {
    format!("label={}={service_id}", brand::label("service"))
}

/// Container IDs of a service on the daemon's own engine.
fn local_containers(service_id: &str) -> String {
    run(
        Path::new("."),
        "docker",
        &["ps", "-a", "-q", "--filter", &label_filter(service_id)],
    )
}

/// Names of a service's running containers on the remote engine; asserts the local engine has
/// none of them.
fn remote_only(remote: &RemoteHost, service_id: &str) -> Vec<String> {
    assert_eq!(
        local_containers(service_id).trim(),
        "",
        "service {service_id} has containers on the daemon's own engine"
    );
    let mut names: Vec<String> = remote
        .docker(&[
            "ps",
            "--filter",
            &label_filter(service_id),
            "--format",
            "{{.Names}}",
        ])
        .lines()
        .map(String::from)
        .collect();
    names.sort();
    names
}

fn local_image_exists(tag: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", tag])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// A bare repository holding `files` on `main`, as a `file://` URL.
fn fixture(root: &Path, name: &str, files: &[(&str, &str)]) -> String {
    let work = root.join(name);
    for (path, body) in files {
        let path = work.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    run(&work, "git", &["init", "-q", "-b", "main"]);
    run(&work, "git", &["add", "."]);
    run(&work, "git", &["commit", "-q", "-m", "fixture"]);
    let bare = root.join(format!("{name}.git"));
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

fn service(
    env: &Environment,
    server: &Server,
    name: &str,
    source: Source,
    port: u16,
) -> CreateService {
    CreateService {
        environment_id: env.id.clone(),
        server_id: Some(server.id.clone()),
        name: name.into(),
        source,
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

fn git(repo: String) -> Source {
    Source::Git {
        repo,
        branch: "main".into(),
        dockerfile: "Dockerfile".into(),
        context: ".".into(),
        source_id: None,
        auto_deploy: false,
        build: Build {
            method: BuildMethod::Auto,
            ..Build::default()
        },
    }
}

/// Deploys and waits for the outcome; returns the deployment and its log.
async fn deploy(api: &Client, service_id: &str, timeout: Duration) -> (Deployment, Vec<String>) {
    let d = api.deploy(service_id, "test").await.unwrap();
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
            "deployment #{} did not finish: {d:?}",
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

async fn deploy_live(
    api: &Client,
    service_id: &str,
    timeout: Duration,
) -> (Deployment, Vec<String>) {
    let (d, log) = deploy(api, service_id, timeout).await;
    assert_eq!(
        d.status,
        DeploymentStatus::Live,
        "{d:#?}\n{}",
        log.join("\n")
    );
    (d, log)
}

/// Service logs, read through the tunnel, once they contain `want`.
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

/// Fetches `http://<container>:<port>/` from inside the remote host.
fn remote_get(remote: &RemoteHost, container: &str, port: u16) -> String {
    let ips = remote.docker(&[
        "inspect",
        "--format",
        "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}",
        container,
    ]);
    let ip = ips.split_whitespace().next().expect("container has an IP");
    let url = format!("http://{ip}:{port}/");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match remote.exec(&["wget", "-q", "-O", "-", "-T", "3", &url]) {
            Ok(body) => return body,
            Err(e) if Instant::now() >= deadline => panic!("GET {url} on the remote: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

#[tokio::test]
#[ignore = "needs Docker with privileged containers and network access for Railpack"]
async fn remote_build_methods() {
    let daemon = Daemon::start().await.expect("these tests need Docker");
    let api = daemon.client();
    let remote = RemoteHost::start(&daemon.dir.path().join("ssh/id_ed25519.pub"));
    let server = api
        .create_server(&CreateServer {
            name: "remote".into(),
            host: "127.0.0.1".into(),
            user: "root".into(),
            port: remote.port,
            ssh_key_secret_id: None,
        })
        .await
        .unwrap();
    let checked = api.check_server(&server.id).await.unwrap();
    assert_eq!(checked.status, ServerStatus::Online, "{checked:?}");
    api.create_project(&CreateProject {
        name: "remote".into(),
        description: None,
    })
    .await
    .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();
    let root = tempfile::tempdir().unwrap();

    // Image: pulled by the remote engine.
    let mut spec = service(
        &env,
        &server,
        "image",
        Source::Image {
            image: "busybox:1.37".into(),
        },
        8080,
    );
    spec.command = Some(
        "mkdir -p /www && echo image-ok > /www/index.html && exec httpd -f -p 8080 -h /www".into(),
    );
    spec.health_path = Some("/".into());
    let image = api.create_service(&spec).await.unwrap().service;
    let (d, _) = deploy_live(&api, &image.id, Duration::from_secs(300)).await;
    let health = d.steps.iter().find(|s| s.kind == StepKind::Health).unwrap();
    assert_eq!(health.detail.as_deref(), Some("GET / -> ok"));
    let names = remote_only(&remote, &image.id);
    assert_eq!(names.len(), 1, "{names:?}");
    assert_eq!(remote_get(&remote, &names[0], 8080), "image-ok");

    // Dockerfile: the build context is streamed to the remote engine, which builds and runs it.
    let repo = fixture(
        root.path(),
        "dockerfile",
        &[(
            "Dockerfile",
            "FROM busybox:1.37\nARG GREETING=unset\n\
             RUN mkdir /www && echo \"built with $GREETING\" > /www/index.html\n\
             CMD [\"httpd\", \"-f\", \"-p\", \"8080\", \"-h\", \"/www\"]\n",
        )],
    );
    let mut spec = service(&env, &server, "dockerfile", git(repo), 8080);
    spec.health_path = Some("/".into());
    let dockerfile = api.create_service(&spec).await.unwrap().service;
    let (d, log) = deploy_live(&api, &dockerfile.id, Duration::from_secs(300)).await;
    assert!(
        log.iter()
            .any(|l| l.contains("Detected the dockerfile build method")),
        "{log:#?}"
    );
    let tag = d.image.unwrap();
    remote.docker(&["image", "inspect", &tag]);
    assert!(!local_image_exists(&tag), "{tag} was built locally");
    let names = remote_only(&remote, &dockerfile.id);
    assert_eq!(
        remote_get(&remote, &names[0], 8080),
        "built with hello-from-env"
    );

    // Compose: the CLI runs next to the daemon with DOCKER_HOST on the tunnel, so builds,
    // containers and named volumes all live in the remote engine.
    let repo = fixture(
        root.path(),
        "compose",
        &[
            (
                "compose.yaml",
                "services:\n  web:\n    build: ./web\n    environment:\n      GREETING: ${GREETING}\n  \
                 worker:\n    image: busybox:1.37\n    \
                 command: [\"sh\", \"-c\", \"[ -f /data/first ] || date +%s%N > /data/first; echo first=$$(cat /data/first); exec sleep 3600\"]\n    \
                 volumes:\n      - data:/data\n  \
                 files:\n    image: busybox:1.37\n    \
                 command: [\"sh\", \"-c\", \"echo files=$$(cat /repo/hello.txt 2>&1); exec sleep 3600\"]\n    \
                 volumes:\n      - ./files:/repo\nvolumes:\n  data: {}\n",
            ),
            (
                "web/Dockerfile",
                "FROM busybox:1.37\nCMD [\"sh\", \"-c\", \"echo web says $GREETING; exec httpd -f -p 8080 -h /\"]\n",
            ),
            ("files/hello.txt", "from the repository\n"),
        ],
    );
    let compose = api
        .create_service(&service(&env, &server, "compose", git(repo), 8080))
        .await
        .unwrap()
        .service;
    deploy_live(&api, &compose.id, Duration::from_secs(300)).await;
    let names = remote_only(&remote, &compose.id);
    assert_eq!(names.len(), 3, "{names:?}");
    let web = logs(&api, &compose.id, Some("web"), "web says").await;
    assert!(web.contains("web says hello-from-env"), "{web}");
    let volumes = remote.docker(&[
        "volume",
        "ls",
        "-q",
        "--filter",
        &format!(
            "label=com.docker.compose.project={}-{}",
            brand::PREFIX,
            compose.id
        ),
    ]);
    assert!(volumes.ends_with("_data"), "{volumes}");
    let worker = logs(&api, &compose.id, Some("worker"), "first=").await;
    assert!(worker.contains("first="), "{worker}");
    // A relative bind mount resolves against the clone next to the daemon, a path that does not
    // exist on the remote, whose engine mounts an empty directory there instead of the files.
    let files = logs(&api, &compose.id, Some("files"), "files=").await;
    assert!(files.contains("No such file"), "{files}");
    let mounts = remote.docker(&[
        "inspect",
        "--format",
        "{{range .Mounts}}{{.Type}} {{.Source}};{{end}}",
        names.iter().find(|n| n.contains("files")).unwrap(),
    ]);
    let builds = daemon.dir.path().join("builds");
    assert!(
        mounts.starts_with(&format!("bind {}/", builds.display())),
        "{mounts}"
    );

    // Railpack: `railpack prepare` runs next to the daemon, buildx builds on the remote
    // engine's BuildKit and loads the image there.
    let repo = fixture(
        root.path(),
        "railpack",
        &[
            (
                "package.json",
                "{\"name\":\"hello\",\"version\":\"1.0.0\",\"scripts\":{\"start\":\"node index.js\"}}\n",
            ),
            (
                "index.js",
                "const http = require('http');\nconsole.log('node says ' + process.env.GREETING);\n\
                 http.createServer((_, res) => res.end('railpack-ok')).listen(3000);\n",
            ),
        ],
    );
    let railpack = api
        .create_service(&service(&env, &server, "railpack", git(repo), 3000))
        .await
        .unwrap()
        .service;
    let (d, log) = deploy_live(&api, &railpack.id, Duration::from_secs(900)).await;
    assert!(
        log.iter().any(|l| l.contains("Railpack detected")),
        "{log:#?}"
    );
    let tag = d.image.unwrap();
    remote.docker(&["image", "inspect", &tag]);
    assert!(!local_image_exists(&tag), "{tag} was built locally");
    let names = remote_only(&remote, &railpack.id);
    assert_eq!(remote_get(&remote, &names[0], 3000), "railpack-ok");
    let out = logs(&api, &railpack.id, None, "node says").await;
    assert!(out.contains("node says hello-from-env"), "{out}");

    // Offline: with sshd gone the deploy fails on the connection instead of falling back to the
    // daemon's own engine, and works again once the server is back.
    remote.exec(&["pkill", "sshd"]).unwrap();
    let checked = api.check_server(&server.id).await.unwrap();
    assert_eq!(checked.status, ServerStatus::Offline, "{checked:?}");
    let (d, _) = deploy(&api, &dockerfile.id, Duration::from_secs(120)).await;
    assert_eq!(d.status, DeploymentStatus::Failed, "{d:#?}");
    assert!(
        d.error
            .as_deref()
            .is_some_and(|e| e.starts_with("ssh to root@127.0.0.1")),
        "{d:#?}"
    );
    assert!(
        d.steps.iter().all(|s| s.status == StepStatus::Skipped),
        "{d:#?}"
    );
    assert_eq!(remote_only(&remote, &dockerfile.id).len(), 1);
    remote.exec(&["/usr/sbin/sshd"]).unwrap();
    deploy_live(&api, &dockerfile.id, Duration::from_secs(300)).await;
    remote_only(&remote, &dockerfile.id);

    let project = api.projects().await.unwrap().remove(0);
    api.delete_project(&project.id).await.unwrap();
    api.delete_server(&server.id).await.unwrap();
}

/// A health check that fails while a deployment clones drops the server's tunnel. The build
/// after the clone must reopen it instead of running the docker CLI against the daemon's own
/// engine.
#[tokio::test]
#[ignore = "needs Docker with privileged containers"]
async fn remote_build_after_a_dropped_tunnel() {
    let daemon = Daemon::start_with(|dir| {
        let config = dir.join("slow-clone.gitconfig");
        std::fs::write(&config, "[uploadpack]\n\tpackObjectsHook = \"sleep 8;\"\n").unwrap();
        vec![(
            "GIT_CONFIG_GLOBAL".to_string(),
            config.to_string_lossy().into_owned(),
        )]
    })
    .await
    .expect("these tests need Docker");
    let api = daemon.client();
    let remote = RemoteHost::start(&daemon.dir.path().join("ssh/id_ed25519.pub"));
    let server = api
        .create_server(&CreateServer {
            name: "remote".into(),
            host: "127.0.0.1".into(),
            user: "root".into(),
            port: remote.port,
            ssh_key_secret_id: None,
        })
        .await
        .unwrap();
    api.create_project(&CreateProject {
        name: "remote".into(),
        description: None,
    })
    .await
    .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();
    let root = tempfile::tempdir().unwrap();
    let repo = fixture(
        root.path(),
        "compose",
        &[(
            "compose.yaml",
            "services:\n  app:\n    image: busybox:1.37\n    command: [\"sleep\", \"3600\"]\n",
        )],
    );
    let svc = api
        .create_service(&service(&env, &server, "compose", git(repo), 8080))
        .await
        .unwrap()
        .service;
    let d = api.deploy(&svc.id, "test").await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let d = api.deployment(&d.id).await.unwrap();
        let clone = d.steps.iter().find(|s| s.kind == StepKind::Clone).unwrap();
        if clone.status == StepStatus::Running {
            break;
        }
        assert!(Instant::now() < deadline, "the clone never started: {d:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    remote.exec(&["pkill", "sshd"]).unwrap();
    let checked = api.check_server(&server.id).await.unwrap();
    assert_eq!(checked.status, ServerStatus::Offline, "{checked:?}");
    remote.exec(&["/usr/sbin/sshd"]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(300);
    let d = loop {
        let d = api.deployment(&d.id).await.unwrap();
        if d.status.is_finished() {
            break d;
        }
        assert!(Instant::now() < deadline, "the deployment never finished");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let local = local_containers(&svc.id);
    let project = format!("{}-{}", brand::PREFIX, svc.id);
    let _ = Command::new("docker")
        .args(["compose", "--project-name", &project, "down", "--volumes"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    assert_eq!(
        local.trim(),
        "",
        "compose ran on the daemon's own engine: {d:#?}"
    );
    assert_eq!(d.status, DeploymentStatus::Live, "{d:#?}");
    assert_eq!(remote_only(&remote, &svc.id).len(), 1);
}
