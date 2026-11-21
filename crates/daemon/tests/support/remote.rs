//! A second Docker host for remote-server tests: Docker-in-Docker with sshd, reached through
//! the daemon's SSH tunnel like any other server.

use common::brand;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DIND_IMAGE: &str = "docker:29-dind";

pub fn run(dir: &Path, program: &str, args: &[&str]) -> String {
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
pub struct RemoteHost {
    pub container: String,
    pub port: u16,
}

impl RemoteHost {
    pub fn start(authorized_key: &Path) -> Self {
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

    pub fn exec(&self, args: &[&str]) -> Result<String, String> {
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

    pub fn try_docker(&self, args: &[&str]) -> Result<String, String> {
        self.exec(&[&["docker"], args].concat())
    }

    /// `docker <args>` against the remote engine.
    pub fn docker(&self, args: &[&str]) -> String {
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
