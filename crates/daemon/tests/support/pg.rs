//! A throwaway Postgres 18 container shared by every test in one test binary, with a fresh
//! database per test. Tests skip when Docker is unavailable, unless
//! `<ENV_PREFIX>_TEST_REQUIRE_DOCKER` is set (CI sets it), which turns the skip into a failure.

use std::process::{ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

struct Server {
    container: String,
    port: u16,
    /// Closing this pipe (when the test process exits) makes a watcher remove the container.
    _reaper: ChildStdin,
}

static SERVER: OnceLock<Option<Server>> = OnceLock::new();
static NEXT: AtomicU32 = AtomicU32::new(0);

/// URL of a new, empty database, or `None` when Docker is unavailable and the test should skip.
pub fn database_url() -> Option<String> {
    let server = SERVER.get_or_init(start).as_ref()?;
    let name = format!("t{}", NEXT.fetch_add(1, Ordering::Relaxed));
    psql(server, &format!("CREATE DATABASE {name}"));
    Some(format!(
        "postgres://postgres:test@127.0.0.1:{}/{name}?sslmode=disable",
        server.port
    ))
}

fn start() -> Option<Server> {
    let docker_ok = Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let require = common::brand::env_var("TEST_REQUIRE_DOCKER");
    if !docker_ok {
        assert!(
            std::env::var_os(&require).is_none(),
            "{require} is set but Docker is unavailable"
        );
        eprintln!(
            "skipping Postgres-backed tests: Docker is unavailable (set {require} to fail instead)"
        );
        return None;
    }
    let container = format!("{}-test-pg-{}", common::brand::PREFIX, std::process::id());
    let out = Command::new("docker")
        .args(["run", "-d", "--rm", "--name", &container])
        .args(["-e", "POSTGRES_PASSWORD=test", "-p", "127.0.0.1::5432"])
        .args(["--tmpfs", "/var/lib/postgresql", "postgres:18-alpine"])
        .args(["-c", "fsync=off", "-c", "max_connections=500"])
        .output()
        .expect("docker run");
    assert!(
        out.status.success(),
        "cannot start the test Postgres: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reaper = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "trap '' INT TERM; read _; docker rm -f {container} >/dev/null 2>&1"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the container reaper")
        .stdin
        .take()
        .expect("reaper stdin");
    let out = Command::new("docker")
        .args(["port", &container, "5432/tcp"])
        .output()
        .expect("docker port");
    let port = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .and_then(|p| p.trim().parse().ok())
        .expect("published port");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !Command::new("docker")
        .args(["exec", &container, "pg_isready", "-q", "-h", "127.0.0.1"])
        .args(["-U", "postgres"])
        .status()
        .is_ok_and(|s| s.success())
    {
        assert!(Instant::now() < deadline, "the test Postgres did not start");
        std::thread::sleep(Duration::from_millis(200));
    }
    Some(Server {
        container,
        port,
        _reaper: reaper,
    })
}

fn psql(server: &Server, sql: &str) {
    let out = Command::new("docker")
        .args(["exec", &server.container, "psql", "-q", "-U", "postgres"])
        .args(["-v", "ON_ERROR_STOP=1", "-c", sql])
        .output()
        .expect("docker exec psql");
    assert!(
        out.status.success(),
        "{sql}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
