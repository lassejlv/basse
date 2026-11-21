//! The Notifications screen: channels, rules and the delivery log, the channel wizard's last
//! step and the rule dialog.

use crate::app::{App, Screen};
use crate::form::{Field, Form, FormKind};
use crate::mouse::{self, Hit, List};
use crate::picker::PickItem;
use crate::theme;
use crate::util::{self, truncate};
use crate::view::{
    dialog, inside, lines_of, panel, row_hits, rule_line, scroll_offset, setting_row, title,
};
use chrono::{DateTime, Local, Utc};
use common::notify::*;
use common::{Id, Tree};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};
use std::collections::{BTreeSet, HashSet};
use std::time::Instant;

/// How many deliveries the log loads.
pub const LOG_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Channels,
    Rules,
    Deliveries,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Channels, Section::Rules, Section::Deliveries];

    pub fn title(self) -> &'static str {
        match self {
            Section::Channels => "Channels",
            Section::Rules => "Rules",
            Section::Deliveries => "Delivery log",
        }
    }

    pub fn step(self, step: isize) -> Self {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as isize;
        Self::ALL[(i + step).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Default)]
pub struct NotifyView {
    pub section: Section,
    pub channels: Vec<NotifyChannel>,
    pub rules: Vec<NotifyRule>,
    /// Newest first.
    pub deliveries: Vec<NotifyDelivery>,
    pub channel_row: usize,
    pub rule_row: usize,
    pub delivery_row: usize,
    /// Channels being tested from the list right now.
    pub testing: HashSet<Id>,
    /// Only failed and retrying deliveries.
    pub failures_only: bool,
    /// Only deliveries of this channel.
    pub channel_filter: Option<Id>,
    pub loaded_at: Option<Instant>,
    /// Screen to return to on esc.
    pub back: Option<Screen>,
}

impl NotifyView {
    pub fn selected_channel(&self) -> Option<&NotifyChannel> {
        self.channels.get(self.channel_row)
    }

    pub fn selected_rule(&self) -> Option<&NotifyRule> {
        self.rules.get(self.rule_row)
    }

    pub fn deliveries(&self) -> Vec<&NotifyDelivery> {
        self.deliveries
            .iter()
            .filter(|d| {
                !self.failures_only
                    || matches!(d.status, DeliveryStatus::Failed | DeliveryStatus::Retrying)
            })
            .collect()
    }

    pub fn selected_delivery(&self) -> Option<&NotifyDelivery> {
        self.deliveries().get(self.delivery_row).copied()
    }

    pub fn channel(&self, id: &str) -> Option<&NotifyChannel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn len(&self) -> usize {
        match self.section {
            Section::Channels => self.channels.len(),
            Section::Rules => self.rules.len(),
            Section::Deliveries => self.deliveries().len(),
        }
    }

    pub fn row(&self) -> usize {
        match self.section {
            Section::Channels => self.channel_row,
            Section::Rules => self.rule_row,
            Section::Deliveries => self.delivery_row,
        }
    }

    pub fn select(&mut self, row: usize) {
        let row = row.min(self.len().saturating_sub(1));
        match self.section {
            Section::Channels => self.channel_row = row,
            Section::Rules => self.rule_row = row,
            Section::Deliveries => self.delivery_row = row,
        }
    }

    fn failed_today(&self) -> usize {
        let since = Utc::now() - chrono::Duration::hours(24);
        self.deliveries
            .iter()
            .filter(|d| d.at >= since && d.status == DeliveryStatus::Failed)
            .count()
    }
}

/// Keep the selection on the same item when a list reloads.
pub fn keep_row<T>(old: Option<&str>, list: &[T], id: impl Fn(&T) -> &str, row: usize) -> usize {
    old.and_then(|old| list.iter().position(|x| id(x) == old))
        .unwrap_or(row)
        .min(list.len().saturating_sub(1))
}

// Formatting

pub fn marker(severity: Severity) -> Span<'static> {
    match severity {
        Severity::Critical => Span::styled("●", theme::fg(theme::ERR)),
        Severity::Warning => Span::styled("○", theme::fg(theme::ERR)),
        Severity::Success => Span::styled("●", theme::fg(theme::OK)),
        Severity::Info => Span::styled("●", theme::fg(theme::INFO)),
    }
}

/// 30s, 5m, 1h.
pub fn span(secs: u32) -> String {
    match secs {
        0 => "off".into(),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M:%S").to_string()
}

/// "project acme", "env acme/production", "instance": the kind muted, the label as text.
pub fn scope_spans(scope: &RuleScope, label: &str, muted: bool) -> Vec<Span<'static>> {
    let style = if muted { theme::muted() } else { theme::text() };
    let kind = match scope {
        RuleScope::Instance => return vec![Span::styled("instance", style)],
        RuleScope::Project { .. } => "project",
        RuleScope::Environment { .. } => "env",
        RuleScope::Service { .. } => "service",
    };
    vec![
        Span::styled(format!("{kind} "), theme::muted()),
        Span::styled(label.to_string(), style),
    ]
}

fn scope_detail(scope: &RuleScope) -> &'static str {
    match scope {
        RuleScope::Instance => "every project, server and the daemon",
        RuleScope::Project { .. } => "every environment and service in it",
        RuleScope::Environment { .. } => "every service in it",
        RuleScope::Service { .. } => "this service only",
    }
}

/// The preset a rule started from, and whether its events were changed since.
pub fn preset_label(rule_preset: Option<&str>, events: &[NotifyEvent]) -> (String, bool) {
    match rule_preset.and_then(preset) {
        Some(p) => {
            let mut a = p.events();
            let mut b = events.to_vec();
            a.sort();
            b.sort();
            (p.id.to_string(), a != b)
        }
        None => ("custom".into(), false),
    }
}

fn test_line(test: Option<&ChannelTest>, testing: bool, tick: usize) -> Line<'static> {
    if testing {
        return Line::styled(
            format!("{} testing…", util::spinner(tick)),
            theme::fg(theme::INFO),
        );
    }
    match test {
        None => Line::styled("never tested", theme::muted()),
        Some(t) if t.ok => Line::from(vec![
            Span::styled("ok", theme::text()),
            Span::styled(
                format!(
                    " {}ms · {}",
                    t.latency_ms.unwrap_or_default(),
                    util::ago(t.at)
                ),
                theme::muted(),
            ),
        ]),
        Some(t) => Line::from(vec![
            Span::styled("failed", theme::fg(theme::ERR)),
            Span::styled(
                format!(
                    " {} · {}",
                    truncate(t.error.as_deref().unwrap_or("no reason given"), 20),
                    util::ago(t.at)
                ),
                theme::muted(),
            ),
        ]),
    }
}

fn status_glyph(status: DeliveryStatus) -> (&'static str, Style) {
    match status {
        DeliveryStatus::Sent => ("●", theme::fg(theme::OK)),
        DeliveryStatus::Failed => ("✕", theme::fg(theme::ERR)),
        DeliveryStatus::Retrying => ("↻", theme::fg(theme::INFO)),
        DeliveryStatus::Suppressed => ("○", theme::muted()),
    }
}

fn status_style(status: DeliveryStatus) -> Style {
    match status {
        DeliveryStatus::Sent => theme::text(),
        DeliveryStatus::Failed => theme::fg(theme::ERR),
        DeliveryStatus::Retrying => theme::fg(theme::INFO),
        DeliveryStatus::Suppressed => theme::muted(),
    }
}

fn delivery_result(d: &NotifyDelivery) -> Span<'static> {
    match (d.status, &d.error, d.latency_ms) {
        (DeliveryStatus::Failed, Some(e), _) => {
            Span::styled(truncate(e, 30), theme::fg(theme::ERR))
        }
        (_, Some(e), _) => Span::styled(truncate(e, 30), theme::muted()),
        (_, None, Some(ms)) => Span::styled(format!("{ms}ms"), theme::muted()),
        _ => Span::styled("—", theme::muted()),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

// The screen

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.notify;
    let [head, tabs, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .spacing(1)
    .areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Notifications", theme::text()),
            Span::styled(
                format!(
                    "   {} · {} · {} in 24h",
                    plural(view.channels.len(), "channel", "channels"),
                    plural(view.rules.len(), "rule", "rules"),
                    plural(view.failed_today(), "failed delivery", "failed deliveries"),
                ),
                theme::muted(),
            ),
        ])),
        head,
    );
    let mut spans = vec![Span::raw(" ")];
    let mut offset = 1;
    for s in Section::ALL {
        let w = Span::raw(s.title()).width();
        mouse::span_target(tabs, offset, w, Hit::NotifySection(s));
        offset += w + 4;
        let style = if s == view.section {
            theme::accent().add_modifier(ratatui::style::Modifier::UNDERLINED)
        } else {
            theme::muted()
        };
        spans.push(Span::styled(s.title(), style));
        spans.push(Span::raw("    "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs);
    match view.section {
        Section::Channels => channels(frame, body, app),
        Section::Rules => rules(frame, body, app),
        Section::Deliveries => deliveries(frame, body, app),
    }
}

fn empty(frame: &mut Frame, area: Rect, before: &str, key: &str, after: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(before.to_string(), theme::muted()),
            Span::styled(key.to_string(), theme::accent()),
            Span::styled(after.to_string(), theme::muted()),
        ])),
        area,
    );
}

