//! SSH access to remote servers through the system `ssh` binary and the daemon's own key.
//!
//! Each remote server gets one long-lived `ssh -L` process that forwards a local Unix socket to
//! the remote Docker socket; the Docker client then talks to that socket.

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
}

impl SshPaths {
    /// Options shared by every ssh invocation: our key only, no prompts, trust-on-first-use
    /// host keys recorded in the data directory, and no user ssh config.
    pub fn options(&self) -> Vec<String> {
        [
            "-F",
            "/dev/null",
            "-i",
            &self.key.to_string_lossy(),
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

impl Tunnel {
    pub async fn open(paths: &SshPaths, server: &Server) -> Result<Self> {
        let host = server.host.as_deref().context("SSH server has no host")?;
        let user = server.user.as_deref().unwrap_or("root");
        let port = server.port.unwrap_or(22);
        std::fs::create_dir_all(&paths.sockets)
            .with_context(|| format!("cannot create {}", paths.sockets.display()))?;
        let socket = paths.sockets.join(format!("{}.sock", server.id));
        let _ = std::fs::remove_file(&socket);
        let mut child = Command::new("ssh")
            .args(paths.options())
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
