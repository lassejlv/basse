//! The picker dialog: choose an SSH key, S3 credentials or a backup store from a list.

use crate::form::Form;
use crate::mouse::List;
use crate::mouse::{self, Hit};
use crate::theme;
use crate::util::truncate;
use common::{Id, Secret, SecretKind, StorageConnection};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

/// What the list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    /// SSH key secrets, the daemon's own key and "generate new key".
    SshKey,
    /// S3 credentials secrets.
    S3Credentials,
    /// Where a backup goes: the data directory or a storage connection.
    Store,
    /// A notification channel for a rule; items are filled by the rule dialog.
    Channel,
    /// What a rule listens to; items are filled by the rule dialog.
    Scope,
}

/// Where the choice goes.
pub enum PickFor {
    /// A pick field of a form, which is shown again afterwards.
    Field(Box<Form>, &'static str),
    /// The key a server connects with.
    ServerKey { server_id: Id, server: String },
    /// A manual backup of a database service.
    BackupNow { service_id: Id, name: String },
    /// The channel or scope of the rule dialog, which is shown again afterwards.
    Rule(Box<crate::notify::RuleDialog>),
}

pub struct PickItem {
    /// None is the default choice, such as the daemon's own key.
    pub id: Option<Id>,
    pub name: String,
    pub detail: String,
    pub note: String,
    pub note_style: Style,
    /// Leading status glyph.
    pub glyph: Option<(&'static str, Style)>,
    /// Text that c copies, e.g. the public key.
    pub copy: Option<String>,
    /// The "generate new key" entry.
    pub generate: bool,
}

impl PickItem {
    pub fn new(id: Option<Id>, name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            detail: detail.into(),
            note: String::new(),
            note_style: theme::muted(),
            glyph: None,
            copy: None,
            generate: false,
        }
    }
}

pub struct Picker {
    pub title: String,
    pub subtitle: String,
    pub kind: PickKind,
    pub items: Vec<PickItem>,
    pub row: usize,
    pub target: PickFor,
    /// The value in use now, marked in the list.
    pub current: Option<Id>,
    pub note: Vec<Line<'static>>,
    /// Set while a key is being generated.
    pub busy: bool,
}

/// "ssh-ed25519 AAAAC3Nz…q9Lk" and the key's comment.
pub fn short_key(key: &str) -> (String, String) {
    let mut parts = key.split_whitespace();
    let kind = parts.next().unwrap_or_default();
    let body = parts.next().unwrap_or_default();
    let comment = parts.collect::<Vec<_>>().join(" ");
    let n = body.chars().count();
    let body = if n > 12 {
        let head: String = body.chars().take(8).collect();
        let tail: String = body.chars().skip(n - 4).collect();
        format!("{head}…{tail}")
    } else {
        body.to_string()
    };
    (format!("{kind} {body}"), comment)
}

/// "bucket / prefix", or the bucket alone.
pub fn bucket_path(s: &StorageConnection) -> String {
    if s.path_prefix.is_empty() {
        s.bucket.clone()
    } else {
        format!("{} / {}", s.bucket, s.path_prefix)
    }
}

/// Glyph, short label and style of a connection's last test.
pub fn test_state(s: &StorageConnection) -> (&'static str, String, Style) {
    match &s.last_test {
        None => ("○", "never tested".into(), theme::muted()),
        Some(t) if t.ok => (
            "●",
            format!(
                "ok {}ms · {}",
                t.latency_ms.unwrap_or_default(),
                crate::util::ago(t.at)
            ),
            theme::fg(theme::OK),
        ),
        Some(_) => ("✕", "last test failed".into(), theme::fg(theme::ERR)),
    }
}

fn store_item(s: &StorageConnection) -> PickItem {
    let (glyph, note, style) = test_state(s);
    let mut item = PickItem::new(Some(s.id.clone()), &s.name, bucket_path(s));
    item.glyph = Some((glyph, style));
    item.note = note;
    item.note_style = if style == theme::fg(theme::ERR) {
        style
    } else {
        theme::muted()
    };
    item
}

fn usage(s: &Secret) -> String {
    match s.used_by.as_slice() {
        [] => "unused".to_string(),
        [one] => one.to_string(),
        many => format!("used by {}", many.len()),
    }
}

impl Picker {
    /// Rebuild the list from the secrets, keeping the selected entry.
    pub fn fill(
        &mut self,
        secrets: &[Secret],
        storages: &[StorageConnection],
        daemon_key: Option<&str>,
    ) {
        if matches!(self.kind, PickKind::Channel | PickKind::Scope) {
            return;
        }
        let selected = self.items.get(self.row).map(|i| (i.id.clone(), i.generate));
        let mut items = Vec::new();
        match self.kind {
            PickKind::SshKey => {
                let (key, comment) = daemon_key.map(short_key).unwrap_or_default();
                let mut own = PickItem::new(
                    None,
                    "daemon's own key",
                    format!("{key} {comment}").trim().to_string(),
                );
                own.note = "built in".into();
                own.copy = daemon_key.map(str::to_string);
                items.push(own);
                for s in secrets.iter().filter(|s| s.kind == SecretKind::SshKey) {
                    let key = s.public_part.clone().unwrap_or_default();
                    let mut item = PickItem::new(Some(s.id.clone()), &s.name, short_key(&key).0);
                    item.note = usage(s);
                    item.copy = Some(key);
                    items.push(item);
                }
                let mut generate = PickItem::new(
                    None,
                    "+ generate new key",
                    "the daemon creates an ed25519 key",
                );
                generate.generate = true;
                items.push(generate);
            }
            PickKind::S3Credentials => {
                for s in secrets
                    .iter()
                    .filter(|s| s.kind == SecretKind::S3Credentials)
                {
                    let mut item = PickItem::new(
                        Some(s.id.clone()),
                        &s.name,
                        s.public_part.clone().unwrap_or_default(),
                    );
                    item.note = usage(s);
                    items.push(item);
                }
            }
            PickKind::Store => {
                items.push(PickItem::new(
                    None,
                    "local",
                    "the data directory on this machine",
                ));
                items.extend(storages.iter().map(store_item));
            }
            PickKind::Channel | PickKind::Scope => {}
        }
        self.items = items;
        if !matches!(self.target, PickFor::BackupNow { .. }) {
            self.mark_current();
        }
        self.row = selected
            .and_then(|(id, generate)| {
                self.items
                    .iter()
                    .position(|i| i.id == id && i.generate == generate)
            })
            .or_else(|| {
                self.items
                    .iter()
                    .position(|i| !i.generate && i.id == self.current)
            })
            .unwrap_or(0);
    }