fn list_panel(
    frame: &mut Frame,
    area: Rect,
    heading: &str,
    right: String,
    detail_h: u16,
) -> (Rect, Rect) {
    let [list, detail] = Layout::vertical([Constraint::Fill(1), Constraint::Length(detail_h)])
        .spacing(1)
        .areas(area);
    let block = panel(
        title(heading, true),
        Some(Line::styled(right, theme::muted())),
        true,
    );
    let inner = inside(&block, list);
    frame.render_widget(block, list);
    mouse::hit(inner, Hit::List(List::Notify));
    (inner, detail)
}

fn enabled_glyph(on: bool) -> Span<'static> {
    if on {
        Span::styled("●", theme::fg(theme::OK))
    } else {
        Span::styled("○", theme::muted())
    }
}

fn row_style(selected: bool) -> Style {
    if selected {
        theme::selected()
    } else {
        Style::new()
    }
}

fn channels(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.notify;
    let enabled = view.channels.iter().filter(|c| c.enabled).count();
    let (inner, detail) = list_panel(
        frame,
        area,
        "Channels",
        format!(
            "{} · {enabled} enabled",
            plural(view.channels.len(), "channel", "channels")
        ),
        9,
    );
    if view.channels.is_empty() {
        empty(
            frame,
            inner,
            "no channels yet · ",
            "a",
            " to add email, Discord, Slack, Telegram, ntfy or a webhook",
        );
    } else {
        let rows: Vec<Row> = view
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let selected = i == view.channel_row;
                let text = if c.enabled {
                    theme::text()
                } else {
                    theme::muted()
                };
                Row::new(vec![
                    Cell::from(enabled_glyph(c.enabled)),
                    Cell::from(Span::styled(
                        c.name.clone(),
                        if selected { theme::accent() } else { text },
                    )),
                    Cell::from(Span::styled(c.kind.label(), theme::muted())),
                    Cell::from(Span::styled(c.target.clone(), text)),
                    Cell::from(test_line(
                        c.last_test.as_ref(),
                        view.testing.contains(&c.id),
                        app.tick,
                    )),
                ])
                .style(row_style(selected))
            })
            .collect();
        let offset = scroll_offset(view.channel_row, inner.height.saturating_sub(1) as usize);
        row_hits(inner, rows.len(), offset, Hit::Item);
        frame.render_widget(
            Table::new(
                rows.into_iter().skip(offset),
                [
                    Constraint::Length(2),
                    Constraint::Length(20),
                    Constraint::Length(20),
                    Constraint::Fill(1),
                    Constraint::Length(40),
                ],
            )
            .header(Row::new(["", "NAME", "KIND", "TARGET", "LAST TEST"]).style(theme::muted()))
            .column_spacing(1),
            inner,
        );
    }
    channel_detail(frame, detail, app);
}

fn sealed(present: bool) -> Vec<Span<'static>> {
    if present {
        vec![
            Span::styled("••••••••••••", theme::muted()),
            Span::styled("  sealed", theme::muted()),
        ]
    } else {
        vec![Span::styled("none", theme::muted())]
    }
}

fn config_rows(c: &NotifyChannel) -> Vec<(&'static str, Vec<Span<'static>>)> {
    let text = |t: String| vec![Span::styled(t, theme::text())];
    let mut rows = vec![("kind", text(c.kind.label().to_string()))];
    match &c.config {
        ChannelConfig::Email { provider, from, to } => {
            match provider {
                EmailProvider::Smtp {
                    host,
                    port,
                    security,
                    username,
                    password,
                } => {
                    rows.push((
                        "server",
                        vec![
                            Span::styled(format!("{host}:{port}"), theme::text()),
                            Span::styled(format!("  {}", security.as_str()), theme::muted()),
                        ],
                    ));
                    if let Some(u) = username.as_ref().filter(|u| !u.is_empty()) {
                        rows.push(("username", text(u.clone())));
                    }
                    rows.push((
                        "password",
                        sealed(password.as_ref().is_some_and(|p| !p.is_empty())),
                    ));
                }
                EmailProvider::Resend { .. } => rows.push(("api key", sealed(true))),
                EmailProvider::Cloudflare { account_id, .. } => {
                    rows.push(("account", text(account_id.clone())));
                    rows.push(("api token", sealed(true)));
                }
            }
            rows.push(("from", text(from.clone())));
            rows.push(("to", text(to.join(", "))));
        }
        ChannelConfig::Discord { .. }
        | ChannelConfig::Slack { .. }
        | ChannelConfig::Mattermost { .. }
        | ChannelConfig::GoogleChat { .. }
        | ChannelConfig::Teams { .. } => rows.push((
            "webhook",
            vec![
                Span::styled(c.target.clone(), theme::text()),
                Span::styled("  the rest of the URL is sealed", theme::muted()),
            ],
        )),
        ChannelConfig::Telegram { chat_id, .. } => {
            rows.push(("bot token", sealed(true)));
            rows.push(("chat id", text(chat_id.clone())));
        }
        ChannelConfig::Ntfy {
            server,
            topic,
            token,
        } => {
            rows.push(("server", text(server.clone())));
            rows.push(("topic", text(topic.clone())));
            rows.push(("token", sealed(token.is_some())));
        }
        ChannelConfig::Gotify { server, .. } => {
            rows.push(("server", text(server.clone())));
            rows.push(("app token", sealed(true)));
        }
        ChannelConfig::Pushover { .. } => {
            rows.push(("user key", sealed(true)));
            rows.push(("app token", sealed(true)));
        }
        ChannelConfig::Webhook {
            secret, headers, ..
        } => {
            rows.push(("url", text(c.target.clone())));
            rows.push((
                "signing",
                if secret.is_some() {
                    vec![
                        Span::styled("hmac-sha256", theme::text()),
                        Span::styled("  secret sealed", theme::muted()),
                    ]
                } else {
                    vec![Span::styled("unsigned", theme::muted())]
                },
            ));
            if !headers.is_empty() {
                let names: Vec<_> = headers.iter().map(|h| h.name.clone()).collect();
                rows.push(("headers", text(names.join(", "))));
            }
        }
    }
    rows
}

