//! Modal forms: one generic model, rendered as the dialog from the Paper design.

use crate::mouse::{self, Hit};
use crate::theme;
use common::Id;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text,
    /// One of a fixed set, switched with ←/→ and drawn as chips.
    Choice(Vec<String>),
    Toggle,
    /// A value chosen in a picker dialog; ⏎ opens it. `value` holds the id, `shown` its name.
    Pick,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub value: String,
    pub choice: usize,
    pub on: bool,
    pub placeholder: String,
    pub hint: String,
    pub visible: bool,
    /// Typed value is masked until revealed.
    pub masked: bool,
    /// What a pick field shows for its value.
    pub shown: String,
    /// A choice whose pick only lights up while the field is focused, for a row of options where
    /// the focused row is the answer.
    pub quiet: bool,
}

impl Field {
    pub fn text(key: &'static str, label: &'static str) -> Self {
        Self {
            key,
            label,
            kind: FieldKind::Text,
            value: String::new(),
            choice: 0,
            on: false,
            placeholder: String::new(),
            hint: String::new(),
            visible: true,
            masked: false,
            shown: String::new(),
            quiet: false,
        }
    }

    /// A field filled from a picker: `id` is the stored value, `shown` its label.
    pub fn pick(
        key: &'static str,
        label: &'static str,
        id: impl Into<String>,
        shown: impl Into<String>,
    ) -> Self {
        Self {
            kind: FieldKind::Pick,
            value: id.into(),
            shown: shown.into(),
            ..Self::text(key, label)
        }
    }

    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    pub fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }

    pub fn choice(key: &'static str, label: &'static str, options: Vec<String>) -> Self {
        Self {
            kind: FieldKind::Choice(options),
            ..Self::text(key, label)
        }
    }

    pub fn toggle(key: &'static str, label: &'static str, on: bool) -> Self {
        Self {
            kind: FieldKind::Toggle,
            on,
            ..Self::text(key, label)
        }
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn selected(mut self, choice: usize) -> Self {
        self.choice = choice;
        self
    }
}

/// What a submitted form turns into; carries the ids the request needs.
#[derive(Debug, Clone, PartialEq)]
pub enum FormKind {
    NewProject,
    NewEnvironment {
        project_id: Id,
    },
    NewService {
        environment_id: Id,
    },
    EditService {
        service_id: Id,
        /// The git source the service clones from, which makes auto-deploy editable.
        source_id: Option<Id>,
    },
    AddDomain {
        service_id: Id,
    },
    EnvVar {
        service_id: Id,
        original_key: Option<String>,
    },
    AddVolume {
        service_id: Id,
    },
    AddServer,
    PublicIp,
    PurgeServer {
        server_id: Id,
    },
    /// Asks for the host port; submitting opens a confirm dialog.
    ExposeDatabase {
        service_id: Id,
        name: String,
    },
    BackupSchedule {
        service_id: Id,
    },
    DownloadBackup {
        backup_id: Id,
        file_name: String,
        /// An instance backup rather than a database backup.
        instance: bool,
    },
    AddSecret,
    GenerateKey,
    RenameSecret {
        secret_id: Id,
    },
    ReplaceSecret {
        secret_id: Id,
        kind: common::SecretKind,
    },
    AddStorage,
    EditStorage {
        storage_id: Id,
    },
    InstanceBackup,
    InstanceSchedule {
        has_passphrase: bool,
    },
    /// First step of the channel wizard: the focused row's choice is the kind.
    NotifyKind,
    /// Second step of the channel wizard. `channel_id` is set when editing, or once the wizard
    /// has created the channel (`created`).
    NotifyChannel {
        kind: common::notify::ChannelKind,
        channel_id: Option<Id>,
        created: bool,
    },
    /// Starts the GitHub App manifest flow.
    CreateGithubApp,
    ConnectGithubApp,
    AddGithubToken,
}

