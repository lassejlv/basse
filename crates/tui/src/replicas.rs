//! Replicas and autoscaling: the Replicas panel of the Overview, the Scaling panel of the
//! Settings tab, the scale dialog and the autoscale form, following the "SVC-17" to "SVC-22"
//! artboards in Paper.

use crate::app::{App, Msg, Overlay, ServiceView, Tab};
use crate::form::{Field, Form, FormKind};
use crate::mouse::{self, Hit, List};
use crate::theme;
use crate::util::{self, truncate};
use crate::view::{action_chip, dialog, inside, panel, rule_line, title};
use common::api::UpdateService;
use common::{
    Autoscale, BuildMethod, Id, LbPolicy, MAX_REPLICAS, Replica, ReplicaHealth, ScaleCapacity,
    Service, Source,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Columns of the Replicas table: heading and width; the last one takes the rest.
const COLUMNS: [(&str, usize); 8] = [
    ("#", 6),
    ("STATE", 16),
    ("HEALTH", 14),
    ("CPU", 9),
    ("MEM", 11),
    ("RESTARTS", 13),
    ("UPTIME", 10),
    ("CONTAINER", 0),
];

/// Rows of the Scaling panel, in order.
pub const SCALING_ROWS: [&str; 5] = [
    "replicas",
    "load balancing",
    "rolling surge",
    "autoscale",
    "shared volume",
];

const SCALING_KEY_W: usize = 16;

/// A Scaling panel row: its value and the key hint after it.
type ScalingRow = (Vec<Span<'static>>, Option<(char, &'static str)>);

const AUTOSCALE_NOTE: &str =
    "Scales on the average over the last minute. Never beyond the server's memory headroom.";

/// Whether a service can run more than one replica; databases and compose builds cannot.
pub fn scalable(service: &Service) -> bool {
    match &service.source {
        Source::Database { .. } => false,
        Source::Git { build, .. } => build.method != BuildMethod::Compose,
        Source::Image { .. } => true,
    }
}

/// Why a service has no scaling, for the one line the Scaling panel shows instead.
fn unsupported(service: &Service) -> &'static str {
    match service.source {
        Source::Database { .. } => "databases run one replica · scaling is not supported",
        _ => "compose services run one replica · scaling is not supported",
    }
}

/// Rows the Replicas panel takes: borders, the heading, one per replica, a gap and the note.
pub fn panel_height(view: &ServiceView) -> u16 {
    view.replicas().len() as u16 + 5
}

fn replica_state(r: &Replica) -> (Span<'static>, Span<'static>) {
    let Some(c) = &r.container else {
        return (
            Span::styled("○", theme::muted()),
            Span::styled("not created", theme::muted()),
        );
    };
    let failing = c.state == "restarting" || c.exit_code.is_some_and(|e| e != 0);
    let style = match c.state.as_str() {
        _ if failing => theme::fg(theme::ERR),
        "running" => theme::fg(theme::OK),
        _ => theme::muted(),
    };
    let glyph = if c.state == "running" || failing {
        "●"
    } else {
        "○"
    };
    (
        Span::styled(glyph, style),
        Span::styled(c.state.clone(), style),
    )
}

fn health(h: ReplicaHealth) -> Span<'static> {
    let style = match h {
        ReplicaHealth::Healthy => theme::fg(theme::OK),
        ReplicaHealth::Unhealthy => theme::fg(theme::ERR),
        ReplicaHealth::Down => theme::muted(),
    };
    Span::styled(h.as_str(), style)
}

fn uptime(r: &Replica) -> Option<String> {
    let c = r.container.as_ref()?;
    let at = c.started_at.filter(|_| c.state != "exited")?;
    Some(util::ago(at).trim_end_matches(" ago").to_string())
}

