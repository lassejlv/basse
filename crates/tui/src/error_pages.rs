//! The error pages dialog: whether a service's errors get branded pages, and the custom page
//! for each error, per service or as the instance default. Setting a page opens a form; the
//! dialog comes back when it is saved.

use crate::app::{App, Msg, Overlay, Then};
use crate::form::{Field, Form, FormKind};
use crate::mouse::{self, Hit};
use crate::theme;
use crate::view::{action_chip, dialog, rule_line};
use common::api::{SetErrorPage, UpdateService};
use common::*;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

const WIDTH: u16 = 72;
/// Where the source column starts, counted from the row's start.
const SOURCE_COL: usize = 26;

/// The open dialog. Its pages are read from the app, so they stay current.
pub struct ErrorPagesDialog {
    /// `None` for the instance defaults.
    pub service_id: Option<Id>,
    pub row: usize,
}

const FROM: [&str; 2] = ["file on this computer", "file in the repo"];

impl App {
    /// Opens the dialog with `kind`'s row selected.
    pub fn open_error_pages(&mut self, service_id: Option<Id>, kind: ErrorPageKind) {
        if let Some(id) = &service_id {
            self.load_detail(id);
        }
        self.load_instance_error_pages();
        let row = ErrorPageKind::ALL
            .iter()
            .position(|k| *k == kind)
            .unwrap_or(0);
        self.overlay = Some(Overlay::ErrorPages(ErrorPagesDialog { service_id, row }));
    }

