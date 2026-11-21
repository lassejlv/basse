//! The picker dialog: choose an SSH key, S3 credentials, a backup store, or a git source,
//! repository and branch from a list.

use crate::form::Form;
use crate::mouse::List;
use crate::mouse::{self, Hit};
use crate::theme;
use crate::util::truncate;
use common::{GitBranch, GitRepo, Id, Secret, SecretKind, StorageConnection};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use std::time::Instant;

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
    /// A ready git source for a new service.
    GitSource,
    /// A repository of a source, searched and paged on the daemon.
    Repo,
    /// A branch of a repository, filtered here.
    Branch,
    /// The server a new service or database runs on.
    Server,
    /// A folder of a repository, for a service's root directory; ⏎ opens a folder.
    Dir,
    /// A fixed list its caller filled, such as a provider or a load balancer type.
    Fixed,
}

/// State of the repository and branch pickers: the search line and the page.
#[derive(Default)]
pub struct GitPick {
    pub source_id: Id,
    pub repo: String,
    pub search: String,
    /// 1-based.
    pub page: u32,
    pub pages: u32,
    /// Repositories matching the search across all pages.
    pub total: u32,
    pub loading: bool,
    /// When the search last changed; the request waits for typing to pause.
    pub typed_at: Option<Instant>,
    /// Every branch; `items` holds the ones matching the search.
    pub branches: Vec<GitBranch>,
    pub error: Option<String>,
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
    /// A muted tag after the name, e.g. `private` or `default`.
    pub badge: String,
    /// Leading status glyph.
    pub glyph: Option<(&'static str, Style)>,
    /// Text that c copies, e.g. the public key.
    pub copy: Option<String>,
    /// The "generate new key" entry.
    pub generate: bool,
    /// Shown muted and skipped by ↑↓ and clicks; `note` says why.
    pub disabled: bool,
}

impl PickItem {
    pub fn new(id: Option<Id>, name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            detail: detail.into(),
            note: String::new(),
            note_style: theme::muted(),
            badge: String::new(),
            glyph: None,
            copy: None,
            generate: false,
            disabled: false,
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
    /// The search line and paging of the repository and branch pickers.
    pub git: Option<GitPick>,
}

/// A repository row: `private` as a badge, the default branch and when it was last pushed.
pub fn repo_item(r: &GitRepo) -> PickItem {
    let mut item = PickItem::new(Some(r.full_name.clone()), &r.full_name, &r.default_branch);
    if r.private {
        item.badge = "private".into();
    }
    item.note = r.pushed_at.map(crate::util::ago).unwrap_or_default();
    item
}

/// The root directory picker's rows at `path` (empty is the repository root): use this
/// folder, go up, then each folder under it.
pub fn dir_items(path: &str, dirs: &[String]) -> Vec<PickItem> {
    let shown = |p: &str| {
        if p.is_empty() {
            "/".to_string()
        } else {
            format!("{p}/")
        }
    };
    let mut use_it = PickItem::new(
        Some(path.to_string()),
        format!("use {}", shown(path)),
        "build this folder",
    );
    use_it.generate = true;
    use_it.glyph = Some(("✓", theme::fg(theme::OK)));
    let mut items = vec![use_it];
    if !path.is_empty() {
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        let up = if parent.is_empty() {
            "up to the repository root".to_string()
        } else {
            format!("up to {parent}/")
        };
        items.push(PickItem::new(Some(parent.to_string()), "../", up));
    }
    items.extend(dirs.iter().map(|d| {
        let full = if path.is_empty() {
            d.clone()
        } else {
            format!("{path}/{d}")
        };
        PickItem::new(Some(full), format!("{d}/"), "")
    }));
    items
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
            PickKind::GitSource
            | PickKind::Repo
            | PickKind::Branch
            | PickKind::Server
            | PickKind::Dir
            | PickKind::Fixed => return,
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

    /// Show the branches whose name contains the search, the default one first.
    pub fn filter_branches(&mut self) {
        let Some(git) = &self.git else {
            return;
        };
        let needle = git.search.to_lowercase();
        let mut branches: Vec<&GitBranch> = git
            .branches
            .iter()
            .filter(|b| b.name.to_lowercase().contains(&needle))
            .collect();
        branches.sort_by_key(|b| !b.default);
        self.items = branches
            .into_iter()
            .map(|b| {
                let mut item = PickItem::new(Some(b.name.clone()), &b.name, "");
                if b.default {
                    item.badge = "default".into();
                }
                item
            })
            .collect();
        self.row = self
            .current
            .as_ref()
            .and_then(|c| self.items.iter().position(|i| i.id.as_ref() == Some(c)))
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

    /// Move the selection by `step` rows, over disabled ones.
    pub fn step(&mut self, step: isize) {
        let mut row = self.row;
        loop {
            match row
                .checked_add_signed(step)
                .filter(|r| *r < self.items.len())
            {
                Some(next) if self.items[next].disabled => row = next,
                Some(next) => return self.row = next,
                None => return,
            }
        }
    }

    /// Draw the dialog frame with its title, centered in `area`; returns the padded inside.
    fn open(&self, frame: &mut Frame, area: Rect, width: u16, height: u16) -> Rect {
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
        inner
    }

    /// The footer: hints on the left, the accept chip on the right.
    fn footer(&self, frame: &mut Frame, keys: Rect, chip: String, hints: &[(&str, &str)]) {
        let chip = Span::styled(chip, theme::chip());
        let chip_w = (chip.width() as u16).min(keys.width);
        let chip_area = Rect::new(keys.right() - chip_w, keys.y, chip_w, 1);
        frame.render_widget(Paragraph::new(chip), chip_area);
        if !self.busy {
            mouse::key_target(chip_area, "⏎");
        }
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

    pub fn render(&self, frame: &mut Frame, area: Rect, tick: usize) {
        if self.git.is_some() {
            return self.render_search(frame, area, tick);
        }
        let width = area.width.saturating_sub(4).min(110);
        let list_h = self.items.len().max(1) as u16;
        let height = list_h + self.note.len() as u16 + 7;
        let inner = self.open(frame, area, width, height);
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
        let note_w = if self.kind == PickKind::Server {
            30
        } else {
            22
        };
        let detail_w = (list.width as usize).saturating_sub(name_w + note_w + 4);
        mouse::hit(list, Hit::List(List::Picker));
        let mut lines = Vec::new();
        if self.items.is_empty() {
            lines.push(Line::styled(
                match self.kind {
                    PickKind::Channel => "no channels yet · add one on the Channels tab",
                    PickKind::Fixed => "nothing to pick yet",
                    _ => "nothing to pick yet · add S3 credentials on the Secrets screen",
                },
                theme::muted(),
            ));
        }
        let offset = (self.row + 1).saturating_sub(list.height as usize);
        for (i, item) in self.items.iter().enumerate().skip(offset) {
            let selected = i == self.row;
            let (glyph, glyph_style) = item.glyph.unwrap_or((" ", theme::muted()));
            let detail_style = if item.generate || item.disabled {
                theme::muted()
            } else {
                theme::text()
            };
            lines.push(
                Line::from(vec![
                    Span::styled(format!("{glyph} "), glyph_style),
                    Span::styled(
                        truncate(&item.name, name_w - 1),
                        match (selected, item.disabled) {
                            (_, true) => theme::muted(),
                            (true, false) => theme::accent(),
                            (false, false) => theme::text(),
                        },
                    ),
                    Span::styled(
                        {
                            let used = truncate(&item.name, name_w - 1).chars().count();
                            let badge = if item.badge.is_empty() {
                                String::new()
                            } else {
                                format!("  {}", item.badge)
                            };
                            let badge = truncate(&badge, (name_w - 1).saturating_sub(used));
                            format!("{badge:<w$}", w = name_w - used)
                        },
                        theme::muted(),
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
            Some(i) if i.generate && self.kind == PickKind::GitSource => {
                " ⏎ open sources ".to_string()
            }
            Some(i) if self.kind == PickKind::Dir && i.generate => {
                format!(" ⏎ {} ", truncate(&i.name, 30))
            }
            Some(_) if self.kind == PickKind::Dir => " ⏎ open ".to_string(),
            Some(i) if i.generate => " ⏎ generate ".to_string(),
            Some(i) if matches!(self.target, PickFor::BackupNow { .. }) => {
                format!(" ⏎ back up to {} ", truncate(&i.name, 30))
            }
            Some(i) => format!(" ⏎ use {} ", truncate(&i.name, 30)),
            None => " ⏎ close ".to_string(),
        };
        let copyable = self.selected().is_some_and(|i| i.copy.is_some());
        let hints: &[(&str, &str)] = if copyable {
            &[("↑↓", "move"), ("c", "copy public key"), ("esc", "cancel")]
        } else {
            &[("↑↓", "move"), ("esc", "cancel")]
        };
        self.footer(frame, keys, chip, hints);
    }

    /// The repository and branch pickers: a search line, the rows and the page.
    fn render_search(&self, frame: &mut Frame, area: Rect, tick: usize) {
        let Some(git) = &self.git else {
            return;
        };
        let repos = self.kind == PickKind::Repo;
        let list_h = if repos {
            crate::app::REPO_PAGE as u16
        } else {
            git.branches.len().clamp(1, 12) as u16
        };
        let header_h = u16::from(repos);
        let width = area.width.saturating_sub(4).min(110);
        let inner = self.open(frame, area, width, list_h + header_h + 11);
        let [search, underline, header, list, _, status, _, rule, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(header_h),
            Constraint::Length(list_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        let [label, input] =
            Layout::horizontal([Constraint::Length(10), Constraint::Fill(1)]).areas(search);
        frame.render_widget(Paragraph::new("› search").style(theme::accent()), label);
        let cursor = if tick % 8 < 5 { "▏" } else { " " };
        let mut line = vec![
            Span::raw(" "),
            Span::styled(git.search.clone(), theme::text()),
            Span::styled(cursor, theme::accent()),
        ];
        if git.search.is_empty() {
            line.push(Span::styled(
                match (repos, git.total) {
                    (true, 0) => "type to filter repositories".to_string(),
                    (true, n) => format!("type to filter {n} repositories"),
                    (false, _) => format!("type to filter {} branches", git.branches.len()),
                },
                theme::muted(),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(line)), input);
        frame.render_widget(
            Paragraph::new("─".repeat(input.width as usize)).style(theme::accent()),
            Rect {
                y: underline.y,
                ..input
            },
        );

        let columns = |row: Rect| {
            Layout::horizontal([
                Constraint::Length(2),
                Constraint::Fill(1),
                Constraint::Length(10),
                Constraint::Length(if repos { 18 } else { 0 }),
                Constraint::Length(if repos { 10 } else { 0 }),
            ])
            .areas::<5>(row)
        };
        if repos {
            let [_, name, _, branch, pushed] = columns(header);
            for (area, text) in [
                (name, "NAME"),
                (branch, "DEFAULT BRANCH"),
                (pushed, "PUSHED"),
            ] {
                frame.render_widget(Paragraph::new(text).style(theme::muted()), area);
            }
        }

        mouse::hit(list, Hit::List(List::Picker));
        if self.items.is_empty() {
            let (text, style) = match (&git.error, git.loading) {
                (Some(e), _) => (format!("✕ {e}"), theme::fg(theme::ERR)),
                (None, true) => (
                    format!("{} loading…", crate::util::spinner(tick)),
                    theme::fg(theme::INFO),
                ),
                (None, false) if repos => ("no repository matches".into(), theme::muted()),
                (None, false) => ("no branch matches".into(), theme::muted()),
            };
            frame.render_widget(Paragraph::new(format!("  {text}")).style(style), list);
        }
        let offset = self.row.saturating_sub(list_h.saturating_sub(1) as usize);
        for (i, item) in self
            .items
            .iter()
            .enumerate()
            .skip(offset)
            .take(list_h as usize)
        {
            let selected = i == self.row;
            let row = Rect::new(list.x, list.y + (i - offset) as u16, list.width, 1);
            if selected {
                frame.render_widget(Block::new().style(Style::new().bg(theme::LINE)), row);
            }
            let [_, name, badge, branch, pushed] = columns(row);
            let name_style = if selected {
                theme::accent()
            } else {
                theme::text()
            };
            frame.render_widget(
                Paragraph::new(truncate(&item.name, name.width.saturating_sub(1) as usize))
                    .style(name_style),
                name,
            );
            frame.render_widget(
                Paragraph::new(item.badge.clone()).style(theme::muted()),
                badge,
            );
            frame.render_widget(
                Paragraph::new(item.detail.clone()).style(theme::text()),
                branch,
            );
            frame.render_widget(
                Paragraph::new(item.note.clone()).style(theme::muted()),
                pushed,
            );
            mouse::hit(row, Hit::PickRow(i));
        }

        let mut line = Vec::new();
        if git.loading && !self.items.is_empty() {
            line.push(Span::styled(
                format!("{} ", crate::util::spinner(tick)),
                theme::fg(theme::INFO),
            ));
        }
        if repos {
            line.extend([
                Span::styled("page ", theme::muted()),
                Span::styled(git.page.to_string(), theme::text()),
                Span::styled(
                    format!(
                        " of {} · {} {}",
                        git.pages.max(1),
                        git.total,
                        if git.search.is_empty() {
                            "repositories · newest push first".to_string()
                        } else {
                            format!("matching {}", git.search)
                        }
                    ),
                    theme::muted(),
                ),
            ]);
        } else if git.search.is_empty() {
            line.push(Span::styled(
                format!("{} branches", git.branches.len()),
                theme::muted(),
            ));
        } else {
            line.push(Span::styled(
                format!("{} of {} branches", self.items.len(), git.branches.len()),
                theme::muted(),
            ));
        }
        if let Some(e) = git.error.as_ref().filter(|_| !self.items.is_empty()) {
            line.push(Span::styled(format!("  ✕ {e}"), theme::fg(theme::ERR)));
        }
        frame.render_widget(Paragraph::new(Line::from(line)), status);
        frame.render_widget(
            Paragraph::new("─".repeat(rule.width as usize)).style(theme::fg(theme::LINE)),
            rule,
        );
        let chip = match self.selected() {
            Some(i) => format!(" ⏎ use {} ", truncate(&i.name, 30)),
            None => " ⏎ back ".to_string(),
        };
        let hints: &[(&str, &str)] = if repos {
            &[
                ("type", "search"),
                ("↑↓", "move"),
                ("←→", "page"),
                ("esc", "back"),
            ]
        } else {
            &[("type", "search"), ("↑↓", "move"), ("esc", "back")]
        };
        self.footer(frame, keys, chip, hints);
    }
}