pub enum FormAction {
    None,
    Cancel,
    /// `alt` is the secondary submit (ctrl+s), e.g. create without deploying.
    Submit {
        alt: bool,
    },
    /// ⏎ on a pick field: open its picker.
    Pick(&'static str),
}

pub struct Form {
    pub kind: FormKind,
    pub title: String,
    pub subtitle: String,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
    pub submit_label: &'static str,
    pub alt_label: Option<&'static str>,
    pub busy: bool,
    /// Shown above the actions: the first line with a red ▲, the rest muted.
    pub warning: Vec<String>,
    /// Muted explanation shown above the actions.
    pub note: Vec<String>,
    /// A line above the fields, such as the steps of a wizard.
    pub header: Option<Line<'static>>,
}

impl Form {
    pub fn new(kind: FormKind, title: impl Into<String>, fields: Vec<Field>) -> Self {
        let mut form = Self {
            kind,
            title: title.into(),
            subtitle: String::new(),
            fields,
            focus: 0,
            error: None,
            submit_label: "save",
            alt_label: None,
            busy: false,
            warning: Vec::new(),
            note: Vec::new(),
            header: None,
        };
        form.refresh();
        form.focus = form.next_visible(0, 1);
        form
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    pub fn submit(mut self, label: &'static str, alt: Option<&'static str>) -> Self {
        self.submit_label = label;
        self.alt_label = alt;
        self
    }

    pub fn warning(mut self, lines: Vec<String>) -> Self {
        self.warning = lines;
        self
    }

    pub fn note(mut self, lines: Vec<String>) -> Self {
        self.note = lines;
        self
    }

    pub fn header(mut self, line: Line<'static>) -> Self {
        self.header = Some(line);
        self
    }

    pub fn focus_on(mut self, key: &str) -> Self {
        if let Some(i) = self.fields.iter().position(|f| f.key == key) {
            self.focus = i;
        }
        self
    }

    fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }

    pub fn field_mut(&mut self, key: &str) -> Option<&mut Field> {
        self.fields.iter_mut().find(|f| f.key == key)
    }

    /// Trimmed text of a visible field; empty when hidden.
    pub fn get(&self, key: &str) -> String {
        self.field(key)
            .filter(|f| f.visible)
            .map(|f| f.value.trim().to_string())
            .unwrap_or_default()
    }

    /// Untrimmed text of a field, for values where whitespace matters.
    pub fn raw(&self, key: &str) -> String {
        self.field(key).map(|f| f.value.clone()).unwrap_or_default()
    }

    pub fn choice(&self, key: &str) -> usize {
        self.field(key).map_or(0, |f| f.choice)
    }

    /// The chosen option of a visible choice field.
    pub fn choice_label(&self, key: &str) -> Option<String> {
        let f = self.field(key).filter(|f| f.visible)?;
        match &f.kind {
            FieldKind::Choice(options) => options.get(f.choice).cloned(),
            _ => None,
        }
    }

    pub fn on(&self, key: &str) -> bool {
        self.field(key).is_some_and(|f| f.on)
    }

    pub fn fail(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.busy = false;
    }

    /// Show only the fields that apply to the current choices, and keep defaults in sync.
    pub fn refresh(&mut self) {
        if self.kind == FormKind::CreateGithubApp {
            let organization = self.choice("owner") == 1;
            if let Some(f) = self.field_mut("organization") {
                f.visible = organization;
            }
            return;
        }
        if self.kind == FormKind::PublicIp {
            let fixed = self.choice("mode") == 1;
            if let Some(ip) = self.field_mut("ip") {
                ip.visible = fixed;
            }
            return;
        }
        if matches!(self.kind, FormKind::BackupSchedule { .. }) {
            let hint = crate::database::hour_hint(&self.raw("hour"));
            if let Some(hour) = self.field_mut("hour") {
                hour.hint = hint;
            }
            return;
        }
        if matches!(
            self.kind,
            FormKind::InstanceBackup | FormKind::InstanceSchedule { .. }
        ) {
            let (p, repeat) = (self.raw("passphrase"), self.raw("repeat"));
            let n = p.chars().count();
            let min = common::api::MIN_PASSPHRASE_LEN;
            if let Some(f) = self.field_mut("passphrase") {
                f.hint = match n {
                    0 => format!("{min}+ characters"),
                    n if n < min => format!("{n} of {min} characters"),
                    n => format!("✓ {n} characters"),
                };
            }
            if let Some(f) = self.field_mut("repeat") {
                f.hint = match () {
                    _ if repeat.is_empty() => String::new(),
                    _ if repeat == p => "✓ matches".into(),
                    _ => "must match".into(),
                };
            }
            if let Some(hour) = self.field_mut("hour") {
                hour.hint = crate::database::hour_hint(&hour.value);
            }
            return;
        }
        if let FormKind::NotifyChannel {
            kind: common::notify::ChannelKind::EmailSmtp,
            ..
        } = self.kind
        {
            let port = match self.choice("security") {
                0 => "587",
                1 => "465",
                _ => "25",
            };
            if let Some(f) = self.field_mut("port")
                && ["", "587", "465", "25"].contains(&f.value.as_str())
            {
                f.value = port.into();
            }
            return;
        }
        if self.kind == FormKind::AddSecret {
            let kind = self.choice("kind");
            for f in &mut self.fields {
                f.visible = match f.key {
                    "private_key" => kind == 0,
                    "access_key_id" | "secret_access_key" => kind == 1,
                    "value" => kind == 2,
                    _ => true,
                };
            }
            return;
        }
        if matches!(self.kind, FormKind::EditService { .. }) {
            let has_build = self.field("build").is_some();
            self.build_fields(has_build);
            return;
        }
        if !matches!(self.kind, FormKind::NewService { .. }) {
            return;
        }
        let source = self.choice("source");
        let db = common::DatabaseKind::ALL[self
            .choice("engine")
            .min(common::DatabaseKind::ALL.len() - 1)];
        for f in &mut self.fields {
            f.visible = match f.key {
                "git_source" | "gh_repo" | "gh_branch" | "auto_deploy" => source == 0,
                "repo" | "branch" => source == 1,
                "build" | "dockerfile" | "context" | "target" | "compose_file" => source <= 1,
                "image" => source == 2,
                "engine" | "version" => source == 3,
                "port" | "domain" => source != 3,
                crate::server_pick::KEY => f.visible,
                _ => true,
            };
        }
        self.build_fields(source <= 1);
        let token = self.field("git_source").is_some_and(|f| {
            f.placeholder
                .starts_with(common::GitSourceKind::GithubToken.label())
        });
        let branch = self.get("gh_branch");
        if let Some(auto) = self.field_mut("auto_deploy") {
            auto.hint = match (token, branch.as_str()) {
                (true, _) => "needs a GitHub App".into(),
                (false, "") => "deploys every push".into(),
                (false, b) => format!("every push to {b}"),
            };
        }
        if let Some(version) = self.field_mut("version") {
            version.placeholder = db.default_version().to_string();
        }
        if let Some(name) = self.field_mut("name") {
            name.placeholder = match source {
                3 => db.image().to_string(),
                _ => "derived from the source".to_string(),
            };
        }
    }