    pub(crate) fn load_instance_error_pages(&self) {
        self.spawn(|c| async move {
            match c.instance_error_pages().await {
                Ok(p) => Msg::InstanceErrorPages(p),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    /// A service's pages, or the instance defaults.
    pub fn error_pages(&self, service_id: Option<&str>) -> Option<&ErrorPages> {
        match service_id {
            Some(id) => self.details.get(id).map(|d| &d.error_pages),
            None => self.settings.error_pages.as_ref(),
        }
    }

    fn builds_from_git(&self, service_id: Option<&str>) -> bool {
        service_id
            .and_then(|id| self.details.get(id))
            .is_some_and(|d| matches!(d.service.source, Source::Git { .. }))
    }

    fn form_error_page(&mut self, service_id: Option<Id>, kind: ErrorPageKind) {
        let current = self
            .error_pages(service_id.as_deref())
            .and_then(|p| p.page(kind))
            .map(|p| p.source.clone());
        let mut fields = Vec::new();
        let repo = matches!(current, Some(ErrorPageSource::Repo { .. }));
        if self.builds_from_git(service_id.as_deref()) {
            fields.push(
                Field::choice("from", "from", FROM.map(String::from).to_vec())
                    .selected(usize::from(repo)),
            );
        }
        let path = match &current {
            Some(ErrorPageSource::Repo { path }) => path.clone(),
            _ => String::new(),
        };
        fields.push(
            Field::text("path", "path")
                .value(path)
                .placeholder("./down.html")
                .hint(format!("HTML, ≤ {} KB", MAX_ERROR_PAGE_BYTES / 1024)),
        );
        let mut form = Form::new(
            FormKind::ErrorPage { service_id, kind },
            format!("Set {}", kind.code()),
            fields,
        )
        .subtitle(kind.label())
        .focus_on("path");
        if repo {
            form = form.note(vec![
                "repo pages are read from the root dir on every deploy".into(),
            ]);
        }
        self.overlay = Some(Overlay::Form(form));
    }

    /// Saves the page in the form. An uploaded page that loads external resources shows a
    /// warning first; submitting again saves it anyway.
    pub fn submit_error_page(&mut self) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        let FormKind::ErrorPage { service_id, kind } = form.kind.clone() else {
            return;
        };
        let path = form.get("path");
        let body = if form.choice_label("from").as_deref() == Some(FROM[1]) {
            if path.is_empty() {
                return form.fail("type the page's path in the repository, like errors/503.html");
            }
            SetErrorPage::Repo { path }
        } else {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            let html = match crate::app::read_local_file(&path, home.as_deref()) {
                Ok(html) => html,
                Err(e) => return form.fail(e),
            };
            let warnings = match check_error_page(&html) {
                Ok(w) => w,
                Err(e) => return form.fail(e),
            };
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            let shown: Vec<String> = warnings
                .iter()
                .flat_map(|w| w.split("; "))
                .enumerate()
                .map(|(i, w)| match i {
                    0 => format!("{} {w}", name.as_deref().unwrap_or("the page")),
                    _ => w.to_string(),
                })
                .collect();
            if !shown.is_empty() && form.warning != shown {
                form.warning = shown;
                form.submit_label = "save anyway";
                form.error = None;
                return;
            }
            SetErrorPage::Upload { html, name }
        };
        form.busy = true;
        form.error = None;
        self.spawn(move |c| async move {
            match c.set_error_page(service_id.as_deref(), kind, &body).await {
                Ok(page) => Msg::FormDone {
                    toast: match (&page.source, page.size) {
                        (ErrorPageSource::Repo { path }, _) => {
                            format!("{kind} page reads {path} on every deploy · deploy to apply")
                        }
                        (_, size) => format!("{kind} page set · {}", kb(size.unwrap_or_default())),
                    },
                    then: Then::ErrorPages(service_id, kind),
                },
                Err(e) => Msg::FormError(format!("{e:#}")),
            }
        });
    }

    fn reset_error_page(&mut self, service_id: Option<Id>, kind: ErrorPageKind) {
        self.spawn(move |c| async move {
            let done = c
                .delete_error_page(service_id.as_deref(), kind)
                .await
                .map(|()| format!("{kind} page reset to the default"));
            Msg::ErrorPagesChanged(service_id, done.map_err(|e| format!("{e:#}")))
        });
    }

    fn toggle_branded(&mut self, service_id: Id) {
        let on = !self
            .error_pages(Some(&service_id))
            .is_none_or(|p| p.branded);
        self.spawn(move |c| async move {
            let body = UpdateService {
                branded_error_pages: Some(on),
                ..Default::default()
            };
            let done = c.update_service(&service_id, &body).await.map(|_| {
                if on {
                    "branded error pages on".to_string()
                } else {
                    "branded error pages off · the proxy's own responses pass through".to_string()
                }
            });
            Msg::ErrorPagesChanged(Some(service_id), done.map_err(|e| format!("{e:#}")))
        });
    }

    /// A reset or toggle finished: reload what changed and report it.
    pub fn error_pages_changed(&mut self, service_id: Option<Id>, done: Result<String, String>) {
        match done {
            Ok(toast) => self.toast(toast),
            Err(e) => self.error(e),
        }
        match service_id {
            Some(id) => self.load_detail(&id),
            None => self.load_instance_error_pages(),
        }
    }
}

fn kb(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.0)
    }
}

pub fn keys(app: &mut App, key: KeyEvent) {
    let Some(Overlay::ErrorPages(d)) = &mut app.overlay else {
        return;
    };
    let service_id = d.service_id.clone();
    let kind = ErrorPageKind::ALL[d.row.min(ErrorPageKind::ALL.len() - 1)];
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.overlay = None,
        KeyCode::Up | KeyCode::Char('k') => d.row = d.row.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => d.row = (d.row + 1).min(ErrorPageKind::ALL.len() - 1),
        KeyCode::Enter => app.form_error_page(service_id, kind),
        KeyCode::Char('x') | KeyCode::Delete => {
            let custom = app
                .error_pages(service_id.as_deref())
                .is_some_and(|p| p.page(kind).is_some());
            if custom {
                app.reset_error_page(service_id, kind);
            } else {
                app.toast(format!("the {kind} page is already the default"));
            }
        }
        KeyCode::Char('b') => {
            if let Some(id) = service_id {
                app.toggle_branded(id);
            }
        }
        _ => {}
    }
}

