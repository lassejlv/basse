//! Where a new workload runs: the server field of a create form and the rows of its picker.
//! Every form that creates a service or database uses these, so the choice looks and behaves
//! the same everywhere.

use crate::form::Field;
use crate::picker::PickItem;
use crate::theme;
use common::{Server, ServerKind, ServerStatus};

/// The form field key.
pub const KEY: &str = "server";

/// The server a new workload lands on unless another is picked: the local server, else the
/// first one that is not offline.
pub fn default(servers: &[Server]) -> Option<&Server> {
    servers
        .iter()
        .find(|s| s.kind == ServerKind::Local)
        .or_else(|| servers.iter().find(|s| s.status != ServerStatus::Offline))
        .or(servers.first())
}

/// "local · 5.75.12.4" or "ssh · root@203.0.113.7"; `address` is where the server is reached.
pub fn summary(server: &Server, address: &str) -> String {
    match (server.kind, &server.user) {
        (ServerKind::Local, _) => format!("local · {address}"),
        (ServerKind::Ssh, Some(user)) => format!("ssh · {user}@{address}"),
        (ServerKind::Ssh, None) => format!("ssh · {address}"),
    }
}

/// The field's hint: green when the server answered its last check.
fn status_hint(server: &Server) -> &'static str {
    match server.status {
        ServerStatus::Online => "✓ connected",
        ServerStatus::Offline => "offline",
        ServerStatus::Unknown => "not checked yet",
    }
}

/// Fill a server field with `server`.
pub fn set(field: &mut Field, server: &Server, address: &str) {
    field.value = server.id.clone();
    field.shown = server.name.clone();
    field.placeholder = summary(server, address);
    field.hint = status_hint(server).into();
}

/// The pick field set to the default server. It is hidden when there is nothing to choose
/// from; the form shows [`note`] instead.
pub fn field(servers: &[Server], address: impl Fn(&Server) -> String) -> Field {
    let mut field = Field::pick(KEY, "server", "", "");
    if let Some(server) = default(servers) {
        set(&mut field, server, &address(server));
    }
    field.visible = servers.len() > 1;
    field
}

/// The muted line a form shows in place of a hidden server field: "on prod-eu-1".
pub fn note(servers: &[Server]) -> Option<String> {
    match servers {
        [one] => Some(format!("on {}", one.name)),
        _ => None,
    }
}

/// Picker rows: usable servers in their usual order, then offline ones, which can't be picked.
pub fn items(servers: &[Server], address: impl Fn(&Server) -> String) -> Vec<PickItem> {
    let mut sorted: Vec<&Server> = servers.iter().collect();
    sorted.sort_by_key(|s| s.status == ServerStatus::Offline);
    let default_id = default(servers).map(|s| s.id.clone());
    sorted
        .into_iter()
        .map(|s| {
            let mut item = PickItem::new(Some(s.id.clone()), &s.name, summary(s, &address(s)));
            if default_id.as_ref() == Some(&s.id) && servers.len() > 1 {
                item.badge = "default".into();
            }
            let docker = s
                .docker_version
                .as_deref()
                .map(|v| format!(" · docker {v}"))
                .unwrap_or_default();
            let (glyph, style, note, disabled) = match s.status {
                ServerStatus::Offline => (
                    "○",
                    theme::muted(),
                    match s.last_checked_at {
                        Some(at) => format!("offline · checked {}", crate::util::ago(at)),
                        None => "offline".into(),
                    },
                    true,
                ),
                ServerStatus::Online => (
                    "●",
                    theme::fg(theme::OK),
                    format!("connected{docker}"),
                    false,
                ),
                ServerStatus::Unknown => ("◌", theme::muted(), "not checked yet".into(), false),
            };
            item.glyph = Some((glyph, style));
            item.note = note;
            item.disabled = disabled;
            item
        })
        .collect()
}

/// What a compose service on a remote server should know; every build method deploys there,
/// but compose resolves bind mounts on the daemon's machine.
pub const REMOTE_COMPOSE: &str =
    "On an ssh server, compose bind mounts point at paths on this machine; use named volumes.";

/// The line under the picker's list.
pub fn picker_note(servers: &[Server]) -> String {
    let offline: Vec<&str> = servers
        .iter()
        .filter(|s| s.status == ServerStatus::Offline)
        .map(|s| s.name.as_str())
        .collect();
    match offline.as_slice() {
        [] => "A service stays on the server it is created on.".into(),
        [one] => format!(
            "{one} is offline, so it can't take new services. Check it again from the Servers screen."
        ),
        many => format!(
            "{} servers are offline, so they can't take new services. Check them from the Servers screen.",
            many.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, kind: ServerKind, status: ServerStatus) -> Server {
        Server {
            id: name.into(),
            name: name.into(),
            kind,
            host: (kind == ServerKind::Ssh).then(|| format!("{name}.example.test")),
            user: (kind == ServerKind::Ssh).then(|| "root".into()),
            port: None,
            ssh_key_secret_id: None,
            status,
            proxy: Default::default(),
            docker_version: Some("27.3".into()),
            last_checked_at: None,
            created_at: Default::default(),
        }
    }

    fn address(s: &Server) -> String {
        s.host.clone().unwrap_or_else(|| "203.0.113.1".into())
    }

    #[test]
    fn defaults_to_the_local_server_and_hides_with_only_one() {
        let only = [server("solo", ServerKind::Local, ServerStatus::Online)];
        let f = field(&only, address);
        assert!(!f.visible);
        assert_eq!(f.value, "solo");
        assert_eq!(note(&only).as_deref(), Some("on solo"));

        let many = [
            server("edge", ServerKind::Ssh, ServerStatus::Online),
            server("home", ServerKind::Local, ServerStatus::Online),
        ];
        let f = field(&many, address);
        assert!(f.visible);
        assert_eq!((f.value.as_str(), f.shown.as_str()), ("home", "home"));
        assert_eq!(f.placeholder, "local · 203.0.113.1");
        assert_eq!(f.hint, "✓ connected");
        assert_eq!(note(&many), None);
    }

    #[test]
    fn offline_servers_come_last_and_are_disabled() {
        let servers = [
            server("down", ServerKind::Ssh, ServerStatus::Offline),
            server("main", ServerKind::Local, ServerStatus::Online),
            server("edge", ServerKind::Ssh, ServerStatus::Online),
        ];
        let rows = items(&servers, address);
        let names: Vec<_> = rows.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["main", "edge", "down"]);
        assert_eq!(
            rows.iter().map(|i| i.disabled).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(rows[0].badge, "default");
        assert_eq!(rows[1].detail, "ssh · root@edge.example.test");
        assert_eq!(rows[1].note, "connected · docker 27.3");
        assert_eq!(rows[2].note, "offline");
        assert!(picker_note(&servers).starts_with("down is offline"));
    }
}