fn channel_detail(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.notify;
    let Some(c) = view.selected_channel() else {
        frame.render_widget(panel(title("channel", false), None, false), area);
        return;
    };
    let block = panel(title(&c.name, false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(4)
        .areas(inner);
    let config = config_rows(c);
    for (row, (k, v)) in lines_of(left, config.len() as u16).iter().zip(config) {
        setting_row(frame, *row, k, v, None);
    }
    let error = c.last_test.as_ref().and_then(|t| t.error.clone());
    let rows = lines_of(right, 4 + u16::from(error.is_some()));
    let state = if c.enabled {
        vec![
            Span::styled("● ", theme::fg(theme::OK)),
            Span::styled("enabled", theme::text()),
        ]
    } else {
        vec![
            Span::styled("○ ", theme::muted()),
            Span::styled("disabled", theme::muted()),
            Span::styled("  nothing is sent here", theme::muted()),
        ]
    };
    let toggle = if c.enabled { "disable" } else { "enable" };
    setting_row(frame, rows[0], "state", state, Some(("e", toggle)));
    let test = match &c.last_test {
        _ if view.testing.contains(&c.id) => vec![Span::styled(
            format!("{} sending a test…", util::spinner(app.tick)),
            theme::fg(theme::INFO),
        )],
        None => vec![Span::styled("never tested", theme::muted())],
        Some(t) if t.ok => vec![
            Span::styled("ok", theme::text()),
            Span::styled(
                format!(
                    "  delivered in {}ms · {}",
                    t.latency_ms.unwrap_or_default(),
                    util::ago(t.at)
                ),
                theme::muted(),
            ),
        ],
        Some(t) => vec![
            Span::styled("failed", theme::fg(theme::ERR)),
            Span::styled(format!("  {}", util::ago(t.at)), theme::muted()),
        ],
    };
    setting_row(frame, rows[1], "last test", test, Some(("t", "test")));
    let rows = match error {
        Some(e) => {
            setting_row(
                frame,
                rows[2],
                "",
                vec![Span::styled(e, theme::fg(theme::ERR))],
                None,
            );
            &rows[1..]
        }
        None => &rows[..],
    };
    let used: Vec<&NotifyRule> = view.rules.iter().filter(|r| r.channel_id == c.id).collect();
    let scopes = used
        .iter()
        .map(|r| {
            let (preset, _) = preset_label(r.preset.as_deref(), &r.events);
            format!("{} · {preset}", r.scope_label)
        })
        .collect::<Vec<_>>()
        .join(", ");
    setting_row(
        frame,
        rows[2],
        "used by",
        match used.len() {
            0 => vec![Span::styled(
                "no rules yet · add one on the Rules tab",
                theme::muted(),
            )],
            n => vec![
                Span::styled(plural(n, "rule", "rules"), theme::text()),
                Span::styled(format!("  {scopes}"), theme::muted()),
            ],
        },
        None,
    );
    let since = Utc::now() - chrono::Duration::hours(24);
    let recent: Vec<_> = view
        .deliveries
        .iter()
        .filter(|d| d.channel_id == c.id && d.at >= since)
        .collect();
    let count = |s: DeliveryStatus| recent.iter().filter(|d| d.status == s).count();
    setting_row(
        frame,
        rows[3],
        "last 24h",
        vec![
            Span::styled(
                format!("{} sent", count(DeliveryStatus::Sent)),
                theme::text(),
            ),
            Span::styled(
                format!(
                    "  {} failed · {} suppressed",
                    count(DeliveryStatus::Failed),
                    count(DeliveryStatus::Suppressed)
                ),
                theme::muted(),
            ),
        ],
        None,
    );
}

fn rules(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.notify;
    let enabled = view.rules.iter().filter(|r| r.enabled).count();
    let (inner, detail) = list_panel(
        frame,
        area,
        "Rules",
        format!(
            "{} · {enabled} enabled",
            plural(view.rules.len(), "rule", "rules")
        ),
        9,
    );
    if view.rules.is_empty() {
        if view.channels.is_empty() {
            empty(
                frame,
                inner,
                "add a channel on the Channels tab first, then ",
                "a",
                " adds a rule",
            );
        } else {
            empty(
                frame,
                inner,
                "no rules yet · ",
                "a",
                " to choose which events reach a channel",
            );
        }
    } else {
        let rows: Vec<Row> = view
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let selected = i == view.rule_row;
                let muted = !r.enabled;
                let text = if muted { theme::muted() } else { theme::text() };
                let channel = view
                    .channel(&r.channel_id)
                    .map_or("unknown channel".into(), |c| c.name.clone());
                let (preset, edited) = preset_label(r.preset.as_deref(), &r.events);
                let preset_style = if preset == "custom" {
                    theme::muted()
                } else {
                    text
                };
                let mut preset_spans = vec![Span::styled(preset, preset_style)];
                if edited {
                    preset_spans.push(Span::styled(" · edited", theme::muted()));
                }
                Row::new(vec![
                    Cell::from(enabled_glyph(r.enabled)),
                    Cell::from(Span::styled(
                        channel,
                        if selected { theme::accent() } else { text },
                    )),
                    Cell::from(Line::from(scope_spans(&r.scope, &r.scope_label, muted))),
                    Cell::from(Line::from(preset_spans)),
                    Cell::from(Span::styled(
                        plural(r.events.len(), "event", "events"),
                        text,
                    )),
                    Cell::from(Span::styled(
                        format!("dedupe {} · {}/h", span(r.dedupe_secs), r.max_per_hour),
                        theme::muted(),
                    )),
                ])
                .style(row_style(selected))
            })
            .collect();
        let offset = scroll_offset(view.rule_row, inner.height.saturating_sub(1) as usize);
        row_hits(inner, rows.len(), offset, Hit::Item);
        frame.render_widget(
            Table::new(
                rows.into_iter().skip(offset),
                [
                    Constraint::Length(2),
                    Constraint::Length(20),
                    Constraint::Length(36),
                    Constraint::Length(24),
                    Constraint::Length(16),
                    Constraint::Fill(1),
                ],
            )
            .header(
                Row::new(["", "CHANNEL", "SCOPE", "PRESET", "EVENTS", "LIMITS"])
                    .style(theme::muted()),
            )
            .column_spacing(1),
            inner,
        );
    }
    rule_detail(frame, detail, app);
}

/// The events by category, each category's events wrapped under its label.
fn event_lines(events: &[NotifyEvent], width: usize, rows: usize) -> Vec<Line<'static>> {
    const LABEL: usize = 18;
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    for cat in EventCategory::ALL {
        let mut used = width;
        for e in cat.events().filter(|e| events.contains(e)) {
            let w = e.label().chars().count() + 5;
            if used + w > width {
                let label = if used == width { cat.label() } else { "" };
                lines.push(vec![Span::styled(
                    format!("{label:<LABEL$}"),
                    theme::muted(),
                )]);
                used = LABEL;
            }
            let line = lines.last_mut().expect("a line was pushed");
            line.push(marker(e.severity()));
            line.push(Span::styled(format!(" {}   ", e.label()), theme::text()));
            used += w;
        }
    }
    if rows > 0 && lines.len() > rows {
        let hidden = lines.len() - rows + 1;
        lines.truncate(rows - 1);
        lines.push(vec![Span::styled(
            format!("+{hidden} more lines · ⏎ edit shows them all"),
            theme::muted(),
        )]);
    }
    lines.into_iter().map(Line::from).collect()
}

fn rule_detail(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.notify;
    let Some(r) = view.selected_rule() else {
        frame.render_widget(panel(title("rule", false), None, false), area);
        return;
    };
    let channel = view.channel(&r.channel_id);
    let name = channel.map_or("unknown channel", |c| c.name.as_str());
    let block = panel(
        vec![Span::styled(
            format!("{name} · {}", r.scope_label),
            theme::muted(),
        )],
        Some(Line::styled(
            plural(r.events.len(), "event", "events"),
            theme::muted(),
        )),
        false,
    );
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(4)
        .areas(inner);
    let (preset_id, edited) = preset_label(r.preset.as_deref(), &r.events);
    let preset_note = match r.preset.as_deref().and_then(preset) {
        Some(p) if edited => format!("  {} · edited", p.description.to_lowercase()),
        Some(p) => format!("  {}", p.description.to_lowercase()),
        None => "  events picked one by one".into(),
    };
    let mut scope = scope_spans(&r.scope, &r.scope_label, false);
    scope.push(Span::styled(
        format!("  {}", scope_detail(&r.scope)),
        theme::muted(),
    ));
    let rows: [(&str, Vec<Span<'static>>); 6] = [
        (
            "channel",
            vec![
                Span::styled(name.to_string(), theme::text()),
                Span::styled(
                    format!("  {}", channel.map_or("", |c| c.kind.label())),
                    theme::muted(),
                ),
            ],
        ),
        ("scope", scope),
        (
            "preset",
            vec![
                Span::styled(preset_id, theme::text()),
                Span::styled(preset_note, theme::muted()),
            ],
        ),
        (
            "dedupe",
            vec![
                Span::styled(span(r.dedupe_secs), theme::text()),
                Span::styled("  same event, same subject", theme::muted()),
            ],
        ),
        (
            "rate limit",
            vec![
                Span::styled(format!("{} per hour", r.max_per_hour), theme::text()),
                Span::styled("  the rest are suppressed", theme::muted()),
            ],
        ),
        (
            "state",
            vec![
                enabled_glyph(r.enabled),
                Span::styled(
                    if r.enabled { " enabled" } else { " disabled" },
                    if r.enabled {
                        theme::text()
                    } else {
                        theme::muted()
                    },
                ),
            ],
        ),
    ];
    for (row, (k, v)) in lines_of(left, 6).iter().zip(rows) {
        setting_row(frame, *row, k, v, None);
    }
    frame.render_widget(
        Paragraph::new(event_lines(
            &r.events,
            right.width as usize,
            right.height as usize,
        )),
        right,
    );
}

fn deliveries(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.notify;
    let list = view.deliveries();
    let mut right = vec![format!("last {} attempts", view.deliveries.len())];
    if view.failures_only {
        right.push("failures only".into());
    }
    if let Some(c) = view.channel_filter.as_deref() {
        right.push(format!(
            "{} only",
            view.channel(c).map_or("one channel", |c| c.name.as_str())
        ));
    }
    if let Some(at) = view.loaded_at {
        right.push(format!(
            "refreshed {}",
            util::duration(at.elapsed().as_secs() as i64)
        ));
    }
    let (inner, detail) = list_panel(frame, area, "Delivery log", right.join(" · "), 6);
    if list.is_empty() {
        let text = if view.failures_only {
            "no failed deliveries · "
        } else {
            "nothing sent yet · "
        };
        empty(
            frame,
            inner,
            text,
            "t",
            " on the Channels tab sends a test message",
        );
    } else {
        let rows: Vec<Row> = list
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let selected = i == view.delivery_row;
                let (glyph, style) = status_glyph(d.status);
                Row::new(vec![
                    Cell::from(Span::styled(glyph, style)),
                    Cell::from(Span::styled(
                        time(d.at),
                        if selected {
                            theme::accent()
                        } else {
                            theme::muted()
                        },
                    )),
                    Cell::from(Span::styled(d.channel_name.clone(), theme::text())),
                    Cell::from(match d.event {
                        Some(e) => Span::styled(e.id(), theme::text()),
                        None => Span::styled("test message", theme::muted()),
                    }),
                    Cell::from(Span::styled(d.subject.clone(), theme::muted())),
                    Cell::from(Span::styled(d.status.as_str(), status_style(d.status))),
                    Cell::from(Span::styled(d.attempts.to_string(), theme::text())),
                    Cell::from(delivery_result(d)),
                ])
                .style(row_style(selected))
            })
            .collect();
        let offset = scroll_offset(view.delivery_row, inner.height.saturating_sub(1) as usize);
        row_hits(inner, rows.len(), offset, Hit::Item);
        frame.render_widget(
            Table::new(
                rows.into_iter().skip(offset),
                [
                    Constraint::Length(2),
                    Constraint::Length(10),
                    Constraint::Length(18),
                    Constraint::Length(28),
                    Constraint::Fill(1),
                    Constraint::Length(12),
                    Constraint::Length(6),
                    Constraint::Length(30),
                ],
            )
            .header(
                Row::new([
                    "", "TIME", "CHANNEL", "EVENT", "SUBJECT", "STATUS", "TRIES", "RESULT",
                ])
                .style(theme::muted()),
            )
            .column_spacing(1),
            inner,
        );
    }
    delivery_detail(frame, detail, app);
}

