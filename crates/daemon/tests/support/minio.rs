//! A throwaway MinIO container shared by every test in one test binary, with a fresh bucket per
//! test. Like the Postgres helper, tests skip when Docker is unavailable unless
//! `<ENV_PREFIX>_TEST_REQUIRE_DOCKER` is set (CI sets it).

use std::io::{Read as _, Write as _};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// A maintained community build of MinIO; the official images left Docker Hub in 2025.
pub const IMAGE: &str = "pgsty/minio:RELEASE.2026-08-04T00-00-00Z";
pub const ACCESS_KEY: &str = "test-access-key";
pub const SECRET_KEY: &str = "test-secret-key-0123456789";
pub const REGION: &str = "us-east-1";

struct Server {
    endpoint: String,
    _reaper: ChildStdin,
}

static SERVER: OnceLock<Option<Server>> = OnceLock::new();
static NEXT: AtomicU32 = AtomicU32::new(0);

/// Endpoint and a new, empty bucket, or `None` when Docker is unavailable and the test should
/// skip.
pub async fn bucket() -> Option<(String, String)> {
    let endpoint = SERVER.get_or_init(start).as_ref()?.endpoint.clone();
    let name = format!(
        "t{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let credentials =
        s3::creds::Credentials::new(Some(ACCESS_KEY), Some(SECRET_KEY), None, None, None).unwrap();
    let region = s3::Region::Custom {
        region: REGION.into(),
        endpoint: endpoint.clone(),
    };
    s3::Bucket::create_with_path_style(
        &name,
        region,
        credentials,
        s3::BucketConfiguration::default(),
    )
    .await
    .expect("create the test bucket");
    Some((endpoint, name))
}

/// A client for a test bucket, to look at what the daemon stored.
pub fn client(endpoint: &str, bucket: &str) -> Box<s3::Bucket> {
    let credentials =
        s3::creds::Credentials::new(Some(ACCESS_KEY), Some(SECRET_KEY), None, None, None).unwrap();
    let region = s3::Region::Custom {
        region: REGION.into(),
        endpoint: endpoint.into(),
    };
    s3::Bucket::new(bucket, region, credentials)
        .unwrap()
        .with_path_style()
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
            "skipping MinIO-backed tests: Docker is unavailable (set {require} to fail instead)"
        );
        return None;
    }
    let container = format!(
        "{}-test-minio-{}",
        common::brand::PREFIX,
        std::process::id()
    );
    let out = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            &container,
            "-p",
            "127.0.0.1::9000",
        ])
        .args(["-e", &format!("MINIO_ROOT_USER={ACCESS_KEY}")])
        .args(["-e", &format!("MINIO_ROOT_PASSWORD={SECRET_KEY}")])
        .args(["--tmpfs", "/data", IMAGE, "server", "/data"])
        .output()
        .expect("docker run");
    assert!(
        out.status.success(),
        "cannot start MinIO: {}",
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
        .args(["port", &container, "9000/tcp"])
        .output()
        .expect("docker port");
    let port: u16 = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .and_then(|p| p.trim().parse().ok())
        .expect("published port");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !ready(port) {
        assert!(Instant::now() < deadline, "MinIO did not start");
        std::thread::sleep(Duration::from_millis(200));
    }
    Some(Server {
        endpoint: format!("http://127.0.0.1:{port}"),
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
        .write_all(b"GET /minio/health/ready HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_ok()
        && stream.read_exact(&mut status).is_ok()
        && status.ends_with(b" 200")
}
