//! Adding remote servers over SSH: a real sshd in front of a real Docker engine
//! (Docker-in-Docker), reached through the daemon's preflight and tunnel. Covers a root user, a
//! user in the docker group, a user without access to the socket, sshd refusing the socket
//! forward, and a server without Docker.

mod support;

use common::api::CreateServer;
use common::*;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use support::Daemon;

const DIND_IMAGE: &str = "docker:29-dind";

/// The image already has a docker group. Users: root and deploy can use Docker, nodocker is outside the docker group, and sshd
/// refuses to forward Unix sockets for fwdoff (who is in the docker group).
const DOCKERFILE: &str = r#"FROM docker:29-dind
RUN apk add --no-cache openssh-server && ssh-keygen -A \
 && for u in deploy nodocker fwdoff; do adduser -D -s /bin/sh $u && sed -i "s/^$u:!/$u:*/" /etc/shadow; done \
 && addgroup deploy docker && addgroup fwdoff docker \
 && printf 'AllowTcpForwarding yes\nMatch User fwdoff\n    AllowStreamLocalForwarding no\n' > /etc/ssh/sshd_config.d/test.conf
"#;

const ENTRYPOINT: &str = r#"for u in root deploy nodocker fwdoff; do
  h=$(getent passwd $u | cut -d: -f6)
  mkdir -p $h/.ssh && echo "$AUTHORIZED_KEY" > $h/.ssh/authorized_keys
  chown -R $u $h/.ssh && chmod 700 $h/.ssh && chmod 600 $h/.ssh/authorized_keys
done
/usr/sbin/sshd
if [ "$NO_DOCKER" = 1 ]; then rm -f /usr/local/bin/docker /usr/local/bin/dockerd; exec sleep infinity; fi
exec dockerd-entrypoint.sh"#;

/// A container with sshd on a loopback port, removed when dropped.
struct Remote {
    container: String,
    port: u16,
}

impl Remote {
    fn start(image: &str, name: &str, key: &str, with_docker: bool) -> Self {
        let container = format!("{}-test-{name}-{}", brand::PREFIX, std::process::id());
        let out = Command::new("docker")
            .args(["run", "-d", "--privileged", "--name", &container])
            .args(["-p", "127.0.0.1::22", "-e", "DOCKER_TLS_CERTDIR="])
            .args(["-e", &format!("AUTHORIZED_KEY={}", key.trim())])
            .args(["-e", &format!("NO_DOCKER={}", u8::from(!with_docker))])
            .args(["--entrypoint", "sh", image, "-c", ENTRYPOINT])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "docker run: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = Command::new("docker")
            .args(["port", &container, "22/tcp"])
            .output()
            .unwrap();
        let mapped = String::from_utf8_lossy(&out.stdout);
        let port = mapped
            .lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or_else(|| panic!("no port for sshd: {mapped}"));
        let remote = Self { container, port };
        let deadline = Instant::now() + Duration::from_secs(60);
        while with_docker && !remote.exec_ok(&["docker", "info"]) {
            assert!(Instant::now() < deadline, "the remote engine did not start");
            std::thread::sleep(Duration::from_millis(500));
        }
        remote
    }

