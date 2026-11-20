//! Modal forms: one generic model, rendered as the dialog from the Paper design.

use crate::theme;
use common::Id;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text,
    /// One of a fixed set, switched with ←/→ and drawn as chips.
    Choice(Vec<String>),
    Toggle,
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
        }
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
}

pub enum FormAction {
    None,
    Cancel,
    /// `alt` is the secondary submit (ctrl+s), e.g. create without deploying.
    Submit {
        alt: bool,
    },
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

    pub fn focus_on(mut self, key: &str) -> Self {
        if let Some(i) = self.fields.iter().position(|f| f.key == key) {
            self.focus = i;
        }
        self
    }

    fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }

    fn field_mut(&mut self, key: &str) -> Option<&mut Field> {
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

    pub fn on(&self, key: &str) -> bool {
        self.field(key).is_some_and(|f| f.on)
    }

    pub fn fail(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.busy = false;
    }

    /// Show only the fields that apply to the current choices, and keep defaults in sync.
    fn refresh(&mut self) {
        if !matches!(self.kind, FormKind::NewService { .. }) {
            return;
        }
        let source = self.choice("source");
        let db = common::DatabaseKind::ALL[self
            .choice("engine")
            .min(common::DatabaseKind::ALL.len() - 1)];
        for f in &mut self.fields {
            f.visible = match f.key {
                "repo" | "branch" | "dockerfile" => source == 0,
                "image" => source == 1,
                "engine" | "version" => source == 2,
                "port" | "domain" => source != 2,
                _ => true,
            };
        }
        if let Some(version) = self.field_mut("version") {
            version.placeholder = db.default_version().to_string();
        }
        if let Some(name) = self.field_mut("name") {
            name.placeholder = match source {
                2 => db.image().to_string(),
                _ => "derived from the source".to_string(),
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

    fn move_focus(&mut self, step: isize) {
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
            KeyCode::Enter => return FormAction::Submit { alt: false },
            KeyCode::Tab | KeyCode::Down => self.move_focus(1),
            KeyCode::BackTab | KeyCode::Up => self.move_focus(-1),
            _ => {
                let field = &mut self.fields[self.focus];
                match (&field.kind, key.code) {
                    (FieldKind::Choice(options), KeyCode::Left | KeyCode::Char('h')) => {
                        field.choice = (field.choice + options.len() - 1) % options.len();
                    }
                    (
                        FieldKind::Choice(options),
                        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' '),
                    ) => {
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

    pub fn render(&self, frame: &mut Frame, area: Rect, tick: usize) {
        let visible: Vec<&Field> = self.fields.iter().filter(|f| f.visible).collect();
        let roomy = area.height as usize >= visible.len() * 2 + 9;
        let rows_per_field = if roomy { 2 } else { 1 };
        let width = area.width.saturating_sub(4).min(84);
        let height =
            (visible.len() * rows_per_field + 6 + usize::from(self.error.is_some())) as u16;
        let rect = area.centered(
            Constraint::Length(width),
            Constraint::Length(height.min(area.height)),
        );

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
        let inner = block.inner(rect).inner(ratatui::layout::Margin::new(2, 1));
        frame.render_widget(block, rect);

        let hint_w: u16 = if inner.width >= 70 { 22 } else { 0 };
        let label_w: u16 = 16;
        let mut y = inner.y;
        for (i, field) in self.fields.iter().enumerate().filter(|(_, f)| f.visible) {
            let focused = i == self.focus;
            let [label_a, value_a, hint_a] = Layout::horizontal([
                Constraint::Length(label_w),
                Constraint::Fill(1),
                Constraint::Length(hint_w),
            ])
            .areas(Rect::new(inner.x, y, inner.width, 1));
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
                    for (j, opt) in options.iter().enumerate() {
                        let style = if j == field.choice {
                            if focused {
                                theme::chip()
                            } else {
                                theme::text().bg(theme::LINE)
                            }
                        } else {
                            theme::muted()
                        };
                        spans.push(Span::styled(format!(" {opt} "), style));
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
            if roomy {
                if field.kind == FieldKind::Text {
                    let rule = "─".repeat(value_a.width as usize);
                    let style = if focused {
                        theme::accent()
                    } else {
                        theme::fg(theme::LINE)
                    };
                    frame.render_widget(
                        Paragraph::new(rule).style(style),
                        Rect::new(value_a.x, y + 1, value_a.width, 1),
                    );
                }
                y += 2;
            } else {
                y += 1;
            }
        }

        if let Some(error) = &self.error {
            frame.render_widget(
                Paragraph::new(format!("✕ {error}")).style(theme::fg(theme::ERR)),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }

        let actions_y = (y + 1).min(inner.bottom().saturating_sub(1));
        let rule = Rect::new(inner.x, actions_y, inner.width, 1);
        frame.render_widget(
            Paragraph::new("─".repeat(inner.width as usize)).style(theme::fg(theme::LINE)),
            rule,
        );
        let actions = Rect::new(inner.x, actions_y + 1, inner.width, 1);
        let mut left = vec![];
        for (k, l) in [("tab", "next"), ("⇧tab", "prev"), ("esc", "cancel")] {
            left.push(Span::styled(k, theme::accent()));
            left.push(Span::styled(format!(" {l}   "), theme::muted()));
        }
        frame.render_widget(Paragraph::new(Line::from(left)), actions);
        let mut right = vec![];
        if let Some(alt) = self.alt_label {
            right.push(Span::styled("ctrl+s", theme::accent()));
            right.push(Span::styled(format!(" {alt}   "), theme::muted()));
        }
        let submit = if self.busy {
            format!(" {} working… ", crate::util::spinner(tick))
        } else {
            format!(" ⏎ {} ", self.submit_label)
        };
        right.push(Span::styled(submit, theme::chip()));
        frame.render_widget(Paragraph::new(Line::from(right).right_aligned()), actions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

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
                    vec!["Git".into(), "Image".into(), "Database".into()],
                ),
                Field::text("repo", "repository"),
                Field::text("branch", "branch"),
                Field::text("dockerfile", "dockerfile"),
                Field::text("image", "image"),
                Field::choice("engine", "engine", vec!["postgres".into()]),
                Field::text("version", "version"),
                Field::text("name", "name"),
                Field::text("port", "port"),
                Field::text("domain", "domain"),
            ],
        )
    }

    #[test]
    fn source_choice_controls_visible_fields() {
        let mut form = service_form();
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
        form.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(form.fields[form.focus].key, "image");
        for c in "nginx".chars() {
            form.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(form.get("image"), "nginx");
        assert_eq!(form.get("repo"), "");
    }
}
