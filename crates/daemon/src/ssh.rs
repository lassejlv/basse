//! SSH access to remote servers through the system `ssh` binary, with the daemon's own key or
//! a server's `ssh_key` secret.
//!
//! Each remote server gets one long-lived `ssh -L` process that forwards a local Unix socket to
//! the remote Docker socket; the Docker client then talks to that socket. Before the tunnel
//! opens, a preflight over SSH (see [`crate::preflight`]) checks that there is a Docker to reach,
//! and after it opens, the daemon waits until Docker answers a ping through it.
//!
//! A secret key is handed to `ssh` as a file that exists only while the connection is being
//! set up: it is created with mode 0600 in `<data dir>/ssh/keys` (itself 0700) and deleted as
//! soon as the tunnel is up or has failed, since `ssh` reads its identity only during
//! authentication. An ssh-agent would avoid the file, but it means a second long-lived process
//! holding every key in memory behind a socket any process of the daemon's user can use, and it
//! has to be supervised and restarted; the file lives for the seconds of one handshake.

use crate::preflight::{self, DOCKER_SOCKET, Facts};
use anyhow::{Context as _, Result, bail};
use common::{PreflightCheck, Server, ServerProblem};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Refused connections through a fresh tunnel before the forward counts as refused by sshd.
const REFUSALS: u32 = 3;

/// Key and state files the daemon's ssh invocations use instead of the user's `~/.ssh`.
#[derive(Debug, Clone)]
pub struct SshPaths {
    pub key: PathBuf,
    pub known_hosts: PathBuf,
    pub sockets: PathBuf,
    /// Where secret keys are written for the duration of a handshake.
    pub keys: PathBuf,
}

impl SshPaths {
    /// Options shared by every ssh invocation with the daemon's own key.
    pub fn options(&self) -> Vec<String> {
        self.options_with_key(&self.key)
    }

    /// Options shared by every ssh invocation: `key` only, no prompts, trust-on-first-use
    /// host keys recorded in the data directory, and no user ssh config.
    fn options_with_key(&self, key: &Path) -> Vec<String> {
        [
            "-F",
            "/dev/null",
            "-i",
            &key.to_string_lossy(),
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            &format!("UserKnownHostsFile={}", self.known_hosts.display()),
            "-o",
            "ConnectTimeout=10",
            "-o",
            "LogLevel=ERROR",
        ]
        .map(str::to_string)
        .to_vec()
    }

