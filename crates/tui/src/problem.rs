//! Dialogs that explain failures in full: a server's preflight checklist with its problem and
//! fix, and the whole text of an error the key bar had to cut short.

use crate::app::{App, ConfirmAction, Msg, Overlay};
use crate::mouse;
use crate::theme;
use crate::util;
use crate::view::{action_chip, dialog, rule_line};
use chrono::{DateTime, Local};
use common::{CheckStatus, Server, ServerKind, ServerStatus};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use std::time::Instant;

const WIDTH: u16 = 96;

/// A server's last check: the preflight checklist, and the problem with its fix.
pub struct ServerDetail {
    pub server: Server,
    /// Set while a check runs.
    pub busy: Option<Instant>,
}

/// The full text of an error from the key bar.
pub struct ErrorDetail {
    pub text: String,
    pub at: DateTime<Local>,
}

/// The command in a fix such as "run: sudo usermod -aG docker deploy, then reconnect", or the
/// whole fix when it names none.
pub fn fix_command(fix: &str) -> &str {
    match fix.split_once(": ") {
        Some((_, command)) => command.strip_suffix(", then reconnect").unwrap_or(command),
        None => fix,
    }
}

impl App {
    pub fn open_server_detail(&mut self, server: Server) {
        self.overlay = Some(Overlay::ServerDetail(ServerDetail { server, busy: None }));
    }

    /// Checks a server again. The result updates an open detail dialog, or shows as a toast.
    pub fn check_server(&mut self, server: &Server) {
        match &mut self.overlay {
            Some(Overlay::ServerDetail(d)) if d.server.id == server.id => {
                d.busy = Some(Instant::now())
            }
            _ => self.progress(format!("checking {}…", server.name)),
        }
        let id = server.id.clone();
        self.spawn(|c| async move {
            Msg::ServerChecked(c.check_server(&id).await.map_err(|e| format!("{e:#}")))
        });
    }

    pub fn install_docker(&mut self, id: String) {
        self.spawn(|c| async move {
            Msg::DockerInstalled(c.install_docker(&id).await.map_err(|e| format!("{e:#}")))
        });
    }

    /// A check finished: refresh the dialog showing that server, or report it in the key bar.
    pub fn server_checked(&mut self, server: Server) {
        if let Some(slot) = self.servers.iter_mut().find(|s| s.id == server.id) {
            *slot = server.clone();
        }
        if let Some(Overlay::ServerDetail(d)) = &mut self.overlay
            && d.server.id == server.id
        {
            d.busy = None;
            d.server = server;
            return;
        }
        match (&server.status, &server.problem) {
            (ServerStatus::Online, _) => self.toast(format!(
                "{} online · docker {}",
                server.name,
                server.docker_version.as_deref().unwrap_or("?")
            )),
            (_, Some(p)) => self.error(format!("{}: {}", server.name, p.summary)),
            _ => self.error(format!("{} is offline", server.name)),
        }
    }

    /// Opens the full text of the error in the key bar, if it had to be cut short.
    pub fn expand_error(&mut self) -> bool {
        let Some(t) = self.toast.as_ref().filter(|t| t.error && t.truncated.get()) else {
            return false;
        };
        let elapsed = chrono::Duration::from_std(t.at.elapsed()).unwrap_or_default();
        self.overlay = Some(Overlay::ErrorDetail(ErrorDetail {
            text: t.text.clone(),
            at: Local::now() - elapsed,
        }));
        self.toast = None;
        true
    }
}

