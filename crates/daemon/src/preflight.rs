//! The preflight the daemon runs over SSH before it tunnels to a remote server's Docker socket,
//! and the diagnosis that turns its findings into a checklist and one actionable problem.
//!
//! A failed tunnel looks the same whatever the cause: ssh accepts the local connection, the
//! server refuses the channel, and the Docker client sees a reset connection. So the daemon asks
//! the server first, in one SSH session, and only tunnels when every step passed.

use common::{CheckStatus, PreflightCheck, Server, ServerProblem, brand};
use std::collections::HashMap;

/// The Docker socket the tunnel forwards to.
pub const DOCKER_SOCKET: &str = "/var/run/docker.sock";
const INSTALL_HINT: &str = "curl -fsSL https://get.docker.com | sh";
pub const FORWARD_LABEL: &str = "sshd forwards the docker socket";
const FORWARDING_FIX: &str = "set AllowTcpForwarding yes and AllowStreamLocalForwarding yes \
     (and no DisableForwarding) in /etc/ssh/sshd_config, then restart sshd";

/// Reports one `key=value` line per finding. POSIX sh, so it runs under dash, ash and bash.
pub const SCRIPT: &str = r#"s=/var/run/docker.sock
echo "user=$(id -un)"
echo "uid=$(id -u)"
command -v docker >/dev/null 2>&1 && echo docker_cli=1
if [ -S "$s" ]; then
  echo socket=1
  echo "socket_mode=$(ls -lL "$s" | awk '{print $1, $3, $4}')"
  [ -r "$s" ] && [ -w "$s" ] && echo socket_access=1
fi
if command -v docker >/dev/null 2>&1; then
  t=; command -v timeout >/dev/null 2>&1 && t="timeout 10"
  if v=$($t docker version --format '{{.Server.Version}}' 2>&1); then
    echo "docker_version=$v"
  else
    echo "docker_error=$(printf '%s' "$v" | tail -n 1)"
  fi
fi
if [ "$(id -u)" = 0 ]; then
  d=$(command -v sshd || echo /usr/sbin/sshd)
  "$d" -T 2>/dev/null | grep -Ei '^(allowtcpforwarding|allowstreamlocalforwarding|disableforwarding) ' | sed 's/^/sshd_/;s/ /=/'
else
  sudo -n true 2>/dev/null && echo sudo=1
fi
r="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/docker.sock"
[ -S "$r" ] && echo "rootless_socket=$r"
command -v systemctl >/dev/null 2>&1 && echo systemd=1
exit 0
"#;

/// Installs Docker with get.docker.com as root or through passwordless sudo, starts it, and
/// lets a non-root SSH user use its socket.
pub const INSTALL_SCRIPT: &str = r#"set -e
if [ "$(id -u)" = 0 ]; then s=; else s="sudo -n"; fi
f=$(mktemp)
if command -v curl >/dev/null 2>&1; then
  curl -fsSL https://get.docker.com -o "$f"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$f" https://get.docker.com
else
  echo "neither curl nor wget is installed" >&2
  exit 1
fi
$s sh "$f"
rm -f "$f"
if command -v systemctl >/dev/null 2>&1; then $s systemctl enable --now docker || true; fi
[ "$(id -u)" = 0 ] || $s usermod -aG docker "$(id -un)"
"#;

/// What the preflight script found on the server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub user: String,
    pub uid: Option<u32>,
    pub docker_cli: bool,
    pub socket: bool,
    /// Mode, owner and group of the socket, e.g. `srw-rw---- root docker`.
    pub socket_mode: Option<String>,
    pub socket_access: bool,
    pub docker_version: Option<String>,
    pub docker_error: Option<String>,
    pub sudo: bool,
    pub rootless_socket: Option<String>,
    pub systemd: bool,
    /// Forwarding options from `sshd -T`, lowercase, when the user is root.
    pub sshd: HashMap<String, String>,
}