fn delivery_detail(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.notify;
    let Some(d) = view.selected_delivery() else {
        frame.render_widget(panel(title("delivery", false), None, false), area);
        return;
    };
    let block = panel(
        vec![Span::styled(
            format!("{} · {}", time(d.at), d.channel_name),
            theme::muted(),
        )],
        None,
        false,
    );
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(4)
        .areas(inner);
    let rule = d
        .rule_id
        .as_deref()
        .and_then(|id| view.rules.iter().find(|r| r.id == id));
    let rule_spans = match rule {
        Some(r) => {
            let (preset, _) = preset_label(r.preset.as_deref(), &r.events);
            let mut s = scope_spans(&r.scope, &r.scope_label, false);
            s.push(Span::styled(
                format!("  {preset} · {}", plural(r.events.len(), "event", "events")),
                theme::muted(),
            ));
            s
        }
        None if d.rule_id.is_some() => vec![Span::styled("a deleted rule", theme::muted())],
        None => vec![Span::styled("none · a test message", theme::muted())],
    };
    let rows: [(&str, Vec<Span<'static>>); 4] = [
        (
            "event",
            match d.event {
                Some(e) => vec![
                    Span::styled(e.id(), theme::text()),
                    Span::styled(format!("  {}", e.label()), theme::muted()),
                ],
                None => vec![Span::styled("test message", theme::text())],
            },
        ),
        (
            "subject",
            vec![Span::styled(d.subject.clone(), theme::text())],
        ),
        ("title", vec![Span::styled(d.title.clone(), theme::text())]),
        ("rule", rule_spans),
    ];
    for (row, (k, v)) in lines_of(left, 4).iter().zip(rows) {
        setting_row(frame, *row, k, v, None);
    }
    let (glyph, style) = status_glyph(d.status);
    let target = view.channel(&d.channel_id).map(|c| c.target.clone());
    let rows: [(&str, Vec<Span<'static>>); 3] = [
        (
            "status",
            vec![
                Span::styled(format!("{glyph} "), style),
                Span::styled(d.status.as_str(), status_style(d.status)),
                Span::styled(
                    format!(
                        "  after {}",
                        plural(d.attempts as usize, "attempt", "attempts")
                    ),
                    theme::muted(),
                ),
            ],
        ),
        (
            if d.error.is_some() {
                "error"
            } else {
                "latency"
            },
            match (&d.error, d.latency_ms) {
                (Some(e), _) => vec![Span::styled(e.clone(), theme::text())],
                (None, Some(ms)) => vec![Span::styled(format!("{ms}ms"), theme::text())],
                _ => vec![Span::styled("—", theme::muted())],
            },
        ),
        (
            "channel",
            vec![
                Span::styled(d.channel_name.clone(), theme::text()),
                Span::styled(
                    format!("  {}", target.unwrap_or_else(|| "deleted".into())),
                    theme::muted(),
                ),
            ],
        ),
    ];
    for (row, (k, v)) in lines_of(right, 3).iter().zip(rows) {
        setting_row(frame, *row, k, v, None);
    }
}

// The channel wizard

/// Rows of the kind step: label, kinds, hint.
pub const KIND_GROUPS: [(&str, &[ChannelKind], &str); 4] = [
    (
        "email",
        &[
            ChannelKind::EmailSmtp,
            ChannelKind::EmailResend,
            ChannelKind::EmailCloudflare,
        ],
        "server or API",
    ),
    (
        "chat",
        &[
            ChannelKind::Discord,
            ChannelKind::Slack,
            ChannelKind::Teams,
            ChannelKind::Mattermost,
            ChannelKind::GoogleChat,
            ChannelKind::Telegram,
        ],
        "",
    ),
    (
        "push",
        &[
            ChannelKind::Ntfy,
            ChannelKind::Gotify,
            ChannelKind::Pushover,
        ],
        "",
    ),
    ("other", &[ChannelKind::Webhook], "JSON POST to any URL"),
];

fn kind_option(kind: ChannelKind) -> String {
    kind.label().trim_start_matches("email · ").to_string()
}

/// "1 kind › 2 settings › 3 test & save" with the current step lit.
pub fn steps(current: usize, kind: Option<ChannelKind>) -> Line<'static> {
    let first = match kind {
        Some(k) => format!("1 kind · {}", k.label()),
        None => "1 kind".into(),
    };
    let names = [first, "2 settings".into(), "3 test & save".into()];
    let mut spans = Vec::new();
    for (i, name) in names.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ›  ", theme::muted()));
        }
        spans.push(Span::styled(
            name,
            if i + 1 == current {
                theme::text()
            } else {
                theme::muted()
            },
        ));
    }
    Line::from(spans)
}

pub fn kind_form() -> Form {
    let fields = KIND_GROUPS
        .iter()
        .map(|(label, kinds, hint)| {
            Field::choice(
                label,
                label,
                kinds.iter().map(|k| kind_option(*k)).collect(),
            )
            .hint(*hint)
            .quiet()
        })
        .collect();
    Form::new(FormKind::NotifyKind, "Add channel", fields)
        .subtitle("kind")
        .header(steps(1, None))
        .note(vec![
            "Email uses your SMTP server, Resend or Cloudflare; the rest post to a webhook.".into(),
        ])
        .submit("next", None)
}

/// The kind the focused row of the kind step points at.
pub fn chosen_kind(form: &Form) -> ChannelKind {
    let (_, kinds, _) = KIND_GROUPS[form.focus.min(KIND_GROUPS.len() - 1)];
    kinds[form.fields[form.focus].choice.min(kinds.len() - 1)]
}

