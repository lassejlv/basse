//! Spawns the real daemon binary in a temporary data directory.
#![allow(
    dead_code,
    reason = "shared by several test crates, each using a subset"
)]

pub mod mailpit;
pub mod minio;
pub mod pg;

use client::Client;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The Docker engine a test daemon's local server talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// None: `DOCKER_HOST` points at a socket that does not exist, so the daemon can neither
    /// see nor change anything on the host's engine. Every daemon on the host engine shares its
    /// one proxy container, `brand::PROXY_CONTAINER` on ports 80 and 443, and recreates a
    /// running one at startup and on every instance domain change. With a leftover proxy
    /// (or a developer's real one) on the host, parallel tests removed it under each other
    /// ("removal of container ... is already in progress") and replaced the real one.
    None,
    /// The host's engine, for tests that deploy for real. They genuinely share the host proxy,
    /// so each daemon holds `HOST_ENGINE` while it serves and they run one at a time. A test
    /// must drop one before it serves the next.
    Host,
}

/// Held by every serving daemon on the host engine; see [`Engine::Host`].
static HOST_ENGINE: Mutex<()> = Mutex::new(());

pub struct Daemon {
    child: Child,
    pub url: String,
    pub token: String,
    pub dir: TempDir,
    engine: Engine,
    _host: Option<MutexGuard<'static, ()>>,
}

impl Daemon {
    /// Initializes and serves a daemon without a Docker engine on its own Postgres database,
    /// or `None` when the tests must skip because Docker is unavailable.
    pub async fn start() -> Option<Self> {
        Self::start_with(Engine::None, |_| Vec::new()).await
    }

    /// Like `start`, on the host's Docker engine.
    pub async fn start_on_host() -> Option<Self> {
        Self::start_with(Engine::Host, |_| Vec::new()).await
    }

    /// Like `start` on `engine`, running `prepare` on the data directory between `init` and
    /// `serve`; `serve` gets the environment variables it returns.
    pub async fn start_with(
        engine: Engine,
        prepare: impl FnOnce(&Path) -> Vec<(String, String)>,
    ) -> Option<Self> {
        let database_url = pg::database_url()?;
        let dir = tempfile::tempdir().unwrap();
        let out = daemon_cmd(dir.path(), engine)
            .args(["init", "--no-proxy", "--database-url", &database_url])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        let token = token_from(&stdout);
        let serve_env = prepare(dir.path());
        Some(Self::serve(dir, token, serve_env, engine).await)
    }

    /// Serves an existing data directory on `engine`, e.g. one a restore wrote, with its API
    /// token.
    pub async fn serve(
        dir: TempDir,
        token: String,
        serve_env: Vec<(String, String)>,
        engine: Engine,
    ) -> Self {
        let host =
            (engine == Engine::Host).then(|| HOST_ENGINE.lock().unwrap_or_else(|p| p.into_inner()));
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = daemon_cmd(dir.path(), engine)
            .args(["--listen", &format!("127.0.0.1:{port}"), "serve"])
            .envs(serve_env)
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
            engine,
            _host: host,
        }
    }

    pub fn client(&self) -> Client {
        Client::new(&self.url, &self.token).unwrap()
    }

    pub fn run(&self, args: &[&str]) -> std::process::Output {
        daemon_cmd(self.dir.path(), self.engine)
            .args(args)
            .output()
            .unwrap()
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

/// The daemon binary on `data_dir` and `engine`.
pub fn daemon_cmd(data_dir: &Path, engine: Engine) -> Command {
    let mut cmd = Command::new(daemon_bin());
    cmd.arg("--data-dir").arg(data_dir);
    if engine == Engine::None {
        cmd.env(
            "DOCKER_HOST",
            format!("unix://{}", data_dir.join("no-docker.sock").display()),
        );
    }
    cmd
}

/// The token is the line after the "API token" heading in the `init` / `token rotate` output.
pub fn token_from(stdout: &str) -> String {
    let mut lines = stdout.lines();
    lines.find(|l| l.contains("API token"));
    lines.next().expect("token line").trim().to_string()
}