pub fn server_detail_keys(app: &mut App, key: KeyEvent) {
    let Some(Overlay::ServerDetail(d)) = &app.overlay else {
        return;
    };
    let server = d.server.clone();
    let busy = d.busy.is_some();
    let problem = server.problem.clone();
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => app.overlay = None,
        KeyCode::Char('c') if !busy => app.check_server(&server),
        KeyCode::Char('y') => {
            if let Some(fix) = problem.as_ref().and_then(|p| p.fix.as_deref()) {
                app.copy(fix_command(fix), "the fix");
            }
        }
        KeyCode::Char('i') if !busy && problem.is_some_and(|p| p.can_install_docker) => {
            let user = server.user.as_deref().unwrap_or("root");
            let host = server.host.as_deref().unwrap_or("the server");
            let how = if user == "root" {
                "as root".to_string()
            } else {
                format!("as {user} with sudo")
            };
            app.overlay = Some(Overlay::Confirm(crate::app::Confirm {
                title: format!("Install Docker · {}", server.name),
                message: format!(
                    "Runs get.docker.com {how} on {host} over SSH, starts Docker and checks {} again. Takes a minute or two.\n\nsame as: curl -fsSL https://get.docker.com | sh",
                    server.name
                ),
                danger: false,
                action: ConfirmAction::InstallDocker(server.id),
                verb: "install docker",
                busy: None,
                error: Vec::new(),
            }));
        }
        _ => {}
    }
}

pub fn error_detail_keys(app: &mut App, key: KeyEvent) {
    let Some(Overlay::ErrorDetail(e)) = &app.overlay else {
        return;
    };
    match key.code {
        KeyCode::Char('y') => {
            let text = e.text.clone();
            app.copy(&text, "the error");
        }
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => app.overlay = None,
        _ => {}
    }
}

/// One checklist row; details line up at `column`, counted from the label's start.
fn check_line(
    glyph: &'static str,
    glyph_style: ratatui::style::Style,
    label: &str,
    label_style: ratatui::style::Style,
    detail: Option<&str>,
    column: usize,
) -> Line<'static> {
    let pad = column.saturating_sub(label.chars().count());
    let mut spans = vec![
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled(label.to_string(), label_style),
    ];
    if let Some(detail) = detail {
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(detail.to_string(), theme::muted()));
    }
    Line::from(spans)
}