/// The settings step for a kind, filled from an existing channel when editing. Credentials are
/// never returned by the daemon, so they start empty and stay unchanged unless typed.
pub fn channel_form(kind: ChannelKind, existing: Option<&NotifyChannel>, created: bool) -> Form {
    let editing = existing.is_some() && !created;
    let secret = |key: &'static str, label: &'static str| {
        let f = Field::text(key, label).masked();
        if existing.is_some() {
            f.placeholder("unchanged").hint("type to replace")
        } else {
            f.hint("sealed, never shown")
        }
    };
    let config = existing.map(|c| &c.config);
    // An optional credential the channel has: sent back as shown to keep it.
    let kept = |has: bool| {
        if has {
            REDACTED.to_string()
        } else {
            String::new()
        }
    };
    // The daemon keeps a credential sent back exactly as it showed it.
    let kept_url = match config {
        Some(
            ChannelConfig::Discord { webhook_url }
            | ChannelConfig::Slack { webhook_url }
            | ChannelConfig::Mattermost { webhook_url }
            | ChannelConfig::GoogleChat { webhook_url }
            | ChannelConfig::Teams { webhook_url },
        ) => webhook_url.clone(),
        Some(ChannelConfig::Webhook { url, .. }) => url.clone(),
        _ => String::new(),
    };
    let mut fields = vec![
        Field::text("name", "name")
            .value(existing.map(|c| c.name.clone()).unwrap_or_default())
            .placeholder(match kind {
                k if k.is_email() => "ops-email",
                ChannelKind::Webhook => "pager-hook",
                k => k.as_str(),
            }),
    ];
    let (from, to) = match config {
        Some(ChannelConfig::Email { from, to, .. }) => (from.clone(), to.join(", ")),
        _ => Default::default(),
    };
    match kind {
        ChannelKind::EmailSmtp => {
            let (host, port, security, username) = match config {
                Some(ChannelConfig::Email {
                    provider:
                        EmailProvider::Smtp {
                            host,
                            port,
                            security,
                            username,
                            ..
                        },
                    ..
                }) => (
                    host.clone(),
                    port.to_string(),
                    *security,
                    username.clone().unwrap_or_default(),
                ),
                _ => (
                    String::new(),
                    "587".into(),
                    SmtpSecurity::Starttls,
                    String::new(),
                ),
            };
            fields.extend([
                Field::text("host", "host")
                    .value(host)
                    .placeholder("smtp.example.com"),
                Field::text("port", "port").value(port).hint("465 for tls"),
                Field::choice(
                    "security",
                    "security",
                    SmtpSecurity::ALL
                        .iter()
                        .map(|s| s.as_str().into())
                        .collect(),
                )
                .selected(
                    SmtpSecurity::ALL
                        .iter()
                        .position(|s| *s == security)
                        .unwrap_or(0),
                ),
                Field::text("username", "username")
                    .value(username)
                    .hint("empty for no login"),
                secret("password", "password"),
            ]);
        }
        ChannelKind::EmailResend => fields.push(secret("api_key", "api key")),
        ChannelKind::EmailCloudflare => {
            let account = match config {
                Some(ChannelConfig::Email {
                    provider: EmailProvider::Cloudflare { account_id, .. },
                    ..
                }) => account_id.clone(),
                _ => String::new(),
            };
            fields.extend([
                Field::text("account_id", "account id").value(account),
                secret("api_token", "api token"),
            ]);
        }
        ChannelKind::Discord
        | ChannelKind::Slack
        | ChannelKind::Mattermost
        | ChannelKind::GoogleChat
        | ChannelKind::Teams => {
            let mut f = secret("webhook_url", "webhook url");
            f.shown = kept_url;
            fields.push(match existing {
                Some(c) => f.placeholder(format!("unchanged · {}", c.target)),
                None => f.placeholder("https://…").hint("the URL is the credential"),
            });
        }
        ChannelKind::Telegram => {
            let chat = match config {
                Some(ChannelConfig::Telegram { chat_id, .. }) => chat_id.clone(),
                _ => String::new(),
            };
            fields.extend([
                secret("bot_token", "bot token"),
                Field::text("chat_id", "chat id")
                    .value(chat)
                    .hint("from @userinfobot"),
            ]);
        }
        ChannelKind::Ntfy => {
            let (server, topic) = match config {
                Some(ChannelConfig::Ntfy { server, topic, .. }) => (server.clone(), topic.clone()),
                _ => ("https://ntfy.sh".into(), String::new()),
            };
            fields.extend([
                Field::text("server", "server").value(server),
                Field::text("topic", "topic")
                    .value(topic)
                    .placeholder("ops-alerts"),
                Field {
                    shown: kept(matches!(
                        config,
                        Some(ChannelConfig::Ntfy { token: Some(_), .. })
                    )),
                    ..secret("token", "access token").hint("optional")
                },
            ]);
        }
        ChannelKind::Gotify => {
            let server = match config {
                Some(ChannelConfig::Gotify { server, .. }) => server.clone(),
                _ => String::new(),
            };
            fields.extend([
                Field::text("server", "server")
                    .value(server)
                    .placeholder("https://gotify.example.com"),
                secret("app_token", "app token"),
            ]);
        }
        ChannelKind::Pushover => fields.extend([
            secret("user_key", "user key"),
            secret("app_token", "app token"),
        ]),
        ChannelKind::Webhook => {
            let mut url = match existing {
                Some(c) => Field::text("url", "url")
                    .placeholder(format!("unchanged · {}", c.target))
                    .hint("type to replace"),
                None => Field::text("url", "url").placeholder("https://"),
            };
            url.shown = kept_url;
            let signing = Field {
                shown: kept(matches!(
                    config,
                    Some(ChannelConfig::Webhook {
                        secret: Some(_),
                        ..
                    })
                )),
                ..secret("secret", "signing secret").hint("optional")
            };
            fields.extend([url, signing]);
        }
    }
    if kind.is_email() {
        fields.extend([
            Field::text("from", "from")
                .value(from)
                .placeholder("alerts@example.com"),
            Field::text("to", "to")
                .value(to)
                .placeholder("ops@example.com")
                .hint("comma-separated"),
        ]);
    }
    let note = if editing {
        "Credentials you leave empty stay as they are. A test is sent before saving."
    } else {
        "Credentials are sealed with the master key. Next, a test before saving."
    };
    Form::new(
        FormKind::NotifyChannel {
            kind,
            channel_id: existing.map(|c| c.id.clone()),
            created,
        },
        if editing {
            "Edit channel"
        } else {
            "Add channel"
        },
        fields,
    )
    .subtitle(kind.label())
    .header(steps(2, Some(kind)))
    .note(vec![note.into()])
    .submit("next: test", None)
}

/// The name and config the settings step describes. Empty credentials of an existing channel are
/// sent back the way the daemon showed them (REDACTED, or the cut-down URL), which it keeps.
pub fn channel_config(form: &Form) -> Result<(String, ChannelConfig), String> {
    let FormKind::NotifyChannel {
        kind, channel_id, ..
    } = &form.kind
    else {
        return Err("not a channel form".into());
    };
    let name = form.get("name");
    common::validate_name(&name)?;
    let keep = channel_id.is_some();
    let shown = |key: &str| {
        form.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.shown.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| REDACTED.into())
    };
    let secret = |key: &str, label: &str| -> Result<String, String> {
        match form.get(key) {
            v if !v.is_empty() => Ok(v),
            _ if keep => Ok(shown(key)),
            _ => Err(format!("{label} is required")),
        }
    };
    let optional = |key: &str| -> Option<String> {
        let kept = form
            .fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.shown.clone())
            .filter(|s| keep && !s.is_empty());
        Some(form.get(key)).filter(|v| !v.is_empty()).or(kept)
    };
    let to: Vec<String> = form
        .get("to")
        .split([',', ' ', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    let email = |provider: EmailProvider| ChannelConfig::Email {
        provider,
        from: form.get("from"),
        to: to.clone(),
    };
    let webhook = || secret("webhook_url", "webhook url");
    let config = match kind {
        ChannelKind::EmailSmtp => {
            let port = form
                .get("port")
                .parse::<u16>()
                .map_err(|_| "port must be a number from 1 to 65535".to_string())?;
            let username = Some(form.get("username")).filter(|u| !u.is_empty());
            let password = match username {
                Some(_) => Some(secret("password", "password")?),
                None => None,
            };
            email(EmailProvider::Smtp {
                host: form.get("host"),
                port,
                security: SmtpSecurity::ALL[form.choice("security").min(2)],
                username,
                password,
            })
        }
        ChannelKind::EmailResend => email(EmailProvider::Resend {
            api_key: secret("api_key", "api key")?,
        }),
        ChannelKind::EmailCloudflare => email(EmailProvider::Cloudflare {
            account_id: form.get("account_id"),
            api_token: secret("api_token", "api token")?,
        }),
        ChannelKind::Discord => ChannelConfig::Discord {
            webhook_url: webhook()?,
        },
        ChannelKind::Slack => ChannelConfig::Slack {
            webhook_url: webhook()?,
        },
        ChannelKind::Mattermost => ChannelConfig::Mattermost {
            webhook_url: webhook()?,
        },
        ChannelKind::GoogleChat => ChannelConfig::GoogleChat {
            webhook_url: webhook()?,
        },
        ChannelKind::Teams => ChannelConfig::Teams {
            webhook_url: webhook()?,
        },
        ChannelKind::Telegram => ChannelConfig::Telegram {
            bot_token: secret("bot_token", "bot token")?,
            chat_id: form.get("chat_id"),
        },
        ChannelKind::Ntfy => ChannelConfig::Ntfy {
            server: form.get("server").trim_end_matches('/').to_string(),
            topic: form.get("topic"),
            token: optional("token"),
        },
        ChannelKind::Gotify => ChannelConfig::Gotify {
            server: form.get("server").trim_end_matches('/').to_string(),
            app_token: secret("app_token", "app token")?,
        },
        ChannelKind::Pushover => ChannelConfig::Pushover {
            user_key: secret("user_key", "user key")?,
            app_token: secret("app_token", "app token")?,
        },
        ChannelKind::Webhook => ChannelConfig::Webhook {
            url: secret("url", "url")?,
            secret: optional("secret"),
            headers: Vec::new(),
        },
    };
    config.validate()?;
    Ok((name, config))
}

/// The wizard's last step: the channel exists (disabled while new) and a test was sent.
pub struct ChannelTestStep {
    pub channel: NotifyChannel,
    /// The settings step, for ⇧tab.
    pub form: Box<Form>,
    /// Made by this wizard: cancel deletes it and save enables it.
    pub created: bool,
    /// None while the test message is on its way.
    pub result: Option<Result<NotifyDelivery, String>>,
    pub saving: bool,
}

pub fn render_test_step(frame: &mut Frame, area: Rect, step: &ChannelTestStep, tick: usize) {
    let c = &step.channel;
    let width = 104u16.min(area.width.saturating_sub(4));
    let note = match &step.result {
        None => "Waiting for the provider to accept the message.".to_string(),
        Some(Ok(d)) if d.status == DeliveryStatus::Sent => format!(
            "Look for “{}” at {}. After saving, add a rule on the Rules tab to choose which events reach it.",
            d.title, c.target
        ),
        Some(_) => "Fix the settings with ⇧tab, or save anyway and fix it later; failed deliveries show in the delivery log.".to_string(),
    };
    let note = Paragraph::new(note)
        .style(theme::muted())
        .wrap(Wrap { trim: true });
    let note_h = note.line_count(width.saturating_sub(6)) as u16;
    let heading = vec![
        Span::styled(
            if step.created {
                "Add channel"
            } else {
                "Edit channel"
            },
            theme::text(),
        ),
        Span::styled(format!(" · {}", c.kind.label()), theme::muted()),
    ];
    let inner = dialog(frame, area, width, note_h + 12, heading);
    let [head, _, rows, _, note_a, _, rule, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(note_h),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(steps(3, Some(c.kind))), head);
    let rows = lines_of(rows, 3);
    let label = |t: &str, focus: bool| {
        let t = if focus {
            format!("› {t}")
        } else {
            t.to_string()
        };
        Span::styled(
            format!("{t:<16}"),
            if focus {
                theme::accent()
            } else {
                theme::muted()
            },
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            label("name", false),
            Span::styled(c.name.clone(), theme::text()),
        ])),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            label("sends to", false),
            Span::styled(c.target.clone(), theme::text()),
        ])),
        rows[1],
    );
    let result: Vec<Span<'static>> = match &step.result {
        None => vec![Span::styled(
            format!("{} sending a test message…", util::spinner(tick)),
            theme::fg(theme::INFO),
        )],
        Some(Ok(d)) if d.status == DeliveryStatus::Sent => vec![
            Span::styled("● ", theme::fg(theme::OK)),
            Span::styled("delivered", theme::text()),
            Span::styled(
                format!(
                    "  in {}ms · {}",
                    d.latency_ms.unwrap_or_default(),
                    util::ago(d.at)
                ),
                theme::muted(),
            ),
        ],
        Some(Ok(d)) => vec![
            Span::styled("✕ ", theme::fg(theme::ERR)),
            Span::styled(d.status.as_str(), theme::fg(theme::ERR)),
            Span::styled(
                format!("  {}", d.error.as_deref().unwrap_or("no reason given")),
                theme::text(),
            ),
        ],
        Some(Err(e)) => vec![
            Span::styled("✕ ", theme::fg(theme::ERR)),
            Span::styled(e.clone(), theme::text()),
        ],
    };
    let again = [("t", "send again")];
    let hint_w = mouse::hints_width(&again) as u16;
    let mut spans = vec![label("test", true)];
    spans.extend(result);
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            width: rows[2].width.saturating_sub(hint_w + 1),
            ..rows[2]
        },
    );
    if step.result.is_some() && !step.saving {
        mouse::hints(frame, rows[2], &again, Alignment::Right);
    }
    frame.render_widget(note, note_a);
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    let chip = if step.saving {
        format!(" {} saving… ", util::spinner(tick))
    } else {
        " ⏎ save ".to_string()
    };
    let chip = Span::styled(chip, theme::chip());
    let chip_w = (chip.width() as u16).min(keys.width);
    let chip_area = Rect::new(keys.right() - chip_w, keys.y, chip_w, 1);
    frame.render_widget(Paragraph::new(chip), chip_area);
    if !step.saving {
        mouse::key_target(chip_area, "⏎");
        mouse::hints(
            frame,
            Rect {
                width: keys.width.saturating_sub(chip_w + 1),
                ..keys
            },
            &[("t", "send test"), ("⇧tab", "back"), ("esc", "cancel")],
            Alignment::Left,
        );
    }
}