    /// `GIT_SSH_COMMAND` value that applies the same options to git.
    pub fn git_ssh_command(&self) -> String {
        std::iter::once("ssh".to_string())
            .chain(self.options().iter().map(|a| shell_quote(a)))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// A remote server that cannot be used, with the preflight checklist that found out why.
#[derive(Debug, Clone)]
pub struct Unreachable {
    pub checks: Vec<PreflightCheck>,
    pub problem: ServerProblem,
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.problem.summary)?;
        match &self.problem.fix {
            Some(fix) => write!(f, " · {fix}"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for Unreachable {}

/// A running `ssh -N -L <socket>:/var/run/docker.sock` process.
pub struct Tunnel {
    child: Child,
    pub socket: PathBuf,
}

/// A private key file that is deleted when dropped.
struct TempKey(PathBuf);

impl TempKey {
    fn write(dir: &Path, server_id: &str, private_key: &str) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        crate::config::set_mode(dir, 0o700)?;
        let path = dir.join(format!("{server_id}-{}.key", crate::db::new_id()));
        crate::secrets::write_private(&path, private_key.as_bytes())?;
        Ok(Self(path))
    }
}

impl Drop for TempKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Removes key files and tunnel sockets a crashed daemon may have left behind.
pub fn clear_temp_files(paths: &SshPaths) {
    for dir in [&paths.keys, &paths.sockets] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// How to log in to one server: the ssh options with its key, and `user@host`.
struct Login {
    options: Vec<String>,
    port: String,
    destination: String,
    target: String,
    _key: Option<TempKey>,
}

impl Login {
    fn new(paths: &SshPaths, server: &Server, private_key: Option<&str>) -> Result<Self> {
        let host = server.host.as_deref().context("SSH server has no host")?;
        let user = server.user.as_deref().unwrap_or("root");
        let port = server.port.unwrap_or(22);
        let key = private_key
            .map(|key| TempKey::write(&paths.keys, &server.id, key))
            .transpose()?;
        let options = match &key {
            Some(key) => paths.options_with_key(&key.0),
            None => paths.options(),
        };
        Ok(Self {
            options,
            port: port.to_string(),
            destination: format!("{user}@{host}"),
            target: format!("{user}@{host}:{port}"),
            _key: key,
        })
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(&self.options).args(["-p", &self.port]);
        cmd
    }

    /// Runs a POSIX sh script on the server, fed through stdin.
    async fn run(&self, script: &str, timeout: Duration) -> Result<std::process::Output> {
        let mut child = self
            .command()
            .args(["-T", &self.destination, "sh", "-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("cannot run ssh (install openssh-client)")?;
        let mut stdin = child.stdin.take().context("ssh stdin")?;
        stdin.write_all(script.as_bytes()).await?;
        drop(stdin);
        tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .with_context(|| format!("ssh to {} timed out", self.target))?
            .context("ssh failed")
    }
}

/// Runs a POSIX sh script on a server over SSH and returns its stdout, or an error with its
/// stderr when it fails.
pub async fn run(
    paths: &SshPaths,
    server: &Server,
    private_key: Option<&str>,
    script: &str,
    timeout: Duration,
) -> Result<String> {
    let login = Login::new(paths, server, private_key)?;
    let out = login.run(script, timeout).await?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stdout.lines().chain(stderr.lines()).collect();
        bail!(
            "the script on {} failed ({}): {}",
            login.target,
            out.status,
            tail[tail.len().saturating_sub(15)..].join("\n")
        );
    }
    Ok(stdout)
}

/// Whether Docker answered through a tunnel socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    Answered,
    /// The socket accepted connections but closed them without an answer: the remote end
    /// refused the forward.
    Refused,
    /// Nothing listened on the socket in time.
    TimedOut,
}

/// Waits until Docker answers `GET /_ping` through `socket`. A socket that does not exist yet or
/// does not accept connections yet is retried until `timeout`, since ssh creates the file before
/// it listens.
pub async fn wait_for_docker(socket: &Path, timeout: Duration) -> Ready {
    let deadline = Instant::now() + timeout;
    let mut refused = 0;
    loop {
        match ping(socket).await {
            Some(true) => return Ready::Answered,
            Some(false) => {
                refused += 1;
                if refused >= REFUSALS {
                    return Ready::Refused;
                }
            }
            None => {}
        }
        if Instant::now() >= deadline {
            return if refused > 0 {
                Ready::Refused
            } else {
                Ready::TimedOut
            };
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `None` when nothing accepts connections on the socket, otherwise whether an HTTP answer came.
async fn ping(socket: &Path) -> Option<bool> {
    let mut stream = UnixStream::connect(socket).await.ok()?;
    let exchange = async {
        stream
            .write_all(b"GET /_ping HTTP/1.0\r\nHost: docker\r\n\r\n")
            .await?;
        let mut head = [0u8; 5];
        stream.read_exact(&mut head).await?;
        std::io::Result::Ok(&head == b"HTTP/")
    };
    Some(matches!(
        tokio::time::timeout(Duration::from_secs(5), exchange).await,
        Ok(Ok(true))
    ))
}

impl Tunnel {
    /// Checks the server over SSH, then opens the tunnel and waits until Docker answers through
    /// it. Connects with `private_key` when set (`key_name` names its secret in hints), otherwise
    /// with the daemon's own key. Fails with [`Unreachable`] when the server cannot be used, and
    /// returns the preflight checklist on success.
    pub async fn connect(
        paths: &SshPaths,
        server: &Server,
        private_key: Option<&str>,
        key_name: Option<&str>,
    ) -> Result<(Self, Vec<PreflightCheck>)> {
        let login = Login::new(paths, server, private_key)?;
        let out = login.run(preflight::SCRIPT, CONNECT_TIMEOUT).await?;
        if out.status.code() == Some(255) {
            let report = preflight::login_failed(
                server,
                key_name,
                &paths.known_hosts.to_string_lossy(),
                &String::from_utf8_lossy(&out.stderr),
            );
            return Err(unreachable(report.checks, report.problem));
        }
        let report =
            preflight::diagnose(server, &Facts::parse(&String::from_utf8_lossy(&out.stdout)));
        let mut checks = report.checks;
        if report.problem.is_some() {
            return Err(unreachable(checks, report.problem));
        }
        let tunnel = Self::open(paths, server, &login).await?;
        let ready = wait_for_docker(&tunnel.socket, CONNECT_TIMEOUT).await;
        if ready == Ready::TimedOut {
            bail!(
                "the SSH tunnel to {} did not open within {}s",
                login.target,
                CONNECT_TIMEOUT.as_secs()
            );
        }
        let refused = preflight::tunnel_result(server, &mut checks, ready == Ready::Answered);
        if refused.is_some() {
            return Err(unreachable(checks, refused));
        }
        Ok((tunnel, checks))
    }

    async fn open(paths: &SshPaths, server: &Server, login: &Login) -> Result<Self> {
        std::fs::create_dir_all(&paths.sockets)
            .with_context(|| format!("cannot create {}", paths.sockets.display()))?;
        let socket = paths.sockets.join(format!("{}.sock", server.id));
        let _ = std::fs::remove_file(&socket);
        let mut child = login
            .command()
            .args(["-N", "-T"])
            .args([
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "StreamLocalBindUnlink=yes",
            ])
            .args([
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
            ])
            .arg("-L")
            .arg(format!("{}:{DOCKER_SOCKET}", socket.display()))
            .arg(&login.destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("cannot run ssh (install openssh-client)")?;
        let target = login.target.clone();
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        while !socket.exists() {
            if let Some(status) = child.try_wait()? {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr).await;
                }
                bail!("ssh to {target} failed ({status}): {}", stderr.trim());
            }
            if Instant::now() >= deadline {
                bail!("ssh to {target} timed out");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(server = %target, "ssh: {line}");
                }
            });
        }
        Ok(Self { child, socket })
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

fn unreachable(checks: Vec<PreflightCheck>, problem: Option<ServerProblem>) -> anyhow::Error {
    match problem {
        Some(problem) => Unreachable { checks, problem }.into(),
        None => anyhow::anyhow!("the preflight found no problem"),
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Path helper so callers need not know the layout.
pub fn paths(ssh_dir: &Path) -> SshPaths {
    SshPaths {
        key: ssh_dir.join("id_ed25519"),
        known_hosts: ssh_dir.join("known_hosts"),
        sockets: ssh_dir.join("sockets"),
        keys: ssh_dir.join("keys"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    #[test]
    fn git_ssh_command_quotes_paths() {
        let p = paths(Path::new("/data dir/ssh"));
        let cmd = p.git_ssh_command();
        assert!(
            cmd.starts_with("ssh '-F' '/dev/null' '-i' '/data dir/ssh/id_ed25519'"),
            "{cmd}"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    /// Answers every connection like Docker's `/_ping`.
    fn serve_docker(listener: UnixListener) {
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                    .await;
            }
        });
    }

    #[tokio::test]
    async fn waits_for_a_socket_that_appears_late() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("late.sock");
        let path = socket.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            serve_docker(UnixListener::bind(path).unwrap());
        });
        let started = Instant::now();
        assert_eq!(
            wait_for_docker(&socket, Duration::from_secs(5)).await,
            Ready::Answered
        );
        assert!(started.elapsed() >= Duration::from_millis(400));
    }

    #[tokio::test]
    async fn a_stale_socket_file_is_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        assert!(socket.exists());
        assert_eq!(
            wait_for_docker(&socket, Duration::from_millis(300)).await,
            Ready::TimedOut
        );
    }

    #[tokio::test]
    async fn a_socket_that_drops_every_connection_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("refused.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        assert_eq!(
            wait_for_docker(&socket, Duration::from_secs(5)).await,
            Ready::Refused
        );
    }
}
