//! A throwaway Mailpit container: an SMTP sink with an HTTP API to read what arrived. Tests skip
//! when Docker is unavailable unless `<ENV_PREFIX>_TEST_REQUIRE_DOCKER` is set.

use std::io::{Read as _, Write as _};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const IMAGE: &str = "axllent/mailpit:v1.34.0";

pub struct Server {
    pub smtp_port: u16,
    pub api: String,
    _reaper: ChildStdin,
}

static SERVER: OnceLock<Option<Server>> = OnceLock::new();

/// The shared Mailpit, or `None` when Docker is unavailable and the test should skip.
pub fn server() -> Option<&'static Server> {
    SERVER.get_or_init(start).as_ref()
}

fn port(container: &str, inner: &str) -> u16 {
    let out = Command::new("docker")
        .args(["port", container, inner])
        .output()
        .expect("docker port");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .and_then(|p| p.trim().parse().ok())
        .expect("published port")
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
        eprintln!("skipping SMTP tests: Docker is unavailable (set {require} to fail instead)");
        return None;
    }
    let container = format!(
        "{}-test-mailpit-{}",
        common::brand::PREFIX,
        std::process::id()
    );
    let out = Command::new("docker")
        .args(["run", "-d", "--rm", "--name", &container])
        .args(["-p", "127.0.0.1::1025", "-p", "127.0.0.1::8025", IMAGE])
        .output()
        .expect("docker run");
    assert!(
        out.status.success(),
        "cannot start Mailpit: {}",
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
    let smtp_port = port(&container, "1025/tcp");
    let api_port = port(&container, "8025/tcp");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !ready(api_port) {
        assert!(Instant::now() < deadline, "Mailpit did not start");
        std::thread::sleep(Duration::from_millis(200));
    }
    Some(Server {
        smtp_port,
        api: format!("http://127.0.0.1:{api_port}"),
        _reaper: reaper,
    })
}

fn ready(port: u16) -> bool {
    let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut status = [0u8; 12];
    stream
        .write_all(b"GET /readyz HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_ok()
        && stream.read_exact(&mut status).is_ok()
        && status.ends_with(b" 200")
}