// The rule dialog

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleFocus {
    Channel,
    Scope,
    Dedupe,
    Rate,
    Preset,
    Events,
}

impl RuleFocus {
    const ORDER: [RuleFocus; 6] = [
        RuleFocus::Channel,
        RuleFocus::Scope,
        RuleFocus::Dedupe,
        RuleFocus::Rate,
        RuleFocus::Preset,
        RuleFocus::Events,
    ];

    fn step(self, step: isize) -> Self {
        let i = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0) as isize;
        Self::ORDER[(i + step).rem_euclid(Self::ORDER.len() as isize) as usize]
    }
}

const DEDUPE: [u32; 5] = [60, 300, 600, 1800, 3600];
const RATE: [u32; 5] = [5, 10, 30, 60, 120];

/// The event checklist's three columns.
pub const COLUMNS: [&[EventCategory]; 3] = [
    &[EventCategory::Deploys, EventCategory::Services],
    &[
        EventCategory::Databases,
        EventCategory::Backups,
        EventCategory::Storage,
        EventCategory::Domains,
    ],
    &[
        EventCategory::Servers,
        EventCategory::System,
        EventCategory::Security,
    ],
];

fn column_events(col: usize) -> Vec<NotifyEvent> {
    COLUMNS[col].iter().flat_map(|c| c.events()).collect()
}

fn with_value(options: &[u32], value: u32) -> Vec<u32> {
    let mut v = options.to_vec();
    if !v.contains(&value) {
        v.push(value);
        v.sort_unstable();
    }
    v
}

pub enum RuleAction {
    None,
    Cancel,
    Save,
    Pick(RuleFocus),
}

pub struct RuleDialog {
    pub rule_id: Option<Id>,
    pub channel_id: Id,
    pub channel: String,
    pub channel_detail: String,
    pub scope: RuleScope,
    pub scope_label: String,
    pub dedupe: u32,
    pub rate: u32,
    /// Where the event set started; None for custom.
    pub preset: Option<&'static str>,
    pub events: BTreeSet<NotifyEvent>,
    pub focus: RuleFocus,
    /// Column and row in the event checklist.
    pub cursor: (usize, usize),
    pub busy: bool,
    pub error: Option<String>,
}

impl RuleDialog {
    pub fn new(channel: &NotifyChannel) -> Self {
        let essentials = &PRESETS[0];
        Self {
            rule_id: None,
            channel_id: channel.id.clone(),
            channel: channel.name.clone(),
            channel_detail: channel.kind.label().to_string(),
            scope: RuleScope::Instance,
            scope_label: "instance".into(),
            dedupe: DEFAULT_DEDUPE_SECS,
            rate: DEFAULT_MAX_PER_HOUR,
            preset: Some(essentials.id),
            events: essentials.events().into_iter().collect(),
            focus: RuleFocus::Channel,
            cursor: (0, 0),
            busy: false,
            error: None,
        }
    }

    pub fn edit(rule: &NotifyRule, channel: Option<&NotifyChannel>) -> Self {
        Self {
            rule_id: Some(rule.id.clone()),
            channel_id: rule.channel_id.clone(),
            channel: channel.map_or("unknown channel".into(), |c| c.name.clone()),
            channel_detail: channel.map_or(String::new(), |c| c.kind.label().to_string()),
            scope: rule.scope.clone(),
            scope_label: rule.scope_label.clone(),
            dedupe: rule.dedupe_secs,
            rate: rule.max_per_hour,
            preset: rule.preset.as_deref().and_then(preset).map(|p| p.id),
            events: rule.events.iter().copied().collect(),
            focus: RuleFocus::Preset,
            cursor: (0, 0),
            busy: false,
            error: None,
        }
    }

    /// Row of the preset list: the presets, then custom.
    pub fn preset_row(&self) -> usize {
        self.preset
            .and_then(|id| PRESETS.iter().position(|p| p.id == id))
            .unwrap_or(PRESETS.len())
    }

    pub fn apply_preset(&mut self, row: usize) {
        match PRESETS.get(row) {
            Some(p) => {
                self.preset = Some(p.id);
                self.events = p.events().into_iter().collect();
            }
            None => self.preset = None,
        }
        self.error = None;
    }

    pub fn toggle(&mut self, event: NotifyEvent) {
        if !self.events.remove(&event) {
            self.events.insert(event);
        }
        self.error = None;
    }

    fn cursor_event(&self) -> Option<NotifyEvent> {
        column_events(self.cursor.0).get(self.cursor.1).copied()
    }

    pub fn click_field(&mut self, focus: RuleFocus) {
        if !self.busy {
            self.focus = focus;
        }
    }

    pub fn click_event(&mut self, event: NotifyEvent) {
        if self.busy {
            return;
        }
        for col in 0..COLUMNS.len() {
            if let Some(row) = column_events(col).iter().position(|e| *e == event) {
                self.cursor = (col, row);
            }
        }
        self.focus = RuleFocus::Events;
        self.toggle(event);
    }

    pub fn click_preset(&mut self, row: usize) {
        if self.busy {
            return;
        }
        self.focus = RuleFocus::Preset;
        self.apply_preset(row);
    }

    pub fn click_choice(&mut self, focus: RuleFocus, i: usize) {
        if self.busy {
            return;
        }
        self.focus = focus;
        match focus {
            RuleFocus::Dedupe => {
                if let Some(v) = with_value(&DEDUPE, self.dedupe).get(i) {
                    self.dedupe = *v;
                }
            }
            RuleFocus::Rate => {
                if let Some(v) = with_value(&RATE, self.rate).get(i) {
                    self.rate = *v;
                }
            }
            _ => {}
        }
    }