impl Facts {
    pub fn parse(output: &str) -> Self {
        let mut f = Self::default();
        for line in output.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().to_string();
            match key.trim() {
                "user" => f.user = value,
                "uid" => f.uid = value.parse().ok(),
                "docker_cli" => f.docker_cli = true,
                "socket" => f.socket = true,
                "socket_mode" => f.socket_mode = Some(value),
                "socket_access" => f.socket_access = true,
                "docker_version" if !value.is_empty() => f.docker_version = Some(value),
                "docker_error" if !value.is_empty() => f.docker_error = Some(value),
                "sudo" => f.sudo = true,
                "rootless_socket" => f.rootless_socket = Some(value),
                "systemd" => f.systemd = true,
                key => {
                    if let Some(option) = key.strip_prefix("sshd_") {
                        f.sshd.insert(option.to_lowercase(), value.to_lowercase());
                    }
                }
            }
        }
        f
    }

    fn root(&self) -> bool {
        self.uid == Some(0)
    }

    /// The group that owns the socket, which a non-root user must join.
    fn socket_group(&self) -> &str {
        self.socket_mode
            .as_deref()
            .and_then(|m| m.split_whitespace().nth(2))
            .unwrap_or("docker")
    }

    /// The sshd option that stops the tunnel, as `sshd_config` spells it.
    fn forwarding_blocked(&self) -> Option<&'static str> {
        let local = |option: &str| {
            self.sshd
                .get(option)
                .is_none_or(|v| matches!(v.as_str(), "yes" | "all" | "local"))
        };
        if self
            .sshd
            .get("disableforwarding")
            .is_some_and(|v| v == "yes")
        {
            Some("DisableForwarding yes")
        } else if !local("allowtcpforwarding") {
            Some("AllowTcpForwarding no")
        } else if !local("allowstreamlocalforwarding") {
            Some("AllowStreamLocalForwarding no")
        } else {
            None
        }
    }
}

/// A preflight checklist and, when a step failed, the problem it points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<PreflightCheck>,
    pub problem: Option<ServerProblem>,
}

fn check(label: impl Into<String>, status: CheckStatus, detail: Option<String>) -> PreflightCheck {
    PreflightCheck {
        label: label.into(),
        status,
        detail,
    }
}

fn problem(summary: String, fix: Option<String>) -> ServerProblem {
    ServerProblem {
        detail: summary.clone(),
        summary,
        fix,
        can_install_docker: false,
    }
}

fn host(server: &Server) -> &str {
    server.host.as_deref().unwrap_or("the server")
}

fn ssh_label(server: &Server) -> String {
    format!(
        "ssh {}@{}:{}",
        server.user.as_deref().unwrap_or("root"),
        host(server),
        server.port.unwrap_or(22)
    )
}

/// Every step in order, so a failed preflight still shows what it did not get to.
fn labels(user: &str) -> [String; 4] {
    [
        "docker installed".into(),
        "docker running".into(),
        format!("{user} can use {DOCKER_SOCKET}"),
        FORWARD_LABEL.into(),
    ]
}

fn failed(
    mut checks: Vec<PreflightCheck>,
    user: &str,
    step: PreflightCheck,
    p: ServerProblem,
) -> Report {
    checks.push(step);
    for label in labels(user) {
        if !checks.iter().any(|c| c.label == label) {
            checks.push(check(label, CheckStatus::Skipped, None));
        }
    }
    Report {
        checks,
        problem: Some(p),
    }
}