fn pad(span: Span<'static>, width: usize) -> Span<'static> {
    let text = truncate(&span.content, width.saturating_sub(1));
    Span::styled(format!("{text:<width$}"), span.style)
}

/// "least_conn · autoscale 2–6 · cpu 70%", the line under the Replicas table.
fn routing_note(service: &Service) -> String {
    let s = &service.scaling;
    if s.autoscale.enabled {
        format!("{} · autoscale {}", s.lb_policy, s.autoscale.summary())
    } else {
        s.lb_policy.to_string()
    }
}

/// The Replicas panel of the Overview: one selectable row per replica; ⏎ or a click follows its
/// logs.
pub(crate) fn replicas_panel(frame: &mut Frame, area: Rect, view: &ServiceView) {
    let block = panel(
        title("Replicas", true),
        Some(Line::from(Span::styled("⏎ logs", theme::muted()))),
        true,
    );
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    if area.width >= 12 {
        mouse::key_target(Rect::new(area.right() - 8, area.y, 6, 1), "⏎");
    }
    mouse::hit(inner, Hit::List(List::Replicas));
    if inner.height == 0 {
        return;
    }
    let heading: Vec<Span> = COLUMNS
        .iter()
        .map(|(name, w)| match w {
            0 => Span::styled(*name, theme::muted()),
            w => Span::styled(format!("{name:<w$}"), theme::muted()),
        })
        .collect();
    frame.render_widget(
        Paragraph::new(Line::from(heading)),
        Rect { height: 1, ..inner },
    );
    let rows = inner.height.saturating_sub(3) as usize;
    let replicas = view.replicas();
    let offset = crate::view::scroll_offset(view.row.min(replicas.len().saturating_sub(1)), rows);
    for (i, r) in replicas.iter().enumerate().skip(offset).take(rows) {
        let row = Rect::new(inner.x, inner.y + 1 + (i - offset) as u16, inner.width, 1);
        let selected = i == view.row;
        let (glyph, state) = replica_state(r);
        let running = r.container.as_ref().is_some_and(|c| c.state == "running");
        let restarts = r.container.as_ref().map_or(0, |c| c.restart_count);
        let dash = || "—".to_string();
        let cells = vec![
            pad(
                Span::styled(
                    format!("#{}", r.index),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                ),
                COLUMNS[0].1,
            ),
            glyph,
            Span::raw(" "),
            pad(state, COLUMNS[1].1 - 2),
            pad(health(r.health), COLUMNS[2].1),
            pad(
                Span::styled(
                    r.cpu_percent
                        .filter(|_| running)
                        .map_or_else(dash, |c| format!("{c:.0}%")),
                    theme::text(),
                ),
                COLUMNS[3].1,
            ),
            pad(
                Span::styled(
                    r.mem_used
                        .filter(|_| running)
                        .map_or_else(dash, |m| crate::metrics::mib(m as f64)),
                    theme::text(),
                ),
                COLUMNS[4].1,
            ),
            pad(
                Span::styled(
                    restarts.to_string(),
                    if restarts > 0 {
                        theme::fg(theme::ERR)
                    } else {
                        theme::text()
                    },
                ),
                COLUMNS[5].1,
            ),
            pad(
                Span::styled(uptime(r).unwrap_or_else(dash), theme::muted()),
                COLUMNS[6].1,
            ),
            Span::styled(
                r.container
                    .as_ref()
                    .map_or_else(|| "not created".to_string(), |c| c.name.clone()),
                theme::muted(),
            ),
        ];
        let style = if selected {
            theme::selected()
        } else {
            Style::new()
        };
        frame.render_widget(Paragraph::new(Line::from(cells)).style(style), row);
        mouse::hit(row, Hit::Replica(i));
    }
    if let Some(d) = &view.detail
        && inner.height >= 3
    {
        frame.render_widget(
            Paragraph::new(routing_note(&d.service)).style(theme::muted()),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}

/// The rail panel beside the Logs tab of a replicated service; a click follows a replica.
pub(crate) fn replicas_rail(frame: &mut Frame, area: Rect, view: &ServiceView) {
    let logs = view.tab == Tab::Logs;
    let block = panel(title("Replicas", logs), None, logs);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    mouse::hit(inner, Hit::List(List::Containers));
    let picked = view.log_replica();
    for (i, r) in view
        .replicas()
        .iter()
        .enumerate()
        .take(inner.height as usize)
    {
        let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let selected = logs && picked == Some(r.index);
        let (glyph, state) = replica_state(r);
        let line = Line::from(vec![
            glyph,
            Span::raw(" "),
            Span::styled(
                format!("{:<5}", format!("#{}", r.index)),
                if selected {
                    theme::accent()
                } else {
                    theme::text()
                },
            ),
            pad(state, 13),
            Span::styled(
                uptime(r).map_or_else(String::new, |u| format!("up {u}")),
                theme::muted(),
            ),
        ])
        .style(if selected {
            theme::selected()
        } else {
            Style::new()
        });
        frame.render_widget(Paragraph::new(line), row);
        mouse::hit(row, Hit::Replica(i));
    }
}

/// "2–6 replicas · cpu 70% · rps 50/replica", autoscaling as the Scaling panel says it.
fn autoscale_line(a: &Autoscale) -> String {
    let mut parts = vec![format!("{}–{} replicas", a.min, a.max)];
    if let Some(cpu) = a.cpu_percent {
        parts.push(format!("cpu {cpu}%"));
    }
    if let Some(mem) = a.memory_percent {
        parts.push(format!("mem {mem}%"));
    }
    if let Some(rps) = a.rps {
        parts.push(format!("rps {rps}/replica"));
    }
    parts.join(" · ")
}

/// "round_robin, ip_hash for sticky sessions": the policies a service could switch to.
fn other_policies(current: LbPolicy) -> String {
    let others: Vec<&str> = LbPolicy::ALL
        .into_iter()
        .filter(|p| *p != current)
        .map(LbPolicy::as_str)
        .collect();
    let sticky = if current == LbPolicy::IpHash {
        ""
    } else {
        " for sticky sessions"
    };
    format!("{}{sticky}", others.join(", "))
}

fn on_off(on: bool) -> Vec<Span<'static>> {
    if on {
        vec![Span::styled("● on", theme::fg(theme::OK))]
    } else {
        vec![Span::styled("○ off", theme::muted())]
    }
}

/// The Scaling panel of the Settings tab. ⏎ on a row edits it; `s` and `a` open the scale
/// dialog and the autoscale form.
pub(crate) fn scaling_panel(frame: &mut Frame, area: Rect, view: &ServiceView) {
    let focused = view.scaling_focus;
    let Some(d) = &view.detail else {
        frame.render_widget(panel(title("Scaling", false), None, false), area);
        return;
    };
    let s = &d.service;
    let can = scalable(s);
    let block = panel(
        title("Scaling", focused),
        (can && focused).then(|| Line::from(Span::styled("⏎ to edit", theme::muted()))),
        focused,
    );
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    if !can {
        frame.render_widget(
            Paragraph::new(unsupported(s)).style(theme::muted()),
            Rect { height: 1, ..inner },
        );
        return;
    }
    mouse::hit(area, Hit::ScalingFocus(true));
    if area.width >= 23 {
        mouse::key_target(Rect::new(area.right() - 11, area.y, 9, 1), "⏎");
    }
    let sc = &s.scaling;
    let a = &sc.autoscale;
    let mut autoscale = on_off(a.enabled);
    if a.enabled {
        autoscale.push(Span::styled(
            format!("  {}", autoscale_line(a)),
            theme::muted(),
        ));
    }
    let rows: [ScalingRow; 5] = [
        (
            vec![Span::styled(s.replicas.to_string(), theme::text())],
            Some(('s', "scale")),
        ),
        (
            vec![
                Span::styled(sc.lb_policy.to_string(), theme::text()),
                Span::styled(
                    format!("  {}", other_policies(sc.lb_policy)),
                    theme::muted(),
                ),
            ],
            None,
        ),
        (
            vec![Span::styled(sc.max_surge.to_string(), theme::text())],
            None,
        ),
        (autoscale, Some(('a', "autoscale"))),
        (on_off(sc.shared_volume_ok), None),
    ];
    let mut y = inner.y;
    for (i, (value, hint)) in rows.into_iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, y, inner.width, 1);
        let selected = focused && view.row == i;
        mouse::hit(row, Hit::Scaling(i));
        let mut spans = vec![Span::styled(
            format!("{:<SCALING_KEY_W$}", SCALING_ROWS[i]),
            if selected {
                theme::accent()
            } else {
                theme::muted()
            },
        )];
        spans.extend(value);
        if let Some((key, label)) = hint {
            spans.push(Span::raw("   "));
            let offset: usize = spans.iter().map(Span::width).sum();
            mouse::span_target(
                row,
                offset,
                2 + label.len(),
                Hit::Key(KeyCode::Char(key), KeyModifiers::NONE),
            );
            spans.push(Span::styled(key.to_string(), theme::accent()));
            spans.push(Span::styled(format!(" {label}"), theme::muted()));
        }
        let style = if selected {
            theme::selected()
        } else {
            Style::new()
        };
        frame.render_widget(Paragraph::new(Line::from(spans)).style(style), row);
        y += 1;
        if i == 3 && a.enabled && y < inner.bottom() {
            frame.render_widget(
                Paragraph::new(format!(
                    "{:SCALING_KEY_W$}cooldown up {} / down {}",
                    "",
                    duration(a.scale_up_cooldown_secs),
                    duration(a.scale_down_cooldown_secs)
                ))
                .style(theme::muted()),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }
    }
}

/// "60s", "5m" or "1h": a cooldown as the forms take it.
pub fn duration(secs: u32) -> String {
    match secs {
        0 => "0s".into(),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 && s >= 120 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// Seconds from "90", "60s", "5m" or "1h".
pub fn parse_duration(text: &str) -> Result<u32, String> {
    let text = text.trim().to_ascii_lowercase();
    let wrong = || format!("'{text}' is not a duration like 60s or 5m");
    let (digits, unit) = match text.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => text.split_at(i),
        None => (text.as_str(), ""),
    };
    let n: u32 = digits.parse().map_err(|_| wrong())?;
    let scale = match unit.trim() {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(wrong()),
    };
    n.checked_mul(scale).ok_or_else(wrong)
}

/// A replica count from a form field: 1 to the most a service may run.
pub fn count(text: &str, what: &str) -> Result<u16, String> {
    match text.trim().parse::<u16>() {
        Ok(n) if (1..=MAX_REPLICAS).contains(&n) => Ok(n),
        _ => Err(format!("{what} is a number from 1 to {MAX_REPLICAS}")),
    }
}

/// An optional target: empty is off.
fn target<T: std::str::FromStr>(form: &Form, key: &str, what: &str) -> Result<Option<T>, String> {
    let text = form.get(key);
    let text = text.trim_end_matches('%').trim();
    if text.is_empty() || text == "—" {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| format!("the {what} is a whole number, or empty for off"))
}

/// The autoscale form as the daemon takes it, checked the way the daemon checks it.
pub fn autoscale_from(form: &Form) -> Result<Autoscale, String> {
    let cooldown =
        |key: &str, what: &str| parse_duration(&form.get(key)).map_err(|e| format!("{what}: {e}"));
    let a = Autoscale {
        enabled: form.choice("enabled") == 0,
        min: count(&form.get("min"), "min")?,
        max: count(&form.get("max"), "max")?,
        cpu_percent: target(form, "cpu", "cpu target")?,
        memory_percent: target(form, "memory", "memory target")?,
        rps: target(form, "rps", "requests target")?,
        scale_up_cooldown_secs: cooldown("up", "scale-up cooldown")?,
        scale_down_cooldown_secs: cooldown("down", "scale-down cooldown")?,
        stabilization_secs: cooldown("stabilization", "stabilization")?,
    };
    a.validate()?;
    Ok(a)
}

/// The scale dialog: the replica count, what the server has room for, and what changes.
pub struct ScaleDialog {
    pub service_id: Id,
    pub name: String,
    pub server: String,
    pub current: u16,
    pub target: u16,
    /// Digits replace the count until the first edit; afterwards they append.
    typed: bool,
    pub capacity: Option<Result<ScaleCapacity, String>>,
    /// Volumes every replica would mount, as name and mount path.
    pub volumes: Vec<(String, String)>,
    pub shared_ok: bool,
    /// "Allow a shared volume" is checked: the service opts in before it scales.
    pub allow: bool,
    /// Autoscaling's summary while it manages the count.
    pub autoscale: Option<String>,
    /// The service has a live deployment, so scaling starts or drains containers now.
    pub live: bool,
    pub busy: bool,
}

impl ScaleDialog {
    /// More than one replica would write a volume nobody allowed sharing.
    pub fn refused(&self) -> bool {
        self.target > 1 && !self.volumes.is_empty() && !self.shared_ok
    }

    fn ready(&self) -> bool {
        self.autoscale.is_none() && self.target >= 1 && (!self.refused() || self.allow)
    }
}

impl App {
    /// `s` on the Settings tab: the scale dialog of the open service.
    pub fn open_scale(&mut self) {
        let Some(d) = self.service.as_ref().and_then(|v| v.detail.as_ref()) else {
            return;
        };
        let s = &d.service;
        if !scalable(s) {
            return self.error(unsupported(s));
        }
        let a = &s.scaling.autoscale;
        let id = s.id.clone();
        self.overlay = Some(Overlay::Scale(ScaleDialog {
            service_id: s.id.clone(),
            name: s.name.clone(),
            server: d.server.name.clone(),
            current: s.replicas,
            target: s.replicas,
            typed: false,
            capacity: None,
            volumes: d
                .volumes
                .iter()
                .map(|v| (v.name.clone(), v.mount_path.clone()))
                .collect(),
            shared_ok: s.scaling.shared_volume_ok,
            allow: false,
            autoscale: a.enabled.then(|| a.summary()),
            live: s.live_deployment_id.is_some(),
            busy: false,
        }));
        self.spawn(|c| async move {
            let capacity = c.scale_capacity(&id).await.map_err(|e| format!("{e:#}"));
            Msg::Capacity(id, capacity)
        });
    }

    fn submit_scale(&mut self) {
        let Some(Overlay::Scale(d)) = self.overlay.as_mut() else {
            return;
        };
        if d.busy || !d.ready() {
            return;
        }
        if d.target == d.current {
            self.overlay = None;
            return;
        }
        d.busy = true;
        let (id, n, name) = (d.service_id.clone(), d.target, d.name.clone());
        let share = d.refused() && d.allow;
        self.spawn(move |c| async move {
            if share {
                let body = UpdateService {
                    shared_volume_ok: Some(true),
                    ..Default::default()
                };
                if let Err(e) = c.update_service(&id, &body).await {
                    return Msg::Scaled(Err(format!("{e:#}")));
                }
            }
            Msg::Scaled(
                c.scale(&id, n)
                    .await
                    .map(|_| format!("{name} scales to {n} replica{}", plural(n)))
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    /// `a` on the Settings tab: the autoscale form of the open service.
    pub fn form_autoscale(&mut self) {
        let Some(d) = self.service.as_ref().and_then(|v| v.detail.as_ref()) else {
            return;
        };
        let s = &d.service;
        if !scalable(s) {
            return self.error(unsupported(s));
        }
        let a = &s.scaling.autoscale;
        let text = |v: Option<String>| v.unwrap_or_default();
        let fields = vec![
            Field::choice("enabled", "autoscale", vec!["on".into(), "off".into()])
                .selected(usize::from(!a.enabled)),
            Field::text("min", "min")
                .value(a.min.to_string())
                .hint("replicas"),
            Field::text("max", "max")
                .value(a.max.to_string())
                .hint("replicas"),
            Field::text("cpu", "cpu target")
                .value(text(a.cpu_percent.map(|v| v.to_string())))
                .placeholder("—"),
            Field::text("memory", "memory target")
                .value(text(a.memory_percent.map(|v| v.to_string())))
                .placeholder("—"),
            Field::text("rps", "req/s per replica")
                .value(text(a.rps.map(|v| v.to_string())))
                .placeholder("—"),
            Field::text("up", "scale-up cooldown").value(duration(a.scale_up_cooldown_secs)),
            Field::text("down", "scale-down cooldown").value(duration(a.scale_down_cooldown_secs)),
            Field::text("stabilization", "stabilization").value(duration(a.stabilization_secs)),
        ];
        let form = Form::new(
            FormKind::Autoscale {
                service_id: s.id.clone(),
            },
            "Autoscale",
            fields,
        )
        .subtitle(s.name.clone())
        .note(vec![AUTOSCALE_NOTE.into()])
        .submit("save", None);
        self.overlay = Some(Overlay::Form(form));
    }

    /// ⏎ on a row of the Scaling panel.
    pub fn edit_scaling_row(&mut self, row: usize) {
        let Some(d) = self.service.as_ref().and_then(|v| v.detail.as_ref()) else {
            return;
        };
        let s = d.service.clone();
        if !scalable(&s) {
            return self.error(unsupported(&s));
        }
        let id = s.id.clone();
        match row {
            0 => self.open_scale(),
            1 => {
                let all = LbPolicy::ALL;
                let at = all
                    .iter()
                    .position(|p| *p == s.scaling.lb_policy)
                    .unwrap_or(0);
                let next = all[(at + 1) % all.len()];
                let body = UpdateService {
                    lb_policy: Some(next),
                    ..Default::default()
                };
                self.act(format!("load balancing {next} · applied"), |c| async move {
                    c.update_service(&id, &body).await
                });
            }
            2 => {
                let form = Form::new(
                    FormKind::RollingSurge { service_id: id },
                    "Rolling surge",
                    vec![
                        Field::text("surge", "rolling surge")
                            .value(s.scaling.max_surge.to_string())
                            .hint("replicas at once"),
                    ],
                )
                .subtitle(s.name.clone())
                .note(vec![
                    "A rolling deploy starts this many new replicas before it stops old ones."
                        .into(),
                ]);
                self.overlay = Some(Overlay::Form(form));
            }
            3 => self.form_autoscale(),
            _ => {
                let on = !s.scaling.shared_volume_ok;
                let body = UpdateService {
                    shared_volume_ok: Some(on),
                    ..Default::default()
                };
                let toast = if on {
                    "shared volume allowed · replicas may write the same files"
                } else {
                    "shared volume off"
                };
                self.act(toast.into(), |c| async move {
                    c.update_service(&id, &body).await
                });
            }
        }
    }
}

fn plural(n: impl Into<u64>) -> &'static str {
    if n.into() == 1 { "" } else { "s" }
}

/// Keys of the scale dialog: arrows or digits set the count, space allows a shared volume.
pub fn scale_keys(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Scale(d)) = app.overlay.as_mut() else {
        return;
    };
    if d.busy {
        return;
    }
    match key.code {
        KeyCode::Esc => app.overlay = None,
        KeyCode::Enter => app.submit_scale(),
        KeyCode::Char('a') if d.autoscale.is_some() => app.form_autoscale(),
        KeyCode::Up | KeyCode::Char('k') => {
            d.typed = true;
            d.target = (d.target + 1).min(MAX_REPLICAS);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            d.typed = true;
            d.target = d.target.saturating_sub(1).max(1);
        }
        KeyCode::Char(' ') if d.refused() => d.allow = !d.allow,
        KeyCode::Char(c @ '0'..='9') => {
            let base = if d.typed { d.target } else { 0 };
            d.typed = true;
            d.target = (base * 10 + (c as u16 - '0' as u16)).min(MAX_REPLICAS);
        }
        KeyCode::Backspace => {
            d.typed = true;
            d.target /= 10;
        }
        _ => {}
    }
}

/// The scale dialog: "SVC-19 Scale" and, when a volume is in the way, "SVC-20 Scale · refused".
pub(crate) fn render_scale(frame: &mut Frame, area: Rect, d: &ScaleDialog, tick: usize) {
    let refused = d.refused() && d.autoscale.is_none();
    let mut body: Vec<Line<'static>> = Vec::new();
    if let Some(summary) = &d.autoscale {
        body.push(Line::styled(
            format!("Autoscaling manages these replicas: {summary}."),
            theme::text(),
        ));
        body.push(Line::styled(
            "Turn it off in the autoscale form to scale by hand.",
            theme::muted(),
        ));
    } else if refused {
        body.push(Line::styled(
            "✕ Replicas > 1 refused",
            theme::fg(theme::ERR),
        ));
        for (name, mount) in &d.volumes {
            body.push(Line::from(vec![
                Span::styled(format!("{name} → {mount}"), theme::text()),
                Span::styled("  writable volume", theme::muted()),
            ]));
        }
        body.push(Line::styled(
            "A named volume shared by replicas means they all write the same files.",
            theme::muted(),
        ));
        body.push(Line::raw(""));
        body.push(Line::raw(""));
    } else {
        body.push(capacity_line(d, tick));
        let change = i32::from(d.target) - i32::from(d.current);
        let n = change.unsigned_abs();
        let what = format!("container{}", plural(n));
        body.push(match change {
            _ if d.target == 0 => {
                Line::styled("A service runs at least 1 replica.", theme::fg(theme::ERR))
            }
            0 => Line::styled("No change.", theme::muted()),
            _ if !d.live => Line::styled(
                format!(
                    "Starts {} replica{} on the first deploy.",
                    d.target,
                    plural(d.target)
                ),
                theme::text(),
            ),
            c if c > 0 => Line::styled(format!("Adds {n} {what}. No redeploy."), theme::text()),
            _ => Line::styled(format!("Drains {n} {what}. No redeploy."), theme::text()),
        });
    }
    let inner = dialog(
        frame,
        area,
        80,
        body.len() as u16 + 10,
        vec![
            Span::styled("Scale", theme::text()),
            Span::styled(format!(" {}", d.name), theme::muted()),
        ],
    );
    if inner.height < 5 {
        return;
    }
    let field_focus = !refused && d.autoscale.is_none();
    let label_w = 16u16;
    let field_w = inner.width.saturating_sub(label_w + 14).min(38);
    frame.render_widget(
        Paragraph::new("replicas").style(if field_focus {
            theme::accent()
        } else {
            theme::muted()
        }),
        Rect { height: 1, ..inner },
    );
    let mut value = vec![
        Span::raw(" "),
        Span::styled(format!("{} → ", d.current), theme::muted()),
        Span::styled(
            if d.target == 0 {
                String::new()
            } else {
                d.target.to_string()
            },
            if refused {
                theme::fg(theme::ERR)
            } else {
                theme::text()
            },
        ),
    ];
    if field_focus {
        value.push(Span::styled(
            if tick % 8 < 5 { "▏" } else { " " },
            theme::accent(),
        ));
    }
    let field = Rect::new(inner.x + label_w, inner.y, field_w, 1);
    frame.render_widget(Paragraph::new(Line::from(value)), field);
    frame.render_widget(
        Paragraph::new("─".repeat(field_w as usize)).style(if refused {
            theme::fg(theme::ERR)
        } else {
            theme::fg(theme::LINE)
        }),
        Rect {
            y: field.y + 1,
            ..field
        },
    );
    if d.autoscale.is_none() {
        let hint = Rect::new(
            field.right() + 2,
            inner.y,
            inner.right().saturating_sub(field.right() + 2),
            1,
        );
        mouse::hints(frame, hint, &[("↑↓", "adjust")], Alignment::Left);
    }
    let body_area =
        Rect::new(inner.x, inner.y + 3, inner.width, body.len() as u16).intersection(inner);
    let rows = body.len() as u16;
    frame.render_widget(Paragraph::new(body), body_area);
    if refused && body_area.height == rows {
        let check = Rect::new(inner.x, body_area.bottom() - 1, inner.width, 1);
        let mark = if d.allow { "[x]" } else { "[ ]" };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {mark}  "), theme::accent()),
                Span::styled("Allow a shared volume", theme::accent()),
            ]))
            .style(Style::new().bg(theme::BG)),
            check,
        );
        mouse::hit(check, Hit::Key(KeyCode::Char(' '), KeyModifiers::NONE));
    }
    let keys = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
    let rule = Rect {
        y: keys.y - 1,
        ..keys
    };
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    if d.busy {
        frame.render_widget(
            Paragraph::new("waiting for the daemon").style(theme::muted()),
            keys,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} scaling… ", util::spinner(tick)),
                theme::chip(),
            ))
            .right_aligned(),
            keys,
        );
        return;
    }
    let chip = if d.ready() {
        theme::chip()
    } else {
        Style::new().fg(theme::MUTED).bg(theme::LINE)
    };
    let rest = action_chip(frame, keys, "⏎", "scale", chip);
    let hints: &[(&str, &str)] = if d.autoscale.is_some() {
        &[("a", "autoscale"), ("esc", "cancel")]
    } else if refused {
        &[("space", "toggle"), ("esc", "cancel")]
    } else {
        &[("↑↓", "adjust"), ("esc", "cancel")]
    };
    mouse::hints(frame, rest, hints, Alignment::Left);
}