    fn cycle(&mut self, step: isize) {
        let (options, value) = match self.focus {
            RuleFocus::Dedupe => (with_value(&DEDUPE, self.dedupe), &mut self.dedupe),
            RuleFocus::Rate => (with_value(&RATE, self.rate), &mut self.rate),
            _ => return,
        };
        let i = options.iter().position(|v| v == value).unwrap_or(0) as isize;
        *value = options[(i + step).rem_euclid(options.len() as isize) as usize];
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> RuleAction {
        if self.busy {
            return RuleAction::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (self.focus, key.code) {
            (_, KeyCode::Esc) => return RuleAction::Cancel,
            (_, KeyCode::Char('s')) if ctrl => return RuleAction::Save,
            (_, KeyCode::Tab) => self.focus = self.focus.step(1),
            (_, KeyCode::BackTab) => self.focus = self.focus.step(-1),
            (RuleFocus::Channel | RuleFocus::Scope, KeyCode::Enter | KeyCode::Char(' ')) => {
                return RuleAction::Pick(self.focus);
            }
            (_, KeyCode::Enter) => return RuleAction::Save,
            (RuleFocus::Preset, KeyCode::Up | KeyCode::Char('k')) => match self.preset_row() {
                0 => self.focus = RuleFocus::Rate,
                row => self.apply_preset(row - 1),
            },
            (RuleFocus::Preset, KeyCode::Down | KeyCode::Char('j')) => match self.preset_row() {
                row if row == PRESETS.len() => self.focus = RuleFocus::Events,
                row => self.apply_preset(row + 1),
            },
            (RuleFocus::Preset, KeyCode::Right | KeyCode::Char('l')) => {
                self.focus = RuleFocus::Events
            }
            (RuleFocus::Events, KeyCode::Up | KeyCode::Char('k')) => {
                self.cursor.1 = self.cursor.1.saturating_sub(1);
            }
            (RuleFocus::Events, KeyCode::Down | KeyCode::Char('j')) => self.cursor.1 += 1,
            (RuleFocus::Events, KeyCode::Left | KeyCode::Char('h')) => match self.cursor.0 {
                0 => self.focus = RuleFocus::Preset,
                col => self.cursor.0 = col - 1,
            },
            (RuleFocus::Events, KeyCode::Right | KeyCode::Char('l')) => {
                self.cursor.0 = (self.cursor.0 + 1).min(COLUMNS.len() - 1);
            }
            (RuleFocus::Events, KeyCode::Char(' ' | 'x')) => {
                if let Some(e) = self.cursor_event() {
                    self.toggle(e);
                }
            }
            (RuleFocus::Dedupe | RuleFocus::Rate, KeyCode::Left) => self.cycle(-1),
            (RuleFocus::Dedupe | RuleFocus::Rate, KeyCode::Right | KeyCode::Char(' ')) => {
                self.cycle(1)
            }
            (_, KeyCode::Down) => self.focus = self.focus.step(1),
            (_, KeyCode::Up) => self.focus = self.focus.step(-1),
            _ => {}
        }
        let n = column_events(self.cursor.0).len();
        self.cursor.1 = self.cursor.1.min(n.saturating_sub(1));
        RuleAction::None
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, tick: usize) {
        let heading = vec![
            Span::styled(
                if self.rule_id.is_some() {
                    "Edit rule"
                } else {
                    "Add rule"
                },
                theme::text(),
            ),
            Span::styled(
                format!(" · {} → {}", self.channel, self.scope_label),
                theme::muted(),
            ),
        ];
        let inner = dialog(frame, area, 150, 33, heading);
        if inner.height < 8 {
            return;
        }
        let error_h = u16::from(self.error.is_some());
        let [fields, _, lists, error, rule, keys] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(error_h),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        mouse::hit(
            Rect {
                height: inner.height.saturating_sub(2),
                ..inner
            },
            Hit::List(List::Rule),
        );
        self.render_fields(frame, fields);
        self.render_lists(frame, lists);
        if let Some(e) = &self.error {
            frame.render_widget(
                Paragraph::new(format!("✕ {e}")).style(theme::fg(theme::ERR)),
                error,
            );
        }
        frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
        let chip = if self.busy {
            format!(" {} saving… ", util::spinner(tick))
        } else if self.rule_id.is_some() {
            " ⏎ save rule ".to_string()
        } else {
            " ⏎ add rule ".to_string()
        };
        let chip = Span::styled(chip, theme::chip());
        let chip_w = (chip.width() as u16).min(keys.width);
        let chip_area = Rect::new(keys.right() - chip_w, keys.y, chip_w, 1);
        frame.render_widget(Paragraph::new(chip), chip_area);
        if self.busy {
            mouse::hit(inner, Hit::Block);
            return;
        }
        mouse::key_target(chip_area, "⏎");
        let hints: &[(&str, &str)] = if self.focus == RuleFocus::Events {
            &[
                ("tab", "next"),
                ("⇧tab", "prev"),
                ("space", "toggle"),
                ("esc", "cancel"),
            ]
        } else {
            &[("tab", "next"), ("⇧tab", "prev"), ("esc", "cancel")]
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

    fn render_fields(&self, frame: &mut Frame, area: Rect) {
        let rows = lines_of(area, 4);
        for (row, focus) in rows.iter().zip([
            RuleFocus::Channel,
            RuleFocus::Scope,
            RuleFocus::Dedupe,
            RuleFocus::Rate,
        ]) {
            if row.height == 0 {
                continue;
            }
            let focused = self.focus == focus;
            let label = match focus {
                RuleFocus::Channel => "channel",
                RuleFocus::Scope => "scope",
                RuleFocus::Dedupe => "dedupe",
                _ => "rate limit",
            };
            let [label_a, value_a, hint_a] = Layout::horizontal([
                Constraint::Length(16),
                Constraint::Fill(1),
                Constraint::Length(28),
            ])
            .areas(*row);
            mouse::hit(*row, Hit::RuleField(focus));
            let (label, style) = if focused {
                (format!("› {label}"), theme::accent())
            } else {
                (label.to_string(), theme::muted())
            };
            frame.render_widget(Paragraph::new(label).style(style), label_a);
            let hint = match focus {
                RuleFocus::Channel | RuleFocus::Scope => {
                    let (shown, detail) = if focus == RuleFocus::Channel {
                        (
                            vec![Span::styled(self.channel.clone(), theme::text())],
                            self.channel_detail.clone(),
                        )
                    } else {
                        (
                            scope_spans(&self.scope, &self.scope_label, false),
                            scope_detail(&self.scope).to_string(),
                        )
                    };
                    let mut spans = vec![Span::raw(" ")];
                    spans.extend(shown);
                    spans.push(Span::styled(format!("  {detail}"), theme::muted()));
                    frame.render_widget(Paragraph::new(Line::from(spans)), value_a);
                    let pick = if focused {
                        theme::accent()
                    } else {
                        theme::muted()
                    };
                    frame.render_widget(
                        Paragraph::new(Span::styled("⏎ pick", pick)).right_aligned(),
                        value_a,
                    );
                    if focus == RuleFocus::Scope {
                        "only events inside it"
                    } else {
                        ""
                    }
                }
                _ => {
                    let dedupe = focus == RuleFocus::Dedupe;
                    let (options, value) = if dedupe {
                        (with_value(&DEDUPE, self.dedupe), self.dedupe)
                    } else {
                        (with_value(&RATE, self.rate), self.rate)
                    };
                    let mut x = value_a.x + 1;
                    let mut spans = vec![Span::raw(" ")];
                    for (j, v) in options.iter().enumerate() {
                        let text = if dedupe { span(*v) } else { format!("{v}/h") };
                        let style = match (*v == value, focused) {
                            (true, true) => theme::chip(),
                            (true, false) => theme::text().bg(theme::LINE),
                            _ => theme::muted(),
                        };
                        let chip = Span::styled(format!(" {text} "), style);
                        let w = chip.width() as u16;
                        if x + w > value_a.right() {
                            break;
                        }
                        mouse::hit(Rect::new(x, row.y, w, 1), Hit::RuleChoice(focus, j));
                        x += w + 1;
                        spans.push(chip);
                        spans.push(Span::raw(" "));
                    }
                    frame.render_widget(Paragraph::new(Line::from(spans)), value_a);
                    if dedupe {
                        "same event, same subject"
                    } else {
                        "the rest are suppressed"
                    }
                }
            };
            frame.render_widget(
                Paragraph::new(format!("  {hint}")).style(theme::muted()),
                hint_a,
            );
        }
    }

    fn render_lists(&self, frame: &mut Frame, area: Rect) {
        let [presets, divider, events] = Layout::horizontal([
            Constraint::Length(32),
            Constraint::Length(3),
            Constraint::Fill(1),
        ])
        .areas(area);
        let lines: Vec<Line> = (0..divider.height)
            .map(|_| Line::styled(" │ ", theme::fg(theme::LINE)))
            .collect();
        frame.render_widget(Paragraph::new(lines), divider);

        frame.render_widget(Paragraph::new("preset").style(theme::muted()), presets);
        let chosen = self.preset_row();
        let focused = self.focus == RuleFocus::Preset;
        let entries = PRESETS
            .iter()
            .map(|p| (p.label, p.description))
            .chain(std::iter::once(("Custom", "pick events yourself")));
        let mut y = presets.y + 2;
        for (i, (label, description)) in entries.enumerate() {
            if y + 2 > presets.bottom() {
                break;
            }
            let on = i == chosen;
            let rect = Rect::new(presets.x, y, presets.width, 2);
            mouse::hit(rect, Hit::Preset(i));
            let label_style = match (on, focused) {
                (true, true) => theme::accent(),
                _ => theme::text(),
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(
                            if on { " ● " } else { " ○ " },
                            if on { theme::text() } else { theme::muted() },
                        ),
                        Span::styled(label, label_style),
                    ]),
                    Line::styled(format!("   {}", description.to_lowercase()), theme::muted()),
                ])
                .style(if on {
                    Style::new().bg(theme::LINE)
                } else {
                    Style::new()
                }),
                rect,
            );
            y += 3;
        }

        let legend = Line::from(vec![
            Span::styled(
                format!("{} of {} · ", self.events.len(), NotifyEvent::ALL.len()),
                theme::muted(),
            ),
            marker(Severity::Critical),
            Span::styled(" critical  ", theme::muted()),
            marker(Severity::Warning),
            Span::styled(" warning  ", theme::muted()),
            marker(Severity::Success),
            Span::styled(" success  ", theme::muted()),
            marker(Severity::Info),
            Span::styled(" info", theme::muted()),
        ]);
        let [head, _, cols] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(events);
        frame.render_widget(Paragraph::new("events").style(theme::muted()), head);
        frame.render_widget(Paragraph::new(legend).right_aligned(), head);
        let columns = Layout::horizontal([Constraint::Fill(1); 3])
            .spacing(3)
            .split(cols);
        for (col, area) in columns.iter().enumerate() {
            let mut y = area.y;
            let mut row = 0;
            for cat in COLUMNS[col] {
                if y >= area.bottom() {
                    break;
                }
                let list: Vec<NotifyEvent> = cat.events().collect();
                let on = list.iter().filter(|e| self.events.contains(e)).count();
                let count = format!("{on}/{}", list.len());
                let title_w = (area.width as usize).saturating_sub(count.len() + 1);
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            format!("{:<title_w$} ", truncate(cat.label(), title_w)),
                            theme::muted(),
                        ),
                        Span::styled(count, theme::muted()),
                    ])),
                    Rect::new(area.x, y, area.width, 1),
                );
                y += 1;
                for e in list {
                    if y >= area.bottom() {
                        break;
                    }
                    let checked = self.events.contains(&e);
                    let cursor = self.focus == RuleFocus::Events && self.cursor == (col, row);
                    let style = match (cursor, checked) {
                        (true, _) => theme::accent(),
                        (false, true) => theme::text(),
                        _ => theme::muted(),
                    };
                    let rect = Rect::new(area.x, y, area.width, 1);
                    mouse::hit(rect, Hit::NotifyEvent(e));
                    let label = truncate(e.label(), (area.width as usize).saturating_sub(6));
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(if checked { "[x] " } else { "[ ] " }, style),
                            marker(e.severity()),
                            Span::styled(format!(" {label}"), style),
                        ]))
                        .style(if cursor {
                            Style::new().bg(theme::LINE)
                        } else {
                            Style::new()
                        }),
                        rect,
                    );
                    y += 1;
                    row += 1;
                }
                y += 1;
            }
        }
    }
}