    fn exec_ok(&self, args: &[&str]) -> bool {
        Command::new("docker")
            .args(["exec", &self.container])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn build_image() -> String {
    let image = format!("{}-test-ssh-remote", brand::PREFIX);
    let mut build = Command::new("docker")
        .args(["build", "-q", "-t", &image, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut build.stdin.take().unwrap(), DOCKERFILE.as_bytes()).unwrap();
    assert!(build.wait().unwrap().success(), "cannot build {image}");
    let _ = DIND_IMAGE;
    image
}

async fn add(api: &client::Client, name: &str, user: &str, port: u16) -> Server {
    let server = api
        .create_server(&CreateServer {
            name: name.into(),
            host: "127.0.0.1".into(),
            user: user.into(),
            port,
            ssh_key_secret_id: None,
        })
        .await
        .unwrap();
    api.check_server(&server.id).await.unwrap()
}

fn problem(s: &Server) -> &ServerProblem {
    s.problem
        .as_ref()
        .unwrap_or_else(|| panic!("{} has no problem: {s:?}", s.name))
}

fn statuses(s: &Server) -> Vec<CheckStatus> {
    s.checks.iter().map(|c| c.status).collect()
}

#[tokio::test]
async fn adding_remote_servers_explains_every_failure() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let api = daemon.client();
    let key = std::fs::read_to_string(daemon.dir.path().join("ssh/id_ed25519.pub")).unwrap();
    let image = build_image();
    let remote = Remote::start(&image, "remote", &key, true);
    let bare = Remote::start(&image, "bare", &key, false);
    use CheckStatus::{Fail, Pass, Skipped};

    // Root: every step passes, and the proxy (whose network inspect failed for Lasse) installs.
    let root = add(&api, "root", "root", remote.port).await;
    assert_eq!(root.status, ServerStatus::Online, "{root:?}");
    assert_eq!(root.problem, None);
    assert_eq!(statuses(&root), [Pass; 5]);
    let root = api.setup_proxy(&root.id).await.unwrap();
    assert_eq!(root.proxy, ProxyStatus::Running);

    // A non-root user in the docker group.
    let deploy = add(&api, "deploy", "deploy", remote.port).await;
    assert_eq!(deploy.status, ServerStatus::Online, "{deploy:?}");

    // A non-root user outside it.
    let nodocker = add(&api, "nodocker", "nodocker", remote.port).await;
    assert_eq!(nodocker.status, ServerStatus::Offline);
    let p = problem(&nodocker);
    assert_eq!(
        p.summary,
        "user nodocker can't access /var/run/docker.sock on 127.0.0.1"
    );
    assert_eq!(
        p.fix.as_deref(),
        Some("run: sudo usermod -aG docker nodocker, then reconnect")
    );
    assert_eq!(statuses(&nodocker), [Pass, Pass, Pass, Fail, Skipped]);
    let err = api.setup_proxy(&nodocker.id).await.unwrap_err().to_string();
    assert!(
        err.contains("can't access /var/run/docker.sock"),
        "the proxy install should explain itself: {err}"
    );

    // sshd refuses the forward for this user; only the tunnel itself can tell.
    let fwdoff = add(&api, "fwdoff", "fwdoff", remote.port).await;
    assert_eq!(fwdoff.status, ServerStatus::Offline);
    assert_eq!(
        problem(&fwdoff).summary,
        "sshd on 127.0.0.1 refuses to forward the docker socket"
    );
    assert_eq!(statuses(&fwdoff), [Pass, Pass, Pass, Pass, Fail]);

    // No Docker at all; root may install it.
    let missing = add(&api, "missing", "root", bare.port).await;
    assert_eq!(missing.status, ServerStatus::Offline);
    let p = problem(&missing);
    assert_eq!(p.summary, "docker isn't installed on 127.0.0.1");
    assert_eq!(
        p.fix.as_deref(),
        Some("install it with: curl -fsSL https://get.docker.com | sh")
    );
    assert!(p.can_install_docker);
    assert_eq!(statuses(&missing), [Pass, Fail, Skipped, Skipped, Skipped]);

    // The problem is stored with the server, so lists show it too.
    let listed = api.servers().await.unwrap();
    let stored = listed.iter().find(|s| s.id == missing.id).unwrap();
    assert_eq!(stored.problem, missing.problem);
    assert_eq!(stored.checks, missing.checks);

    // Once fixed, the next check clears the problem.
    assert!(remote.exec_ok(&["addgroup", "nodocker", "docker"]));
    let fixed = api.check_server(&nodocker.id).await.unwrap();
    assert_eq!(fixed.status, ServerStatus::Online, "{fixed:?}");
    assert_eq!(fixed.problem, None);
}