/// Turns the facts into the checklist and the first problem, in the order a person would fix
/// them: install Docker, start it, get access to its socket, then let sshd forward it.
pub fn diagnose(server: &Server, f: &Facts) -> Report {
    let host = host(server);
    let user = match f.user.as_str() {
        "" => server.user.as_deref().unwrap_or("root"),
        user => user,
    };
    let [installed, running, access, _] = labels(user);
    let mut checks = vec![check(
        ssh_label(server),
        CheckStatus::Pass,
        Some(match f.uid {
            Some(uid) => format!("logged in as {user} (uid {uid})"),
            None => format!("logged in as {user}"),
        }),
    )];

    if !f.socket {
        if let Some(rootless) = &f.rootless_socket {
            let p = problem(
                format!(
                    "docker on {host} runs rootless; {} needs the system engine at {DOCKER_SOCKET}",
                    brand::NAME
                ),
                Some(format!("install it with: {INSTALL_HINT}")),
            );
            let step = check(
                installed,
                CheckStatus::Fail,
                Some(format!("only a rootless engine at {rootless}")),
            );
            return failed(checks, user, step, p);
        }
        if !f.docker_cli {
            let mut p = problem(
                format!("docker isn't installed on {host}"),
                Some(format!("install it with: {INSTALL_HINT}")),
            );
            p.can_install_docker = f.root() || f.sudo;
            let step = check(
                installed,
                CheckStatus::Fail,
                Some("no docker command and no docker socket".into()),
            );
            return failed(checks, user, step, p);
        }
        checks.push(check(installed, CheckStatus::Pass, None));
        let fix = if f.systemd {
            "start it with: sudo systemctl enable --now docker"
        } else {
            "start the docker daemon (dockerd)"
        };
        let p = problem(
            format!("docker is installed on {host} but not running"),
            Some(fix.into()),
        );
        let step = check(
            running,
            CheckStatus::Fail,
            Some(format!("{DOCKER_SOCKET} doesn't exist")),
        );
        return failed(checks, user, step, p);
    }

    checks.push(check(
        installed,
        CheckStatus::Pass,
        f.docker_version.as_ref().map(|v| format!("docker {v}")),
    ));
    if !f.socket_access {
        checks.push(check(running, CheckStatus::Pass, None));
        let p = problem(
            format!("user {user} can't access {DOCKER_SOCKET} on {host}"),
            Some(format!(
                "run: sudo usermod -aG {} {user}, then reconnect",
                f.socket_group()
            )),
        );
        let step = check(access, CheckStatus::Fail, f.socket_mode.clone());
        return failed(checks, user, step, p);
    }
    if let (None, Some(error)) = (&f.docker_version, &f.docker_error) {
        let fix = if f.systemd {
            "restart it with: sudo systemctl restart docker"
        } else {
            "restart the docker daemon (dockerd)"
        };
        let mut p = problem(format!("docker on {host} doesn't answer"), Some(fix.into()));
        p.detail = format!("{}: {error}", p.summary);
        let step = check(running, CheckStatus::Fail, Some(error.clone()));
        return failed(checks, user, step, p);
    }
    checks.push(check(running, CheckStatus::Pass, None));
    checks.push(check(access, CheckStatus::Pass, f.socket_mode.clone()));
    if let Some(option) = f.forwarding_blocked() {
        let p = problem(
            format!("sshd on {host} has {option}"),
            Some(FORWARDING_FIX.into()),
        );
        let step = check(FORWARD_LABEL, CheckStatus::Fail, Some(option.into()));
        return failed(checks, user, step, p);
    }
    checks.push(check(FORWARD_LABEL, CheckStatus::Skipped, None));
    Report {
        checks,
        problem: None,
    }
}

/// Records the tunnel's outcome on the forwarding step: Docker answered through it, or every
/// connection was refused although the preflight passed, so sshd refuses the forward (e.g.
/// through a `Match` block or `PermitOpen`). Returns the problem in the second case.
pub fn tunnel_result(
    server: &Server,
    checks: &mut [PreflightCheck],
    answered: bool,
) -> Option<ServerProblem> {
    let step = checks.iter_mut().find(|c| c.label == FORWARD_LABEL)?;
    if answered {
        step.status = CheckStatus::Pass;
        return None;
    }
    step.status = CheckStatus::Fail;
    step.detail = Some("connections through the tunnel are refused".into());
    Some(problem(
        format!(
            "sshd on {} refuses to forward the docker socket",
            host(server)
        ),
        Some(FORWARDING_FIX.into()),
    ))
}