// Pickers for the rule dialog

/// Scope picker ids: "instance", "project:<id>", "environment:<id>", "service:<id>".
pub fn scope_items(tree: Option<&Tree>) -> Vec<PickItem> {
    let mut items = vec![PickItem::new(
        Some("instance".into()),
        "instance",
        scope_detail(&RuleScope::Instance),
    )];
    for p in tree.map(|t| t.projects.as_slice()).unwrap_or_default() {
        items.push(PickItem::new(
            Some(format!("project:{}", p.project.id)),
            p.project.name.clone(),
            "project · every environment and service in it",
        ));
        for e in &p.environments {
            items.push(PickItem::new(
                Some(format!("environment:{}", e.environment.id)),
                format!("{}/{}", p.project.name, e.environment.name),
                "environment · every service in it",
            ));
            for s in &e.services {
                items.push(PickItem::new(
                    Some(format!("service:{}", s.id)),
                    format!("{}/{}/{}", p.project.name, e.environment.name, s.name),
                    "service",
                ));
            }
        }
    }
    items
}

pub fn scope_id(scope: &RuleScope) -> String {
    match scope.id() {
        Some(id) => format!("{}:{id}", scope.kind()),
        None => "instance".into(),
    }
}

pub fn parse_scope(id: &str) -> RuleScope {
    match id.split_once(':') {
        Some(("project", id)) => RuleScope::Project { id: id.into() },
        Some(("environment", id)) => RuleScope::Environment { id: id.into() },
        Some(("service", id)) => RuleScope::Service { id: id.into() },
        _ => RuleScope::Instance,
    }
}

pub fn channel_items(channels: &[NotifyChannel]) -> Vec<PickItem> {
    channels
        .iter()
        .map(|c| {
            let mut item = PickItem::new(
                Some(c.id.clone()),
                c.name.clone(),
                format!("{} · {}", c.kind.label(), c.target),
            );
            if !c.enabled {
                item.note = "disabled".into();
            }
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webhook_channel() -> NotifyChannel {
        NotifyChannel {
            id: "c".into(),
            name: "ops".into(),
            kind: ChannelKind::Webhook,
            enabled: true,
            config: ChannelConfig::Webhook {
                url: "https://hooks.example.com/x".into(),
                secret: None,
                headers: Vec::new(),
            },
            target: "hooks.example.com/…".into(),
            created_at: Utc::now(),
            last_test: None,
        }
    }

    #[test]
    fn presets_fill_the_checklist_and_toggling_keeps_the_starting_point() {
        let mut d = RuleDialog::new(&webhook_channel());
        assert_eq!(d.preset, Some("essentials"));
        assert!(d.events.iter().all(|e| e.severity() == Severity::Critical));
        d.focus = RuleFocus::Preset;
        d.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(d.preset, Some("deploys"));
        assert!(
            d.events
                .iter()
                .all(|e| e.category() == EventCategory::Deploys)
        );
        d.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(d.focus, RuleFocus::Events);
        let first = column_events(0)[0];
        assert!(d.events.contains(&first));
        d.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(!d.events.contains(&first));
        assert_eq!(d.preset, Some("deploys"));
        let events: Vec<_> = d.events.iter().copied().collect();
        assert_eq!(preset_label(d.preset, &events), ("deploys".into(), true));
    }

    #[test]
    fn scope_ids_round_trip() {
        for scope in [
            RuleScope::Instance,
            RuleScope::Project { id: "p".into() },
            RuleScope::Environment { id: "e".into() },
            RuleScope::Service { id: "s".into() },
        ] {
            assert_eq!(parse_scope(&scope_id(&scope)), scope);
        }
    }

    #[test]
    fn smtp_form_builds_a_valid_config_and_editing_keeps_credentials() {
        let mut form = channel_form(ChannelKind::EmailSmtp, None, false);
        for (key, value) in [
            ("name", "ops-email"),
            ("host", "smtp.example.com"),
            ("username", "ops@example.com"),
            ("password", "hunter2"),
            ("from", "alerts@example.com"),
            ("to", "ops@example.com, oncall@example.com"),
        ] {
            form.field_mut(key).unwrap().value = value.into();
        }
        let (name, config) = channel_config(&form).unwrap();
        assert_eq!(name, "ops-email");
        let ChannelConfig::Email { provider, to, .. } = &config else {
            panic!("expected email")
        };
        assert_eq!(to.len(), 2);
        assert!(matches!(provider, EmailProvider::Smtp { port: 587, .. }));

        form.field_mut("password").unwrap().value.clear();
        assert!(channel_config(&form).is_err());
        form.kind = FormKind::NotifyChannel {
            kind: ChannelKind::EmailSmtp,
            channel_id: Some("c".into()),
            created: false,
        };
        let (_, config) = channel_config(&form).unwrap();
        let ChannelConfig::Email {
            provider: EmailProvider::Smtp { password, .. },
            ..
        } = config
        else {
            panic!("expected smtp")
        };
        assert_eq!(password.as_deref(), Some(REDACTED));
    }

    #[test]
    fn kind_step_picks_the_focused_rows_choice() {
        let mut form = kind_form();
        assert_eq!(chosen_kind(&form), ChannelKind::EmailSmtp);
        form.handle_key(KeyEvent::from(KeyCode::Tab));
        form.handle_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(chosen_kind(&form), ChannelKind::Slack);
        form.handle_key(KeyEvent::from(KeyCode::Tab));
        form.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(chosen_kind(&form), ChannelKind::Webhook);
    }
}
