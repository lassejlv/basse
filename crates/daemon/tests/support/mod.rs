//! Spawns the real daemon binary in a temporary data directory.
#![allow(
    dead_code,
    reason = "shared by several test crates, each using a subset"
)]

use client::Client;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub struct Daemon {
    child: Child,
    pub url: String,
    pub token: String,
    pub dir: TempDir,
}

impl Daemon {
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let out = daemon_cmd(dir.path())
            .args(["init", "--no-proxy"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        let token = token_from(&stdout);
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = daemon_cmd(dir.path())
            .args(["--listen", &format!("127.0.0.1:{port}"), "serve"])
            .stdout(Stdio::null())
            .stderr(if std::env::var_os("DAEMON_LOGS").is_some() {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .spawn()
            .unwrap();
        let url = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(res) = reqwest::get(format!("{url}/healthz")).await
                && res.status().is_success()
            {
                break;
            }
            assert!(Instant::now() < deadline, "daemon did not come up");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Self {
            child,
            url,
            token,
            dir,
        }
    }

    pub fn client(&self) -> Client {
        Client::new(&self.url, &self.token).unwrap()
    }

    pub fn run(&self, args: &[&str]) -> std::process::Output {
        daemon_cmd(self.dir.path()).args(args).output().unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Path of the daemon binary Cargo built for these tests.
pub fn daemon_bin() -> String {
    let var = format!("CARGO_BIN_EXE_{}", common::brand::DAEMON_BIN);
    std::env::var(&var).unwrap_or_else(|_| panic!("{var} is not set; run through cargo test"))
}

fn daemon_cmd(data_dir: &Path) -> Command {
    let mut cmd = Command::new(daemon_bin());
    cmd.arg("--data-dir").arg(data_dir);
    cmd
}

/// The token is the line after the "API token" heading in the `init` / `token rotate` output.
pub fn token_from(stdout: &str) -> String {
    let mut lines = stdout.lines();
    lines.find(|l| l.contains("API token"));
    lines.next().expect("token line").trim().to_string()
}