    /// Show the fields of the chosen build method. The build chips' placeholder is a fixed hint,
    /// e.g. what the repository was detected as; otherwise the hint describes the method.
    fn build_fields(&mut self, git: bool) {
        let method =
            common::BuildMethod::ALL[self.choice("build").min(common::BuildMethod::ALL.len() - 1)];
        for f in &mut self.fields {
            match f.key {
                "build" => f.visible = git,
                "dockerfile" | "context" | "target" => {
                    f.visible = git && method == common::BuildMethod::Dockerfile
                }
                "compose_file" => f.visible = git && method == common::BuildMethod::Compose,
                _ => {}
            }
        }
        if let Some(build) = self.field_mut("build") {
            build.hint = match method {
                _ if !build.placeholder.is_empty() => build.placeholder.clone(),
                common::BuildMethod::Auto => "picks on first deploy".into(),
                common::BuildMethod::Railpack => "detects the language".into(),
                common::BuildMethod::Dockerfile | common::BuildMethod::Compose => String::new(),
            };
        }
    }

    fn next_visible(&self, from: usize, step: isize) -> usize {
        let n = self.fields.len() as isize;
        let mut i = from as isize;
        for _ in 0..n {
            if self.fields[i.rem_euclid(n) as usize].visible {
                return i.rem_euclid(n) as usize;
            }
            i += step;
        }
        from
    }

    /// Focus field `i` from a mouse click; a chip click also picks that choice and a toggle flips.
    pub fn click(&mut self, i: usize, choice: Option<usize>) {
        if self.busy || !self.fields.get(i).is_some_and(|f| f.visible) {
            return;
        }
        self.focus = i;
        let field = &mut self.fields[i];
        match (&field.kind, choice) {
            (FieldKind::Choice(options), Some(c)) if c < options.len() => field.choice = c,
            (FieldKind::Toggle, _) => field.on = !field.on,
            _ => return,
        }
        self.error = None;
        self.refresh();
    }