/// Explains an SSH login that failed before the preflight could run, from ssh's stderr.
pub fn login_failed(
    server: &Server,
    key_secret: Option<&str>,
    known_hosts: &str,
    stderr: &str,
) -> Report {
    let host = host(server);
    let user = server.user.as_deref().unwrap_or("root");
    let port = server.port.unwrap_or(22);
    let stderr = stderr.trim();
    let (summary, fix) = if stderr.contains("Permission denied") {
        let key = match key_secret {
            Some(name) => format!("the public key of secret {name}"),
            None => format!("the daemon's public key ({} server key)", brand::CLI_BIN),
        };
        (
            format!("{host} rejected the SSH key for {user}"),
            Some(format!(
                "add {key} to ~{user}/.ssh/authorized_keys on {host}, then reconnect"
            )),
        )
    } else if stderr.contains("Host key verification failed")
        || stderr.contains("IDENTIFICATION HAS CHANGED")
    {
        let entry = if port == 22 {
            host.to_string()
        } else {
            format!("[{host}]:{port}")
        };
        (
            format!("the host key of {host} changed since the daemon first connected"),
            Some(format!(
                "if you reinstalled the server, forget the old key: ssh-keygen -R '{entry}' -f {known_hosts}"
            )),
        )
    } else if stderr.contains("Connection refused") {
        (
            format!("nothing accepts SSH on {host}:{port}"),
            Some("check the host and port, and that sshd is running".into()),
        )
    } else if stderr.contains("timed out") || stderr.contains("No route to host") {
        (
            format!("{host}:{port} doesn't answer"),
            Some(format!(
                "check the address, and that the firewall allows port {port}"
            )),
        )
    } else if stderr.contains("Could not resolve hostname") {
        (
            format!("cannot resolve {host}"),
            Some("check the host name".into()),
        )
    } else {
        (format!("ssh to {user}@{host}:{port} failed"), None)
    };
    let detail = match stderr {
        "" => summary.clone(),
        err => format!("{summary}: {err}"),
    };
    let step = check(
        ssh_label(server),
        CheckStatus::Fail,
        Some(stderr.to_string()).filter(|s| !s.is_empty()),
    );
    let p = ServerProblem {
        summary,
        fix,
        detail,
        can_install_docker: false,
    };
    failed(Vec::new(), user, step, p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use CheckStatus::{Fail, Pass, Skipped};
    use chrono::Utc;
    use common::{ProxyStatus, ServerKind, ServerStatus};

    fn server(user: &str) -> Server {
        Server {
            id: "s1".into(),
            name: "edge".into(),
            kind: ServerKind::Ssh,
            host: Some("1.2.3.4".into()),
            user: Some(user.into()),
            port: Some(22),
            ssh_key_secret_id: None,
            status: ServerStatus::Unknown,
            proxy: ProxyStatus::Missing,
            docker_version: None,
            last_checked_at: None,
            created_at: Utc::now(),
            checks: Vec::new(),
            problem: None,
        }
    }

    const HEALTHY: &str = "user=root\nuid=0\ndocker_cli=1\nsocket=1\n\
        socket_mode=srw-rw---- root docker\nsocket_access=1\ndocker_version=29.1.0\n\
        sshd_allowtcpforwarding=yes\nsshd_allowstreamlocalforwarding=yes\n\
        sshd_disableforwarding=no\nsystemd=1\n";

    fn statuses(r: &Report) -> Vec<CheckStatus> {
        r.checks.iter().map(|c| c.status).collect()
    }

    #[test]
    fn a_healthy_server_passes_every_step_before_the_tunnel() {
        let mut r = diagnose(&server("root"), &Facts::parse(HEALTHY));
        assert_eq!(r.problem, None);
        assert_eq!(statuses(&r), [Pass, Pass, Pass, Pass, Skipped]);
        assert_eq!(r.checks[1].detail.as_deref(), Some("docker 29.1.0"));
        assert_eq!(tunnel_result(&server("root"), &mut r.checks, true), None);
        assert_eq!(r.checks[4].status, Pass);
    }

    #[test]
    fn missing_docker_offers_the_install_to_root() {
        let r = diagnose(
            &server("root"),
            &Facts::parse("user=root\nuid=0\nsystemd=1\n"),
        );
        let p = r.problem.as_ref().unwrap();
        assert_eq!(p.summary, "docker isn't installed on 1.2.3.4");
        assert_eq!(
            p.fix.as_deref(),
            Some("install it with: curl -fsSL https://get.docker.com | sh")
        );
        assert!(p.can_install_docker);
        assert_eq!(statuses(&r), [Pass, Fail, Skipped, Skipped, Skipped]);
    }

    #[test]
    fn missing_docker_offers_the_install_only_with_passwordless_sudo() {
        let offered = |facts: &str| {
            diagnose(&server("deploy"), &Facts::parse(facts))
                .problem
                .unwrap()
                .can_install_docker
        };
        assert!(!offered("user=deploy\nuid=1000\n"));
        assert!(offered("user=deploy\nuid=1000\nsudo=1\n"));
    }

    #[test]
    fn a_stopped_engine_says_how_to_start_it() {
        let facts = "user=root\nuid=0\ndocker_cli=1\ndocker_error=Cannot connect\nsystemd=1\n";
        let r = diagnose(&server("root"), &Facts::parse(facts));
        let p = r.problem.as_ref().unwrap();
        assert_eq!(p.summary, "docker is installed on 1.2.3.4 but not running");
        assert_eq!(
            p.fix.as_deref(),
            Some("start it with: sudo systemctl enable --now docker")
        );
        assert_eq!(statuses(&r), [Pass, Pass, Fail, Skipped, Skipped]);
    }

    #[test]
    fn a_user_outside_the_socket_group_is_told_to_join_it() {
        let facts = "user=deploy\nuid=1000\ndocker_cli=1\nsocket=1\n\
            socket_mode=srw-rw---- root docker\n\
            docker_error=permission denied while trying to connect\n";
        let r = diagnose(&server("deploy"), &Facts::parse(facts));
        let p = r.problem.as_ref().unwrap();
        assert_eq!(
            p.summary,
            "user deploy can't access /var/run/docker.sock on 1.2.3.4"
        );
        assert_eq!(
            p.fix.as_deref(),
            Some("run: sudo usermod -aG docker deploy, then reconnect")
        );
        assert_eq!(statuses(&r), [Pass, Pass, Pass, Fail, Skipped]);
        assert_eq!(
            r.checks[3].detail.as_deref(),
            Some("srw-rw---- root docker")
        );
    }

    #[test]
    fn disabled_forwarding_names_the_sshd_option() {
        let summary = |facts: String| {
            diagnose(&server("root"), &Facts::parse(&facts))
                .problem
                .unwrap()
                .summary
        };
        assert_eq!(
            summary(HEALTHY.replace("allowtcpforwarding=yes", "allowtcpforwarding=no")),
            "sshd on 1.2.3.4 has AllowTcpForwarding no"
        );
        assert_eq!(
            summary(HEALTHY.replace(
                "allowstreamlocalforwarding=yes",
                "allowstreamlocalforwarding=remote"
            )),
            "sshd on 1.2.3.4 has AllowStreamLocalForwarding no"
        );
        assert_eq!(
            summary(HEALTHY.replace("disableforwarding=no", "disableforwarding=yes")),
            "sshd on 1.2.3.4 has DisableForwarding yes"
        );
    }

    #[test]
    fn a_refused_tunnel_blames_sshd() {
        let mut r = diagnose(
            &server("deploy"),
            &Facts::parse(&HEALTHY.replace("uid=0", "uid=1000")),
        );
        let p = tunnel_result(&server("deploy"), &mut r.checks, false).unwrap();
        assert_eq!(
            p.summary,
            "sshd on 1.2.3.4 refuses to forward the docker socket"
        );
        assert_eq!(r.checks[4].status, Fail);
    }

    #[test]
    fn rootless_docker_is_named() {
        let facts =
            "user=deploy\nuid=1000\ndocker_cli=1\nrootless_socket=/run/user/1000/docker.sock\n";
        let p = diagnose(&server("deploy"), &Facts::parse(facts))
            .problem
            .unwrap();
        assert!(p.summary.contains("runs rootless"), "{}", p.summary);
    }

    #[test]
    fn login_failures_are_explained() {
        let s = server("deploy");
        let r = login_failed(
            &s,
            None,
            "/k",
            "deploy@1.2.3.4: Permission denied (publickey).",
        );
        let p = r.problem.as_ref().unwrap();
        assert_eq!(p.summary, "1.2.3.4 rejected the SSH key for deploy");
        assert!(p.fix.as_deref().unwrap().contains("server key"));
        assert_eq!(statuses(&r), [Fail, Skipped, Skipped, Skipped, Skipped]);
        let summary = |stderr: &str| login_failed(&s, None, "/k", stderr).problem.unwrap();
        assert_eq!(
            summary("ssh: connect to host 1.2.3.4 port 22: Connection refused").summary,
            "nothing accepts SSH on 1.2.3.4:22"
        );
        assert_eq!(
            summary("Host key verification failed.").fix.as_deref(),
            Some(
                "if you reinstalled the server, forget the old key: ssh-keygen -R '1.2.3.4' -f /k"
            )
        );
    }
}