pub fn render_server_detail(frame: &mut Frame, area: Rect, d: &ServerDetail, tick: usize) {
    let s = &d.server;
    let width = WIDTH.min(area.width.saturating_sub(4));
    let text_w = width.saturating_sub(6);
    let column = s
        .checks
        .iter()
        .map(|c| c.label.chars().count())
        .max()
        .unwrap_or(0)
        + 4;
    let mut checks: Vec<Line> = s
        .checks
        .iter()
        .map(|c| {
            let detail = c.detail.as_deref();
            match c.status {
                CheckStatus::Pass => check_line(
                    "✓",
                    theme::fg(theme::OK),
                    &c.label,
                    theme::text(),
                    detail,
                    column,
                ),
                CheckStatus::Fail => check_line(
                    "✕",
                    theme::fg(theme::ERR),
                    &c.label,
                    theme::text(),
                    detail,
                    column,
                ),
                CheckStatus::Skipped => {
                    check_line("·", theme::muted(), &c.label, theme::muted(), None, column)
                }
            }
        })
        .collect();
    if checks.is_empty() && s.kind == ServerKind::Local {
        let docker = s.docker_version.as_deref();
        checks.push(check_line(
            "✓",
            theme::fg(theme::OK),
            "this machine's docker",
            theme::text(),
            docker,
            25,
        ));
    }
    let mut problem: Vec<Line> = Vec::new();
    match &s.problem {
        Some(p) => {
            problem.push(Line::from(vec![
                Span::styled("✕ ", theme::fg(theme::ERR)),
                Span::styled(p.summary.clone(), theme::text()),
            ]));
            if let Some(fix) = &p.fix {
                problem.push(Line::from(vec![
                    Span::styled("  fix  ", theme::muted()),
                    Span::styled(fix.clone(), theme::text()),
                ]));
            }
            if p.detail != p.summary {
                problem.push(Line::default());
                problem.push(Line::styled(p.detail.clone(), theme::muted()));
            }
        }
        None if s.status == ServerStatus::Online => problem.push(Line::styled(
            format!(
                "● online · docker {}",
                s.docker_version.as_deref().unwrap_or("?")
            ),
            theme::fg(theme::OK),
        )),
        None => problem.push(Line::styled(
            "not checked yet · c checks it",
            theme::muted(),
        )),
    }
    let problem = Paragraph::new(problem).wrap(Wrap { trim: false });
    let note = match d.busy {
        Some(_) => Line::from(vec![
            Span::styled(format!("{} ", util::spinner(tick)), theme::fg(theme::INFO)),
            Span::styled("checking…", theme::muted()),
        ]),
        None => Line::styled(
            format!(
                "checked {}",
                s.last_checked_at.map_or("never".into(), util::ago)
            ),
            theme::muted(),
        ),
    };
    let (checks_h, problem_h) = (checks.len() as u16, problem.line_count(text_w) as u16);
    let gap = u16::from(checks_h > 0);
    let target = match (s.kind, &s.host) {
        (ServerKind::Ssh, Some(host)) => format!(
            " · {} · {}@{host}:{}",
            s.name,
            s.user.as_deref().unwrap_or("root"),
            s.port.unwrap_or(22)
        ),
        _ => format!(" · {}", s.name),
    };
    let inner = dialog(
        frame,
        area,
        width,
        checks_h + gap + problem_h + 8,
        vec![
            Span::styled("Server", theme::text()),
            Span::styled(target, theme::muted()),
        ],
    );
    let [checks_a, _, problem_a, _, note_a, rule, keys] = Layout::vertical([
        Constraint::Length(checks_h),
        Constraint::Length(gap),
        Constraint::Length(problem_h),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(checks), checks_a);
    frame.render_widget(problem, problem_a);
    frame.render_widget(Paragraph::new(note), note_a);
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    let p = s.problem.as_ref();
    let keys = if p.is_some_and(|p| p.can_install_docker) && d.busy.is_none() {
        action_chip(frame, keys, "i", "install docker", theme::chip())
    } else {
        keys
    };
    let mut hints = vec![("c", "check again")];
    if p.is_some_and(|p| p.fix.is_some()) {
        hints.push(("y", "copy fix"));
    }
    hints.push(("esc", "close"));
    mouse::hints(frame, keys, &hints, Alignment::Left);
}

pub fn render_error_detail(frame: &mut Frame, area: Rect, e: &ErrorDetail) {
    let width = WIDTH.min(area.width.saturating_sub(4));
    let text_w = width.saturating_sub(6);
    let (head, rest) = e
        .text
        .split_once(" · ")
        .or_else(|| e.text.split_once(": "))
        .unwrap_or((&e.text, ""));
    let head = Paragraph::new(Line::from(vec![
        Span::styled("✕ ", theme::fg(theme::ERR)),
        Span::styled(head.to_string(), theme::text()),
    ]))
    .wrap(Wrap { trim: false });
    let has_rest = !rest.is_empty();
    let rest = Paragraph::new(rest.to_string())
        .style(theme::text())
        .wrap(Wrap { trim: true });
    let head_h = head.line_count(text_w) as u16;
    let rest_h = if has_rest {
        rest.line_count(text_w.saturating_sub(2)) as u16
    } else {
        0
    };
    let inner = dialog(
        frame,
        area,
        width,
        head_h + rest_h + u16::from(rest_h > 0) + 8,
        vec![
            Span::styled("Error", theme::text()),
            Span::styled(" · full message", theme::muted()),
        ],
    );
    let [head_a, _, rest_a, _, time, rule, keys] = Layout::vertical([
        Constraint::Length(head_h),
        Constraint::Length(u16::from(rest_h > 0)),
        Constraint::Length(rest_h),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(head, head_a);
    frame.render_widget(
        rest,
        Rect {
            x: rest_a.x + 2,
            width: rest_a.width.saturating_sub(2),
            ..rest_a
        },
    );
    frame.render_widget(
        Paragraph::new(e.at.format("%H:%M:%S").to_string()).style(theme::muted()),
        time,
    );
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    mouse::hints(
        frame,
        keys,
        &[("y", "copy"), ("esc", "close")],
        Alignment::Left,
    );
}

#[cfg(test)]
mod tests {
    use super::fix_command;

    #[test]
    fn the_fix_command_is_what_follows_the_colon() {
        assert_eq!(
            fix_command("run: sudo usermod -aG docker deploy, then reconnect"),
            "sudo usermod -aG docker deploy"
        );
        assert_eq!(
            fix_command("install it with: curl -fsSL https://get.docker.com | sh"),
            "curl -fsSL https://get.docker.com | sh"
        );
        assert_eq!(fix_command("check the host name"), "check the host name");
    }
}
