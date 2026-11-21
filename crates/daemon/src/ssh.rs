//! SSH access to remote servers through the system `ssh` binary, with the daemon's own key or
//! a server's `ssh_key` secret.
//!
//! Each remote server gets one long-lived `ssh -L` process that forwards a local Unix socket to
//! the remote Docker socket; the Docker client then talks to that socket.
//!
//! A secret key is handed to `ssh` as a file that exists only while the connection is being
//! set up: it is created with mode 0600 in `<data dir>/ssh/keys` (itself 0700) and deleted as
//! soon as the tunnel is up or has failed, since `ssh` reads its identity only during
//! authentication. An ssh-agent would avoid the file, but it means a second long-lived process
//! holding every key in memory behind a socket any process of the daemon's user can use, and it
//! has to be supervised and restarted; the file lives for the seconds of one handshake.

use anyhow::{Context as _, Result, bail};
use common::Server;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
use tokio::process::{Child, Command};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const REMOTE_DOCKER_SOCKET: &str = "/var/run/docker.sock";

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

/// Removes key files a crashed daemon may have left behind.
pub fn clear_temp_keys(paths: &SshPaths) {
    if let Ok(entries) = std::fs::read_dir(&paths.keys) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

impl Tunnel {
    /// Connects with `private_key` when set, otherwise with the daemon's own key.
    pub async fn open(
        paths: &SshPaths,
        server: &Server,
        private_key: Option<&str>,
    ) -> Result<Self> {
        let host = server.host.as_deref().context("SSH server has no host")?;
        let user = server.user.as_deref().unwrap_or("root");
        let port = server.port.unwrap_or(22);
        std::fs::create_dir_all(&paths.sockets)
            .with_context(|| format!("cannot create {}", paths.sockets.display()))?;
        let socket = paths.sockets.join(format!("{}.sock", server.id));
        let _ = std::fs::remove_file(&socket);
        let temp_key = private_key
            .map(|key| TempKey::write(&paths.keys, &server.id, key))
            .transpose()?;
        let options = match &temp_key {
            Some(key) => paths.options_with_key(&key.0),
            None => paths.options(),
        };
        let mut child = Command::new("ssh")
            .args(options)
            .args(["-N", "-T", "-p", &port.to_string()])
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
            .arg(format!("{}:{REMOTE_DOCKER_SOCKET}", socket.display()))
            .arg(format!("{user}@{host}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("cannot run ssh (install openssh-client)")?;
        let target = format!("{user}@{host}:{port}");
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
        drop(temp_key);
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
}