    pub fn mark_current(&mut self) {
        for item in &mut self.items {
            if !item.generate && item.id == self.current {
                item.note = "current".into();
                item.glyph = Some(("●", theme::fg(theme::OK)));
            }
        }
    }

    pub fn select(&mut self, id: &str) {
        if let Some(i) = self.items.iter().position(|i| i.id.as_deref() == Some(id)) {
            self.row = i;
        }
    }

    pub fn selected(&self) -> Option<&PickItem> {
        self.items.get(self.row)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, tick: usize) {
        let width = area.width.saturating_sub(4).min(110);
        let list_h = self.items.len().max(1) as u16;
        let height = list_h + self.note.len() as u16 + 7;
        let rect = area.centered(
            Constraint::Length(width),
            Constraint::Length(height.min(area.height)),
        );
        mouse::modal(rect);
        frame.render_widget(Clear, rect);
        let mut heading = vec![
            Span::raw(" "),
            Span::styled(self.title.clone(), theme::text()),
        ];
        if !self.subtitle.is_empty() {
            heading.push(Span::styled(
                format!(" · {}", self.subtitle),
                theme::muted(),
            ));
        }
        heading.push(Span::raw(" "));
        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::muted())
            .style(theme::text().bg(theme::SURFACE))
            .title_top(Line::from(heading));
        let inner = block.inner(rect).inner(ratatui::layout::Margin::new(2, 1));
        frame.render_widget(block, rect);
        let [list, _, note, _, rule, keys] = Layout::vertical([
            Constraint::Length(list_h),
            Constraint::Length(1),
            Constraint::Length(self.note.len() as u16),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        let name_w = if self.kind == PickKind::Scope { 40 } else { 24 };
        let note_w = 22usize;
        let detail_w = (list.width as usize).saturating_sub(name_w + note_w + 4);
        mouse::hit(list, Hit::List(List::Picker));
        let mut lines = Vec::new();
        if self.items.is_empty() {
            lines.push(Line::styled(
                match self.kind {
                    PickKind::Channel => "no channels yet · add one on the Channels tab",
                    _ => "nothing to pick yet · add S3 credentials on the Secrets screen",
                },
                theme::muted(),
            ));
        }
        let offset = (self.row + 1).saturating_sub(list.height as usize);
        for (i, item) in self.items.iter().enumerate().skip(offset) {
            let selected = i == self.row;
            let (glyph, glyph_style) = item.glyph.unwrap_or((" ", theme::muted()));
            let detail_style = if item.generate {
                theme::muted()
            } else {
                theme::text()
            };
            lines.push(
                Line::from(vec![
                    Span::styled(format!("{glyph} "), glyph_style),
                    Span::styled(
                        format!("{:<name_w$}", truncate(&item.name, name_w - 1)),
                        if selected {
                            theme::accent()
                        } else {
                            theme::text()
                        },
                    ),
                    Span::styled(
                        format!(
                            "{:<detail_w$}",
                            truncate(&item.detail, detail_w.saturating_sub(1))
                        ),
                        detail_style,
                    ),
                    Span::styled(truncate(&item.note, note_w), item.note_style),
                ])
                .style(if selected {
                    Style::new().bg(theme::LINE)
                } else {
                    Style::new()
                }),
            );
            if ((i - offset) as u16) < list.height {
                mouse::hit(
                    Rect::new(list.x, list.y + (i - offset) as u16, list.width, 1),
                    Hit::PickRow(i),
                );
            }
        }
        frame.render_widget(Paragraph::new(lines), list);
        frame.render_widget(Paragraph::new(self.note.clone()), note);
        frame.render_widget(
            Paragraph::new("─".repeat(rule.width as usize)).style(theme::fg(theme::LINE)),
            rule,
        );

        let chip = match self.selected() {
            _ if self.busy => format!(" {} generating… ", crate::util::spinner(tick)),
            Some(i) if i.generate => " ⏎ generate ".to_string(),
            Some(i) if matches!(self.target, PickFor::BackupNow { .. }) => {
                format!(" ⏎ back up to {} ", truncate(&i.name, 30))
            }
            Some(i) => format!(" ⏎ use {} ", truncate(&i.name, 30)),
            None => " ⏎ close ".to_string(),
        };
        let chip = Span::styled(chip, theme::chip());
        let chip_w = (chip.width() as u16).min(keys.width);
        let chip_area = Rect::new(keys.right() - chip_w, keys.y, chip_w, 1);
        frame.render_widget(Paragraph::new(chip), chip_area);
        if !self.busy {
            mouse::key_target(chip_area, "⏎");
        }
        let copyable = self.selected().is_some_and(|i| i.copy.is_some());
        let hints: &[(&str, &str)] = if copyable {
            &[("↑↓", "move"), ("c", "copy public key"), ("esc", "cancel")]
        } else {
            &[("↑↓", "move"), ("esc", "cancel")]
        };
        mouse::hints(
            frame,
            Rect {
                width: keys.width.saturating_sub(chip_w + 1),
                ..keys
            },
            hints,
            Alignment::Left,
        );
    }
}