/// What a row's page comes from: the service's own, the instance default, or built in.
fn source(own: Option<&CustomErrorPage>, default: Option<&CustomErrorPage>) -> Vec<Span<'static>> {
    let size = |p: &CustomErrorPage| p.size.map(|s| format!(" · {}", kb(s)));
    match (own, default) {
        (Some(p), _) => match &p.source {
            ErrorPageSource::Upload { .. } => vec![
                Span::styled("uploaded", theme::text()),
                Span::styled(size(p).unwrap_or_default(), theme::muted()),
            ],
            ErrorPageSource::Repo { path } => vec![
                Span::styled("repo", theme::text()),
                Span::styled(
                    format!(
                        " · {path}{}",
                        size(p).unwrap_or_else(|| " · not read yet".into())
                    ),
                    theme::muted(),
                ),
            ],
        },
        (None, Some(p)) if p.size.is_some() => vec![
            Span::styled("instance default", theme::text()),
            Span::styled(size(p).unwrap_or_default(), theme::muted()),
        ],
        _ => vec![Span::styled("built-in", theme::muted())],
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App, d: &ErrorPagesDialog) {
    let service = d.service_id.as_deref();
    let pages = app.error_pages(service);
    let instance = app.settings.error_pages.as_ref();
    let branded = pages.is_none_or(|p| p.branded);
    let note = match (service, branded) {
        (None, _) => "for services without their own page, and for unknown hosts",
        (Some(_), false) => "off · visitors get the proxy's empty 502, 503 or 504",
        (Some(_), true) if app.builds_from_git(service) => {
            "repo pages are read from the root dir on every deploy"
        }
        (Some(_), true) => "pages are uploaded from this computer, up to 256 KB each",
    };
    let head = if service.is_some() { 2 } else { 0 };
    let name = match service {
        Some(id) => app
            .details
            .get(id)
            .map_or_else(String::new, |d| d.service.name.clone()),
        None => "instance default".into(),
    };
    let inner = dialog(
        frame,
        area,
        WIDTH,
        head + ErrorPageKind::ALL.len() as u16 + 9,
        vec![
            Span::styled("Error pages", theme::text()),
            Span::styled(format!(" · {name}"), theme::muted()),
        ],
    );
    let [head_a, rows_a, _, note_a, _, rule, keys] = Layout::vertical([
        Constraint::Length(head),
        Constraint::Length(ErrorPageKind::ALL.len() as u16),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    if service.is_some() {
        let line = Rect {
            height: 1,
            ..head_a
        };
        let value = if branded {
            Span::styled("● on", theme::fg(theme::OK))
        } else {
            Span::styled("○ off", theme::muted())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{:<SOURCE_COL$}", "branded pages"), theme::muted()),
                value,
            ])),
            line,
        );
        mouse::hints(frame, line, &[("b", "toggle")], Alignment::Right);
    }
    for (i, kind) in ErrorPageKind::ALL.into_iter().enumerate() {
        let row = Rect::new(rows_a.x, rows_a.y + i as u16, rows_a.width, 1);
        let selected = i == d.row;
        if selected {
            frame.render_widget(Block::new().style(Style::new().bg(theme::LINE)), row);
        }
        let dim = |s: Style| if branded { s } else { theme::muted() };
        let mut spans = vec![
            Span::styled(format!(" {:<6}", kind.code()), dim(theme::text())),
            Span::styled(
                format!("{:<w$}", kind.label(), w = SOURCE_COL - 7),
                dim(if selected {
                    theme::accent()
                } else {
                    theme::text()
                }),
            ),
        ];
        let own = pages.and_then(|p| p.page(kind));
        let default = service.and(instance.and_then(|p| p.page(kind)));
        spans.extend(
            source(own, default)
                .into_iter()
                .map(|s| Span::styled(s.content, dim(s.style))),
        );
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
        mouse::hit(row, Hit::ErrorPageRow(i));
    }
    frame.render_widget(Paragraph::new(Line::styled(note, theme::muted())), note_a);
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    let keys = action_chip(frame, keys, "⏎", "set page", theme::chip());
    let mut hints = vec![("↑↓", "move"), ("x", "reset")];
    if service.is_some() {
        hints.push(("b", "branded"));
    }
    hints.push(("esc", "close"));
    mouse::hints(frame, keys, &hints, Alignment::Left);
}