    pub fn move_focus(&mut self, step: isize) {
        let n = self.fields.len() as isize;
        let start = (self.focus as isize + step).rem_euclid(n) as usize;
        self.focus = self.next_visible(start, step);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> FormAction {
        if self.busy {
            return FormAction::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return FormAction::Cancel,
            KeyCode::Char('s') if ctrl => {
                return FormAction::Submit {
                    alt: self.alt_label.is_some(),
                };
            }
            KeyCode::Enter | KeyCode::Char(' ')
                if self.fields[self.focus].kind == FieldKind::Pick =>
            {
                return FormAction::Pick(self.fields[self.focus].key);
            }
            KeyCode::Enter => return FormAction::Submit { alt: false },
            KeyCode::Tab | KeyCode::Down => self.move_focus(1),
            KeyCode::BackTab | KeyCode::Up => self.move_focus(-1),
            _ => {
                let field = &mut self.fields[self.focus];
                match (&field.kind, key.code) {
                    (FieldKind::Choice(options), KeyCode::Left) => {
                        field.choice = (field.choice + options.len() - 1) % options.len();
                    }
                    (FieldKind::Choice(options), KeyCode::Right | KeyCode::Char(' ')) => {
                        field.choice = (field.choice + 1) % options.len();
                    }
                    (FieldKind::Toggle, KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right) => {
                        field.on = !field.on
                    }
                    (FieldKind::Text, KeyCode::Backspace) => {
                        field.value.pop();
                    }
                    (FieldKind::Text, KeyCode::Char('u')) if ctrl => field.value.clear(),
                    (FieldKind::Text, KeyCode::Char('w')) if ctrl => {
                        let trimmed = field.value.trim_end().len();
                        field.value.truncate(trimmed);
                        let cut = field
                            .value
                            .rfind([' ', '/', '-', '.', ':'])
                            .map_or(0, |i| i + 1);
                        field.value.truncate(cut);
                    }
                    (FieldKind::Text, KeyCode::Char(c)) if !ctrl => field.value.push(c),
                    _ => {}
                }
                self.error = None;
                self.refresh();
            }
        }
        FormAction::None
    }

    /// Pasted text goes into the focused text field as typed, newlines included.
    pub fn paste(&mut self, text: &str) {
        if self.busy {
            return;
        }
        let field = &mut self.fields[self.focus];
        if field.kind == FieldKind::Text {
            field.value.push_str(text.trim_end_matches(['\r', '\n']));
            self.error = None;
            self.refresh();
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, tick: usize) {
        let area = area.intersection(frame.area());
        let visible: Vec<_> = self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, field)| field.visible)
            .collect();
        let width = area.width.saturating_sub(4).min(92);
        let warning_rows = if self.warning.is_empty() {
            0
        } else {
            self.warning.len() + 1
        };
        let note_rows = if self.note.is_empty() {
            0
        } else {
            self.note.len() + 1
        };
        let header_rows = if self.header.is_some() { 2 } else { 0 };
        let status_rows =
            warning_rows + note_rows + usize::from(self.error.is_some()) + header_rows;
        let roomy = area.height as usize >= visible.len() * 2 + 9 + status_rows;
        let rows_per_field = if roomy { 2 } else { 1 };
        let height = visible.len() * rows_per_field + 6 + status_rows;
        let rect = area.centered(
            Constraint::Length(width),
            Constraint::Length(height.min(usize::from(area.height)) as u16),
        );

        mouse::modal(rect);
        frame.render_widget(Clear, rect);
        let mut title = vec![
            Span::raw(" "),
            Span::styled(self.title.clone(), theme::text()),
        ];
        if !self.subtitle.is_empty() {
            title.push(Span::styled(
                format!(" · {}", self.subtitle),
                theme::muted(),
            ));
        }
        title.push(Span::raw(" "));
        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::muted())
            .style(theme::text().bg(theme::SURFACE))
            .title_top(Line::from(title));
        let content = block.inner(rect);
        frame.render_widget(block, rect);
        if content.is_empty() {
            return;
        }
        let pad_x = if content.width > 4 { 2 } else { 0 };
        let pad_top = u16::from(content.height > 3);
        let inner = Rect::new(
            content.x + pad_x,
            content.y + pad_top,
            content.width - 2 * pad_x,
            content.height - pad_top,
        );
        let actions = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
        let show_rule = inner.height >= 3;
        let body_bottom = actions.y - u16::from(show_rule);
        let body_rows = usize::from(body_bottom - inner.y);
        mouse::hit(
            Rect::new(inner.x, inner.y, inner.width, body_rows as u16),
            Hit::List(mouse::List::Form),
        );
        let head = match &self.header {
            Some(line) if body_rows > 3 => {
                frame.render_widget(
                    Paragraph::new(line.clone()),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                2
            }
            _ => 0,
        };
        let body_rows = body_rows - head;
        let field_rows = rows_per_field.min(body_rows);
        let error_rows = usize::from(self.error.is_some() && body_rows > field_rows);
        let warnings = self.warning.len().min(body_rows - field_rows - error_rows);
        let notes = self
            .note
            .len()
            .min(body_rows - field_rows - error_rows - warnings);
        let capacity = (body_rows - error_rows - warnings - notes) / rows_per_field;
        let start = visible
            .iter()
            .position(|(i, _)| *i == self.focus)
            .unwrap_or(0)
            .saturating_sub(capacity.saturating_sub(1));

        let hint_w: u16 = if inner.width >= 76 { 24 } else { 0 };
        let label_w = 16.min(inner.width / 2);
        let mut y = inner.y + head as u16;
        for (i, field) in visible.into_iter().skip(start).take(capacity) {
            let focused = i == self.focus;
            let hint_w = match field.kind {
                FieldKind::Choice(_) if field.hint.is_empty() => 0,
                _ => hint_w,
            };
            let row = Rect::new(inner.x, y, inner.width, 1);
            let [label_a, value_a, hint_a] = Layout::horizontal([
                Constraint::Length(label_w),
                Constraint::Fill(1),
                Constraint::Length(hint_w),
            ])
            .areas(row);
            mouse::hit(row, Hit::FormField(i));
            let label = if focused {
                format!("› {}", field.label)
            } else {
                field.label.to_string()
            };
            frame.render_widget(
                Paragraph::new(label).style(if focused {
                    theme::accent()
                } else {
                    theme::muted()
                }),
                label_a,
            );
            let value_line = match &field.kind {
                FieldKind::Choice(options) => {
                    let mut spans = Vec::new();
                    let mut x = value_a.x;
                    for (j, opt) in options.iter().enumerate() {
                        if x >= value_a.right() {
                            break;
                        }
                        let style = match (j == field.choice, focused, field.quiet) {
                            (true, true, _) => theme::chip(),
                            (true, false, false) => theme::text().bg(theme::LINE),
                            _ => theme::muted(),
                        };
                        let chip = Span::styled(format!(" {opt} "), style);
                        let width = u16::try_from(chip.width()).unwrap_or(u16::MAX);
                        mouse::hit(
                            Rect::new(x, y, width.min(value_a.right() - x), 1),
                            Hit::FormChoice(i, j),
                        );
                        x = x.saturating_add(width).saturating_add(1);
                        spans.push(chip);
                        spans.push(Span::raw(" "));
                    }
                    Line::from(spans)
                }
                FieldKind::Toggle => {
                    let (glyph, text) = if field.on {
                        ("●", "on")
                    } else {
                        ("○", "off")
                    };
                    let style = if field.on {
                        theme::fg(theme::OK)
                    } else {
                        theme::muted()
                    };
                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled(format!("{glyph} {text}"), style),
                    ])
                }
                FieldKind::Pick => {
                    let action = if focused {
                        theme::accent()
                    } else {
                        theme::muted()
                    };
                    let room = (value_a.width as usize).saturating_sub(9);
                    let shown = crate::util::truncate(&field.shown, room);
                    let detail = crate::util::truncate(
                        &field.placeholder,
                        room.saturating_sub(shown.chars().count() + 2),
                    );
                    let gap = if shown.is_empty() { "" } else { "  " };
                    let used = 1 + shown.chars().count() + gap.len() + detail.chars().count();
                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled(shown, theme::text()),
                        Span::styled(format!("{gap}{detail}"), theme::muted()),
                        Span::raw(" ".repeat((value_a.width as usize).saturating_sub(used + 7))),
                        Span::styled("⏎ pick", action),
                    ])
                }
                FieldKind::Text => {
                    let shown = if field.masked && !field.value.is_empty() {
                        "•".repeat(field.value.chars().count().min(32))
                    } else {
                        field.value.clone()
                    };
                    let max = value_a.width.saturating_sub(3) as usize;
                    let skip = shown.chars().count().saturating_sub(max);
                    let shown: String = shown.chars().skip(skip).collect();
                    let mut spans = vec![Span::raw(" ")];
                    if shown.is_empty() && !focused {
                        spans.push(Span::styled(field.placeholder.clone(), theme::muted()));
                    } else {
                        spans.push(Span::styled(shown, theme::text()));
                    }
                    if focused {
                        let cursor = if tick % 8 < 5 { "▏" } else { " " };
                        spans.push(Span::styled(cursor, theme::accent()));
                        if field.value.is_empty() && !field.placeholder.is_empty() {
                            spans.push(Span::styled(field.placeholder.clone(), theme::muted()));
                        }
                    }
                    Line::from(spans)
                }
            };
            frame.render_widget(Paragraph::new(value_line), value_a);
            if hint_w > 0 && !field.hint.is_empty() {
                let style = if field.hint.starts_with('✓') {
                    theme::fg(theme::OK)
                } else {
                    theme::muted()
                };
                frame.render_widget(
                    Paragraph::new(format!("  {}", field.hint)).style(style),
                    hint_a,
                );
            }
            if roomy && matches!(field.kind, FieldKind::Text | FieldKind::Pick) {
                let rule = "─".repeat(value_a.width as usize);
                let style = if focused {
                    theme::accent()
                } else {
                    theme::fg(theme::LINE)
                };
                let underline = Rect::new(value_a.x, y + 1, value_a.width, 1);
                frame.render_widget(Paragraph::new(rule).style(style), underline);
                mouse::hit(underline, Hit::FormField(i));
            }
            y += rows_per_field as u16;
        }

        if warnings > 0 {
            if usize::from(body_bottom - y) > warnings + error_rows {
                y += 1;
            }
            for (i, line) in self.warning.iter().take(warnings).enumerate() {
                let spans = if i == 0 {
                    vec![
                        Span::styled("▲ ", theme::fg(theme::ERR)),
                        Span::styled(line.clone(), theme::text()),
                    ]
                } else {
                    vec![Span::styled(format!("  {line}"), theme::muted())]
                };
                frame.render_widget(
                    Paragraph::new(Line::from(spans)),
                    Rect::new(inner.x, y, inner.width, 1),
                );
                y += 1;
            }
        }

        if notes > 0 {
            if usize::from(body_bottom - y) > notes + error_rows {
                y += 1;
            }
            for line in self.note.iter().take(notes) {
                frame.render_widget(
                    Paragraph::new(line.clone()).style(theme::muted()),
                    Rect::new(inner.x, y, inner.width, 1),
                );
                y += 1;
            }
        }

        if let Some(error) = &self.error
            && error_rows > 0
        {
            frame.render_widget(
                Paragraph::new(format!("✕ {error}")).style(theme::fg(theme::ERR)),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }

        if show_rule {
            frame.render_widget(
                Paragraph::new("─".repeat(inner.width as usize)).style(theme::fg(theme::LINE)),
                Rect::new(inner.x, body_bottom, inner.width, 1),
            );
        }
        let submit = if self.busy {
            format!(" {} working… ", crate::util::spinner(tick))
        } else {
            format!(" ⏎ {} ", self.submit_label)
        };
        let submit = Line::styled(submit, theme::chip()).right_aligned();
        let submit_w = submit.width().min(usize::from(actions.width)) as u16;
        let submit_area = Rect::new(actions.right() - submit_w, actions.y, submit_w, 1);
        frame.render_widget(submit, submit_area);
        if !self.busy {
            mouse::key_target(submit_area, "⏎");
        }
        let mut left_w = (submit_area.x - actions.x).saturating_sub(3);
        if let Some(alt) = self.alt_label {
            let alt = Line::from(vec![
                Span::styled("ctrl+s", theme::accent()),
                Span::styled(format!(" {alt}"), theme::muted()),
            ])
            .right_aligned();
            let alt_w = alt.width().min(usize::from(left_w)) as u16;
            left_w -= alt_w;
            let alt_area = Rect::new(actions.x + left_w, actions.y, alt_w, 1);
            frame.render_widget(alt, alt_area);
            if !self.busy {
                mouse::key_target(alt_area, "ctrl+s");
            }
            left_w = left_w.saturating_sub(3);
        }
        let navigation = [("tab", "next"), ("⇧tab", "prev"), ("esc", "cancel")];
        let compact = [navigation[0], navigation[2]];
        let hints = if mouse::hints_width(&navigation) <= usize::from(left_w) {
            navigation.as_slice()
        } else if mouse::hints_width(&compact) <= usize::from(left_w) {
            compact.as_slice()
        } else {
            &navigation[2..]
        };
        mouse::hints(
            frame,
            Rect::new(actions.x, actions.y, left_w, 1),
            hints,
            Alignment::Left,
        );
        if self.busy {
            mouse::hit(inner, Hit::Block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Overlay};
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn service_form() -> Form {
        Form::new(
            FormKind::NewService {
                environment_id: "e".into(),
            },
            "New service",
            vec![
                Field::choice(
                    "source",
                    "source",
                    vec![
                        "GitHub".into(),
                        "Git URL".into(),
                        "Image".into(),
                        "Database".into(),
                    ],
                ),
                Field::pick("git_source", "git source", "", ""),
                Field::pick("gh_repo", "repository", "", ""),
                Field::pick("gh_branch", "branch", "", ""),
                Field::text("repo", "repository"),
                Field::text("branch", "branch"),
                Field::choice(
                    "build",
                    "build",
                    vec![
                        "auto".into(),
                        "Dockerfile".into(),
                        "Compose".into(),
                        "Railpack".into(),
                    ],
                ),
                Field::text("dockerfile", "dockerfile"),
                Field::text("context", "context"),
                Field::text("target", "target"),
                Field::text("compose_file", "compose file"),
                Field::text("image", "image"),
                Field::choice("engine", "engine", vec!["postgres".into()]),
                Field::text("version", "version"),
                Field::text("name", "name"),
                Field::text("port", "port"),
                Field::text("domain", "domain"),
                Field::toggle("auto_deploy", "auto-deploy", true),
            ],
        )
    }

    fn fixture(form: Form) -> App {
        let client = client::Client::new("http://localhost", "test").unwrap();
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(client, "test".into(), tx);
        app.overlay = Some(Overlay::Form(form));
        app
    }

    fn click(app: &mut App, (column, row): (u16, u16)) {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            mouse::handle(
                app,
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
            );
        }
    }

    fn draw<'a>(terminal: &'a mut Terminal<TestBackend>, app: &App) -> &'a Buffer {
        let Some(Overlay::Form(form)) = &app.overlay else {
            panic!("expected a form overlay");
        };
        mouse::reset();
        terminal
            .draw(|frame| {
                form.render(frame, frame.area(), 0);
                mouse::finish_frame(&app.mouse);
            })
            .unwrap()
            .buffer
    }

    #[test]
    fn compact_form_footer_advances_to_fields_below_the_initial_view() {
        let mut app = fixture(
            service_form()
                .submit("create & deploy", Some("create only"))
                .focus_on("port"),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 7)).unwrap();
        let row = |buf: &Buffer, y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        };
        let buf = draw(&mut terminal, &app);
        let modal = mouse::modal_bounds(&app.mouse).unwrap();
        assert!(
            mouse::targets(&app.mouse)
                .iter()
                .all(|(target, _)| modal.intersection(*target) == *target)
        );
        assert!(row(buf, 3).contains("› port"));
        assert!(row(buf, 6).contains('╯'));
        let footer = row(buf, 5);
        assert!(footer.contains("ctrl+s create only"));
        assert!(footer.contains("⏎ create & deploy"));
        let alt_x = footer.find("ctrl+s").unwrap() as u16;
        assert_eq!(
            mouse::hit_at(&app.mouse, alt_x, 5),
            Some(Hit::Key(KeyCode::Char('s'), KeyModifiers::CONTROL))
        );
        let tab_x = footer.find("tab next").unwrap() as u16;
        click(&mut app, (tab_x, 5));

        let buf = draw(&mut terminal, &app);
        assert!(row(buf, 3).contains("› domain"));
        let cancel_x = row(buf, 5).find("esc cancel").unwrap() as u16;
        click(&mut app, (cancel_x, 5));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn wide_choice_cells_and_toggle_rows_follow_what_is_drawn() {
        let form = Form::new(
            FormKind::NewProject,
            "Mouse fixture",
            vec![
                Field::choice("source", "source", vec!["界界".into(), "Image".into()]).selected(1),
                Field::toggle("tls", "https", false),
            ],
        );
        let mut app = fixture(form);
        let mut terminal = Terminal::new(TestBackend::new(90, 16)).unwrap();
        let buf = draw(&mut terminal, &app);
        let symbol = |name| {
            buf.pos_of(
                buf.content()
                    .iter()
                    .rposition(|c| c.symbol() == name)
                    .unwrap(),
            )
        };
        let (wide_x, wide_y) = symbol("界");
        let (toggle_x, toggle_y) = symbol("○");
        click(&mut app, (wide_x + 1, wide_y));
        click(&mut app, (toggle_x, toggle_y + 1));
        let Some(Overlay::Form(form)) = &app.overlay else {
            panic!("expected a form overlay");
        };
        assert_eq!(form.choice("source"), 0);
        assert!(!form.on("tls"));

        click(&mut app, (toggle_x, toggle_y));
        let Some(Overlay::Form(form)) = &app.overlay else {
            panic!("expected a form overlay");
        };
        assert!(form.on("tls"));
    }

    #[test]
    fn schedule_hour_hint_follows_the_typed_hour() {
        let mut form = Form::new(
            FormKind::BackupSchedule {
                service_id: "s".into(),
            },
            "Backup schedule",
            vec![
                Field::toggle("enabled", "daily", true),
                Field::text("hour", "hour (UTC)").value("3"),
                Field::text("keep", "keep").value("7"),
            ],
        );
        let hint = |f: &Form| f.fields[1].hint.clone();
        assert!(hint(&form).ends_with(" local"));
        form.handle_key(KeyEvent::from(KeyCode::Tab));
        form.handle_key(KeyEvent::from(KeyCode::Char('9')));
        assert_eq!(form.get("hour"), "39");
        assert_eq!(hint(&form), "0-23");
    }

    #[test]
    fn secret_kind_shows_its_value_fields_and_pastes_stay_whole() {
        let mut form = Form::new(
            FormKind::AddSecret,
            "Add secret",
            vec![
                Field::choice(
                    "kind",
                    "kind",
                    vec!["ssh".into(), "s3".into(), "generic".into()],
                ),
                Field::text("name", "name"),
                Field::text("private_key", "private key").masked(),
                Field::text("access_key_id", "access key id"),
                Field::text("secret_access_key", "secret key").masked(),
                Field::text("value", "value").masked(),
            ],
        );
        let visible = |f: &Form| -> Vec<&str> {
            f.fields
                .iter()
                .filter(|x| x.visible)
                .map(|x| x.key)
                .collect()
        };
        assert_eq!(visible(&form), ["kind", "name", "private_key"]);
        form.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(
            visible(&form),
            ["kind", "name", "access_key_id", "secret_access_key"]
        );
        form.handle_key(KeyEvent::from(KeyCode::Left));
        form.focus = 2;
        form.paste(
            "-----BEGIN KEY-----
abc
-----END KEY-----
",
        );
        assert_eq!(
            form.raw("private_key"),
            "-----BEGIN KEY-----
abc
-----END KEY-----"
        );
    }

    #[test]
    fn enter_on_a_pick_field_opens_its_picker() {
        let mut form = Form::new(
            FormKind::AddServer,
            "Add server",
            vec![
                Field::text("name", "name"),
                Field::pick("ssh_key", "ssh key", "", "daemon's own key"),
            ],
        );
        assert!(matches!(
            form.handle_key(KeyEvent::from(KeyCode::Enter)),
            FormAction::Submit { .. }
        ));
        form.handle_key(KeyEvent::from(KeyCode::Tab));
        assert!(matches!(
            form.handle_key(KeyEvent::from(KeyCode::Enter)),
            FormAction::Pick("ssh_key")
        ));
    }

    #[test]
    fn build_method_shows_its_own_fields_and_hint() {
        let mut form = service_form();
        let visible = |f: &Form| -> Vec<&str> {
            ["build", "dockerfile", "context", "target", "compose_file"]
                .into_iter()
                .filter(|k| f.fields.iter().any(|x| x.key == *k && x.visible))
                .collect()
        };
        let hint = |f: &Form| {
            f.fields
                .iter()
                .find(|x| x.key == "build")
                .unwrap()
                .hint
                .clone()
        };
        assert_eq!(visible(&form), ["build"]);
        assert_eq!(hint(&form), "picks on first deploy");
        form.focus = form.fields.iter().position(|f| f.key == "build").unwrap();
        form.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(visible(&form), ["build", "dockerfile", "context", "target"]);
        form.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(visible(&form), ["build", "compose_file"]);
        form.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(visible(&form), ["build"]);
        assert_eq!(hint(&form), "detects the language");
        form.field_mut("build").unwrap().placeholder = "detected: Dockerfile".into();
        form.refresh();
        assert_eq!(hint(&form), "detected: Dockerfile");
        form.focus = 0;
        form.handle_key(KeyEvent::from(KeyCode::Right));
        form.handle_key(KeyEvent::from(KeyCode::Right));
        assert!(visible(&form).is_empty(), "images have no build");
    }

    #[test]
    fn source_choice_controls_visible_fields() {
        let mut form = service_form();
        let visible = |f: &Form, key: &str| f.fields.iter().any(|x| x.key == key && x.visible);
        for key in ["git_source", "gh_repo", "gh_branch", "auto_deploy", "build"] {
            assert!(visible(&form, key), "{key} shows for GitHub");
        }
        assert!(!visible(&form, "repo"));
        form.handle_key(KeyEvent::from(KeyCode::Right));
        assert!(visible(&form, "repo") && visible(&form, "build"));
        assert!(!visible(&form, "git_source") && !visible(&form, "auto_deploy"));
        assert!(
            form.fields
                .iter()
                .find(|f| f.key == "repo")
                .unwrap()
                .visible
        );
        assert!(
            !form
                .fields
                .iter()
                .find(|f| f.key == "image")
                .unwrap()
                .visible
        );
        form.handle_key(KeyEvent::from(KeyCode::Right));
        assert!(
            !form
                .fields
                .iter()
                .find(|f| f.key == "repo")
                .unwrap()
                .visible
        );
        assert!(
            form.fields
                .iter()
                .find(|f| f.key == "image")
                .unwrap()
                .visible
        );
    }

    #[test]
    fn typing_edits_the_focused_field_and_tab_skips_hidden_ones() {
        let mut form = service_form();
        form.handle_key(KeyEvent::from(KeyCode::Right));
        form.handle_key(KeyEvent::from(KeyCode::Right));
        form.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(form.fields[form.focus].key, "image");
        for c in "nginx".chars() {
            form.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(form.get("image"), "nginx");
        assert_eq!(form.get("repo"), "");
    }
}