/// "Memory headroom on server prod-eu-1 fits 9 more replicas (~85 MiB each)".
fn capacity_line(d: &ScaleDialog, tick: usize) -> Line<'static> {
    match &d.capacity {
        None => Line::styled(
            format!(
                "{} checking memory headroom on server {}",
                util::spinner(tick),
                d.server
            ),
            theme::muted(),
        ),
        Some(Err(e)) => Line::styled(format!("memory headroom unknown · {e}"), theme::muted()),
        Some(Ok(c)) => {
            let more = u32::from(d.target.saturating_sub(d.current));
            let text = format!(
                "Memory headroom on server {} fits {} more replica{} (~{} each)",
                d.server,
                c.fits_more,
                plural(c.fits_more),
                crate::metrics::mib(c.per_replica as f64)
            );
            if more > c.fits_more {
                Line::styled(format!("{text}, not {more}"), theme::fg(theme::ERR))
            } else {
                Line::styled(text, theme::muted())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_and_print_like_the_form() {
        assert_eq!(parse_duration("60s"), Ok(60));
        assert_eq!(parse_duration("5m"), Ok(300));
        assert_eq!(parse_duration("1h"), Ok(3600));
        assert_eq!(parse_duration(" 90 "), Ok(90));
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("5d").is_err());
        assert_eq!(duration(60), "60s");
        assert_eq!(duration(90), "90s");
        assert_eq!(duration(300), "5m");
        assert_eq!(duration(7200), "2h");
    }

    #[test]
    fn policies_name_the_others() {
        assert_eq!(
            other_policies(LbPolicy::LeastConn),
            "round_robin, ip_hash for sticky sessions"
        );
        assert_eq!(other_policies(LbPolicy::IpHash), "least_conn, round_robin");
    }
}
