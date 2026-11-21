//! Drawing. Every screen follows the Paper design: a top bar, bordered panels, a key bar.

use crate::app::*;
use crate::database;
use crate::mouse::{self, Hit, List};
use crate::picker::short_key;
use crate::theme;
use crate::util::{self, truncate};
use common::*;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

pub fn render(frame: &mut Frame, app: &App) {
    mouse::reset();
    let area = frame.area();
    frame.render_widget(
        Block::new().style(Style::new().bg(theme::BG).fg(theme::TEXT)),
        area,
    );
    let [top, body, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area.inner(Margin::new(1, 0)));
    top_bar(frame, top, app);
    let body = Rect {
        y: body.y + 1,
        height: body.height.saturating_sub(1),
        ..body
    };
    match app.screen {
        Screen::Overview => overview(frame, body, app),
        Screen::Service => service(frame, body, app),
        Screen::Deploy => deploy(frame, body, app),
        Screen::Servers => servers(frame, body, app),
        Screen::Settings => settings(frame, body, app),
        Screen::Secrets => secrets(frame, body, app),
        Screen::Storage => storage(frame, body, app),
    }
    key_bar(frame, keys, app);
    if app.overlay.is_some() {
        mouse::reset();
        mouse::hit(area, Hit::Block);
    }
    match &app.overlay {
        Some(Overlay::Form(form)) => {
            dim(frame, area);
            form.render(frame, area, app.tick);
        }
        Some(Overlay::Confirm(c)) => {
            dim(frame, area);
            confirm(frame, area, c, app.tick);
        }
        Some(Overlay::Palette(p)) => {
            dim(frame, area);
            palette(frame, area, p);
        }
        Some(Overlay::Reveal(r)) => {
            dim(frame, area);
            reveal(frame, area, r);
        }
        Some(Overlay::Picker(p)) => {
            dim(frame, area);
            p.render(frame, area, app.tick);
        }
        Some(Overlay::Help) => {
            dim(frame, area);
            help(frame, area, app);
        }
        None => {}
    }
    mouse::finish_frame(&app.mouse);
    mouse::paint_hover(frame.buffer_mut(), &app.mouse);
    mouse::paint_selection(frame.buffer_mut(), app.mouse.selection.as_ref());
}

/// Fade everything under a dialog, like the scrim in the design.
fn dim(frame: &mut Frame, area: Rect) {
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_fg(theme::LINE);
        }
    }
}

/// A rounded panel with its title set into the top border.
fn panel<'a>(title: Vec<Span<'a>>, right: Option<Line<'a>>, focused: bool) -> Block<'a> {
    let border = if focused {
        theme::muted()
    } else {
        theme::fg(theme::LINE)
    };
    let mut spans = vec![Span::raw(" ")];
    spans.extend(title);
    spans.push(Span::raw(" "));
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title_top(Line::from(spans));
    if let Some(right) = right {
        let mut r = vec![Span::raw(" ")];
        r.extend(right.spans);
        r.push(Span::raw(" "));
        block = block.title_top(Line::from(r).right_aligned());
    }
    block
}

fn title(text: &str, focused: bool) -> Vec<Span<'static>> {
    vec![Span::styled(
        text.to_string(),
        if focused {
            theme::text()
        } else {
            theme::muted()
        },
    )]
}

/// Body area of a panel with one column of breathing room on each side.
fn inside(block: &Block, area: Rect) -> Rect {
    block.inner(area).inner(Margin::new(1, 0))
}

fn top_bar(frame: &mut Frame, area: Rect, app: &App) {
    let mut left = vec![
        Span::styled(brand::NAME.to_lowercase(), theme::accent()),
        Span::styled("  ›  ", theme::muted()),
        Span::styled(app.context.clone(), theme::text()),
        Span::raw("  "),
    ];
    if let Some(info) = &app.info {
        let host = info
            .public_ip
            .clone()
            .unwrap_or_else(|| info.hostname.clone());
        left.push(Span::styled(
            format!("{host} · {} {}  ", brand::DAEMON_BIN, info.version),
            theme::muted(),
        ));
    }
    left.push(match (&app.connection_error, &app.info) {
        _ if app.updating.is_some() => Span::styled(
            format!(
                "{} updating to {}{}",
                util::spinner(app.tick),
                app.updating.as_deref().unwrap_or_default(),
                if app.connection_error.is_some() {
                    " · reconnecting"
                } else {
                    ""
                }
            ),
            theme::fg(theme::INFO),
        ),
        (Some(_), _) => Span::styled("○ disconnected", theme::fg(theme::ERR)),
        (None, Some(_)) => Span::styled("● connected", theme::fg(theme::OK)),
        (None, None) => Span::styled(
            format!("{} connecting", util::spinner(app.tick)),
            theme::muted(),
        ),
    });
    let context_offset = left[0].width() + left[1].width();
    let context_width = left[2].width();

    let Some(info) = &app.info else {
        mouse::span_target(area, context_offset, context_width, Hit::OpenSettings);
        frame.render_widget(Paragraph::new(Line::from(left)), area);
        return;
    };
    let history: Vec<f64> = info.cpu_history.iter().map(|v| f64::from(*v)).collect();
    let disk = (info.disk_used * 100)
        .checked_div(info.disk_total)
        .unwrap_or(0);
    let (proxy_glyph, proxy_style) = match info.proxy {
        ProxyStatus::Running => ("●", theme::fg(theme::OK)),
        ProxyStatus::Stopped => ("○", theme::fg(theme::ERR)),
        ProxyStatus::Missing => ("○", theme::muted()),
    };
    let right = Line::from(vec![
        Span::styled("cpu ", theme::muted()),
        Span::styled(format!("{:.0}% ", info.cpu_percent), theme::text()),
        Span::styled(
            util::sparkline(&history, 8, Some(100.0)),
            theme::fg(theme::OK),
        ),
        Span::styled("   mem ", theme::muted()),
        Span::styled(
            format!(
                "{}/{}",
                util::bytes(info.mem_used),
                util::bytes(info.mem_total)
            ),
            theme::text(),
        ),
        Span::styled("   disk ", theme::muted()),
        Span::styled(format!("{disk}%"), theme::text()),
        Span::styled("   proxy ", theme::muted()),
        Span::styled(proxy_glyph, proxy_style),
        Span::styled(" caddy", theme::text()),
    ])
    .right_aligned();
    let left_area = Rect {
        width: area.width.saturating_sub(right.width() as u16),
        ..area
    };
    let left_area = if (left_area.width as usize) < context_offset + context_width {
        area
    } else {
        frame.render_widget(Paragraph::new(right), area);
        left_area
    };
    mouse::span_target(left_area, context_offset, context_width, Hit::OpenSettings);
    frame.render_widget(Paragraph::new(Line::from(left)), left_area);
}

type Keys = Vec<(&'static str, &'static str)>;

fn keys_for(app: &App) -> (Keys, Keys) {
    let right = vec![(",", "settings"), ("?", "help"), ("q", "quit")];
    let left = match app.screen {
        Screen::Overview if app.overview.filtering => {
            vec![("type", "to filter"), ("⏎", "keep"), ("esc", "clear")]
        }
        Screen::Overview => vec![
            ("↑↓", "move"),
            ("⏎", "open"),
            ("d", "deploy"),
            ("r", "restart"),
            ("l", "logs"),
            ("n", "new"),
            ("/", "filter"),
            (":", "command"),
        ],
        Screen::Service => {
            let tab = app.service.as_ref().map_or(Tab::Overview, |v| v.tab);
            let searching = app.service.as_ref().is_some_and(|v| v.logs.searching);
            match tab {
                Tab::Logs if searching => {
                    vec![("type", "to search"), ("⏎", "find"), ("esc", "cancel")]
                }
                Tab::Logs => vec![
                    ("tab", "next tab"),
                    ("f", "follow"),
                    ("/", "search"),
                    ("t", "timestamps"),
                    ("w", "wrap"),
                    ("e", "errors only"),
                    ("d", "deploy"),
                ],
                Tab::Deployments => vec![
                    ("tab", "next tab"),
                    ("↑↓", "move"),
                    ("⏎", "build log"),
                    ("R", "rollback"),
                    ("c", "cancel"),
                    ("d", "deploy"),
                ],
                Tab::Env => vec![
                    ("tab", "next tab"),
                    ("a", "add"),
                    ("⏎", "edit"),
                    ("x", "remove"),
                    ("v", "reveal"),
                    ("d", "deploy"),
                ],
                Tab::Domains => vec![
                    ("tab", "next tab"),
                    ("a", "add"),
                    ("x", "remove"),
                    ("d", "deploy"),
                ],
                Tab::Volumes => vec![
                    ("tab", "next tab"),
                    ("a", "add"),
                    ("x", "remove"),
                    ("X", "remove + data"),
                ],
                Tab::Settings => vec![("tab", "next tab"), ("⏎", "edit"), ("X", "delete service")],
                Tab::Database => {
                    let view = app.service.as_ref();
                    let reveal = view.is_some_and(|v| v.reveal);
                    let public = view
                        .and_then(|v| v.info())
                        .is_some_and(|i| i.public_port.is_some());
                    let mut keys = vec![
                        ("tab", "next tab"),
                        ("v", if reveal { "hide" } else { "reveal" }),
                        ("c", "copy url"),
                    ];
                    if public {
                        keys.push(("C", "copy public url"));
                    }
                    keys.extend([
                        ("p", if public { "unexpose" } else { "expose" }),
                        ("R", "rotate password"),
                        ("d", "deploy"),
                    ]);
                    keys
                }
                Tab::Backups => vec![
                    ("tab", "next tab"),
                    ("↑↓", "move"),
                    ("b", "back up now"),
                    ("R", "restore"),
                    ("D", "download"),
                    ("x", "delete"),
                    ("e", "schedule"),
                ],
                Tab::Overview => vec![
                    ("tab", "next tab"),
                    ("d", "deploy"),
                    ("r", "restart"),
                    ("s", "stop/start"),
                    ("l", "logs"),
                ],
            }
        }
        Screen::Deploy => vec![
            ("f", "follow"),
            ("↑↓", "scroll"),
            ("w", "wrap"),
            ("c", "cancel deploy"),
            ("esc", "back · keeps running"),
        ],
        Screen::Servers => vec![
            ("↑↓", "move"),
            ("a", "add server"),
            ("c", "check"),
            ("K", "change key"),
            ("p", "set up proxy"),
            ("x", "remove"),
            ("X", "remove + clean up"),
            ("esc", "back"),
        ],
        Screen::Settings if app.settings.settings.is_none() => {
            vec![("↑↓", "context"), ("⏎", "switch"), ("x", "remove")]
        }
        Screen::Settings if app.settings.focus == SettingsFocus::Backups => vec![
            ("tab", "contexts"),
            ("↑↓", "move"),
            ("b", "back up now"),
            ("e", "schedule"),
            ("D", "download"),
            ("x", "delete"),
            ("s", "secrets"),
            ("o", "storage"),
        ],
        Screen::Settings => vec![
            ("tab", "backups"),
            ("↑↓", "context"),
            ("⏎", "switch"),
            ("x", "remove"),
            ("u", "update"),
            ("i", "public ip"),
            ("m", "auto proxy"),
            ("r", "restart proxy"),
            ("t", "rotate token"),
            ("k", "copy key"),
            ("c", "prune"),
        ],
        Screen::Secrets => {
            let revealed = app.secrets.value().is_some();
            let mut keys = vec![("↑↓", "move"), ("a", "add"), ("g", "generate key")];
            if revealed {
                keys.extend([("v", "hide"), ("C", "copy value")]);
            } else {
                keys.extend([("v", "reveal"), ("c", "copy public key")]);
            }
            keys.extend([("n", "rename"), ("e", "replace value"), ("x", "delete")]);
            keys
        }
        Screen::Storage => vec![
            ("↑↓", "move"),
            ("a", "add"),
            ("⏎", "edit"),
            ("t", "test"),
            ("x", "delete"),
        ],
    };
    let right = match app.screen {
        Screen::Overview if app.overview.filtering => vec![],
        Screen::Overview => right,
        Screen::Deploy | Screen::Servers => vec![(":", "command"), ("?", "help"), ("q", "quit")],
        Screen::Service if app.service.as_ref().is_some_and(|v| v.logs.searching) => vec![],
        _ => vec![
            ("esc", "back"),
            (":", "command"),
            ("?", "help"),
            ("q", "quit"),
        ],
    };
    (left, right)
}

fn key_bar(frame: &mut Frame, area: Rect, app: &App) {
    let (left, right) = keys_for(app);
    let toast = match &app.toast {
        Some(t) if t.busy => {
            let max = area.width as usize / 2;
            Some(Line::from(vec![
                Span::styled(
                    format!("{} ", util::spinner(app.tick)),
                    theme::fg(theme::INFO),
                ),
                Span::styled(truncate(&t.text, max), theme::muted()),
            ]))
        }
        Some(t) => {
            let style = if t.error {
                theme::fg(theme::ERR)
            } else {
                theme::fg(theme::OK)
            };
            let glyph = if t.error { "✕ " } else { "✓ " };
            let max = area.width as usize / 2;
            Some(Line::from(Span::styled(
                format!("{glyph}{}", truncate(&t.text, max)),
                style,
            )))
        }
        None => None,
    };
    let right_width = toast
        .as_ref()
        .map_or_else(|| mouse::hints_width(&right), Line::width);
    let left_width =
        (area.width as usize).saturating_sub(right_width + usize::from(right_width > 0)) as u16;
    mouse::hints(
        frame,
        Rect {
            width: left_width,
            ..area
        },
        &left,
        Alignment::Left,
    );
    match toast {
        Some(line) => frame.render_widget(Paragraph::new(line.right_aligned()), area),
        None => mouse::hints(frame, area, &right, Alignment::Right),
    }
}

// Overview

fn overview(frame: &mut Frame, area: Rect, app: &App) {
    let [side, main] = Layout::horizontal([Constraint::Length(32), Constraint::Fill(1)])
        .spacing(1)
        .areas(area);
    sidebar(frame, side, app);
    if app.tree.as_ref().is_some_and(|t| t.projects.is_empty()) {
        return empty_state(frame, main);
    }
    let services = app.env_services();
    let table_h = (services.len() as u16 + 4).max(6).min(main.height / 2);
    let [table, lower] = Layout::vertical([Constraint::Length(table_h), Constraint::Fill(1)])
        .spacing(1)
        .areas(main);
    services_table(frame, table, app, &services);
    let [detail, activity] = Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)])
        .spacing(1)
        .areas(lower);
    match app.selected_service() {
        Some(s) => service_detail(frame, detail, app, s),
        None => {
            let block = panel(title("service", false), None, false);
            let inner = inside(&block, detail);
            frame.render_widget(block, detail);
            if !app.overview.filtering {
                mouse::hints(
                    frame,
                    inner,
                    &[("n", "add the first service")],
                    Alignment::Left,
                );
            }
        }
    }
    activity_panel(frame, activity, app);
}

fn empty_state(frame: &mut Frame, area: Rect) {
    let block = panel(title("Welcome", true), None, true);
    let inner = inside(&block, area).inner(Margin::new(2, 1));
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new("Nothing deployed yet.").style(theme::text()),
        inner,
    );
    for (line, hint) in lines_of(inner, 5).into_iter().skip(2).zip([
        ("N", "create a project, with a production environment"),
        ("n", "add a service from a git repo, an image or a database"),
        ("S", "servers and the proxy that serves your domains"),
    ]) {
        mouse::hints(frame, line, &[hint], Alignment::Left);
    }
}

fn sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let servers_h = (app.servers.len() as u16 + 2).clamp(3, 8);
    let [projects, servers] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(servers_h)])
            .spacing(1)
            .areas(area);
    let focused = app.overview.pane == Pane::Sidebar;
    let block = panel(title("Projects", focused), None, focused);
    let inner = inside(&block, projects);
    frame.render_widget(block, projects);
    mouse::hit(projects, Hit::Pane(Pane::Sidebar));
    sidebar_servers(frame, servers, app);

    let Some(tree) = &app.tree else {
        frame.render_widget(
            Paragraph::new(format!("{} loading", util::spinner(app.tick))).style(theme::muted()),
            inner,
        );
        return;
    };
    let items = app.side_items();
    let mut lines = Vec::new();
    let mut item_lines = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let selected = i == app.overview.side;
        let row_style = if selected {
            theme::selected()
        } else {
            Style::new()
        };
        let name_style = |dim: bool| {
            if selected {
                theme::accent()
            } else if dim {
                theme::muted()
            } else {
                theme::text()
            }
        };
        let width = inner.width as usize;
        let line = match *item {
            SideItem::Project(pi) => {
                if pi > 0 {
                    lines.push(Line::raw(""));
                }
                let p = &tree.projects[pi];
                let open = !app.overview.collapsed.contains(&p.project.id);
                let count: usize = p.environments.iter().map(|e| e.services.len()).sum();
                let name = truncate(&p.project.name, width.saturating_sub(8));
                let pad = width.saturating_sub(4 + name.chars().count() + count.to_string().len());
                Line::from(vec![
                    Span::styled(if open { " ▾ " } else { " ▸ " }, theme::muted()),
                    Span::styled(name, name_style(count == 0 && !open)),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(
                        if open {
                            String::new()
                        } else {
                            count.to_string()
                        },
                        theme::muted(),
                    ),
                    Span::raw(" "),
                ])
            }
            SideItem::Environment(pi, ei) => {
                let env = &tree.projects[pi].environments[ei];
                let state = env_state(env);
                let name = truncate(&env.environment.name, width.saturating_sub(10));
                let count = env.services.len().to_string();
                let pad = width.saturating_sub(6 + name.chars().count() + count.len());
                Line::from(vec![
                    Span::raw("   "),
                    util::state_glyph(state, app.tick),
                    Span::raw(" "),
                    Span::styled(name, name_style(env.services.is_empty())),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(count, theme::muted()),
                    Span::raw(" "),
                ])
            }
        };
        item_lines.push((lines.len(), i));
        lines.push(line.style(row_style));
    }
    if lines.is_empty() {
        mouse::hints(frame, inner, &[("N", "new project")], Alignment::Left);
        return;
    }
    let offset = scroll_offset(
        item_lines
            .get(app.overview.side)
            .map_or(0, |&(line, _)| line),
        inner.height as usize,
    );
    frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
    for (line, i) in item_lines {
        if let Some(dy) = line
            .checked_sub(offset)
            .filter(|dy| *dy < inner.height as usize)
        {
            let row = Rect::new(inner.x, inner.y + dy as u16, inner.width, 1);
            mouse::hit(row, Hit::Side(i));
            if matches!(items[i], SideItem::Project(_)) {
                mouse::span_target(row, 1, 1, Hit::Fold(i));
            }
        }
    }
}

fn sidebar_servers(frame: &mut Frame, servers: Rect, app: &App) {
    let block = panel(title("Servers", false), None, false);
    let inner = inside(&block, servers);
    frame.render_widget(block, servers);
    mouse::hit(servers, Hit::OpenServers);
    mouse::span_target(servers, 2, "Servers".len(), Hit::OpenServers);
    let lines: Vec<Line> = app
        .servers
        .iter()
        .take(inner.height as usize)
        .enumerate()
        .map(|(i, s)| {
            mouse::hit(
                Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
                Hit::SidebarServer(i),
            );
            let (glyph, style) = match s.status {
                ServerStatus::Online => ("●", theme::fg(theme::OK)),
                ServerStatus::Offline => ("○", theme::fg(theme::ERR)),
                ServerStatus::Unknown => ("○", theme::muted()),
            };
            let meta = match (s.kind, s.status) {
                (_, ServerStatus::Offline) => "offline",
                (ServerKind::Local, _) => "local",
                (ServerKind::Ssh, _) => "ssh",
            };
            let name = truncate(&s.name, (inner.width as usize).saturating_sub(12));
            let pad = (inner.width as usize).saturating_sub(3 + name.chars().count() + meta.len());
            Line::from(vec![
                Span::styled(format!(" {glyph} "), style),
                Span::styled(
                    name,
                    if s.status == ServerStatus::Offline {
                        theme::muted()
                    } else {
                        theme::text()
                    },
                ),
                Span::raw(" ".repeat(pad)),
                Span::styled(meta, theme::muted()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Worst state wins, so a failing service shows on its environment.
fn env_state(env: &EnvironmentNode) -> ServiceState {
    let states: Vec<ServiceState> = env.services.iter().map(|s| s.state).collect();
    [
        ServiceState::Failed,
        ServiceState::Degraded,
        ServiceState::Building,
        ServiceState::Deploying,
        ServiceState::Running,
    ]
    .into_iter()
    .find(|s| states.contains(s))
    .unwrap_or(if states.is_empty() {
        ServiceState::New
    } else {
        ServiceState::Stopped
    })
}

/// Clickable rows of a table drawn in `inner` with a one-line header and `offset` rows scrolled away.
fn row_hits(inner: Rect, rows: usize, offset: usize, hit: fn(usize) -> Hit) {
    for i in offset..rows {
        let y = inner.y + 1 + (i - offset) as u16;
        if y >= inner.bottom() {
            break;
        }
        mouse::hit(Rect::new(inner.x, y, inner.width, 1), hit(i));
    }
}

fn scroll_offset(selected: usize, height: usize) -> usize {
    (selected + 1).saturating_sub(height)
}

fn services_table(frame: &mut Frame, area: Rect, app: &App, services: &[&Service]) {
    let focused = app.overview.pane == Pane::Services;
    let mut heading = Vec::new();
    if let Some((p, e)) = app.current_env() {
        heading.push(Span::styled(
            format!("{} / ", p.project.name),
            theme::muted(),
        ));
        heading.push(Span::styled(e.environment.name.clone(), theme::text()));
        let n = e.services.len();
        heading.push(Span::styled(
            format!(" · {n} service{}", if n == 1 { "" } else { "s" }),
            theme::muted(),
        ));
    }
    let right = if app.overview.filtering || !app.overview.filter.is_empty() {
        let mut spans = vec![
            Span::styled("/ ", theme::accent()),
            Span::styled(app.overview.filter.clone(), theme::text()),
        ];
        if app.overview.filtering {
            spans.push(Span::styled("▏", theme::accent()));
        }
        Line::from(spans)
    } else {
        Line::styled("filter: all", theme::muted())
    };
    let filter_width = right.width();
    let block = panel(heading, Some(right), focused);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    mouse::hit(area, Hit::Pane(Pane::Services));
    if filter_width + 4 <= area.width as usize {
        mouse::span_target(
            area,
            area.width as usize - filter_width - 2,
            filter_width,
            Hit::Filter,
        );
    }

    let header =
        Row::new(["", "NAME", "SOURCE", "DOMAIN", "DEPLOYED", "CPU", "MEM"]).style(theme::muted());
    let rows: Vec<Row> = services
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = focused && i == app.overview.row;
            let stats = app
                .stats
                .get(&s.id)
                .filter(|_| s.state == ServiceState::Running);
            let deployed = match s.state {
                ServiceState::Building | ServiceState::Deploying => {
                    Span::styled(s.state.as_str(), util::state_style(s.state))
                }
                ServiceState::Failed | ServiceState::Degraded => {
                    Span::styled(s.state.as_str(), util::state_style(s.state))
                }
                _ => Span::styled(
                    s.last_deployed_at.map_or("never".into(), util::ago),
                    theme::muted(),
                ),
            };
            let source_style = if matches!(s.source, Source::Git { .. }) {
                theme::text()
            } else {
                theme::muted()
            };
            Row::new(vec![
                Cell::from(util::state_glyph(s.state, app.tick)),
                Cell::from(Span::styled(
                    s.name.clone(),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                )),
                Cell::from(Span::styled(s.source.label(), source_style)),
                Cell::from(
                    s.primary_domain
                        .clone()
                        .map_or(Span::styled("—", theme::muted()), |d| {
                            Span::styled(d, theme::text())
                        }),
                ),
                Cell::from(deployed),
                Cell::from(stats.map_or(Span::styled("—", theme::muted()), |st| {
                    Span::raw(format!("{:.0}%", st.cpu_percent))
                })),
                Cell::from(stats.map_or(Span::styled("—", theme::muted()), |st| {
                    Span::raw(util::bytes(st.mem_used))
                })),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    let empty = rows.is_empty();
    let offset = scroll_offset(app.overview.row, inner.height.saturating_sub(1) as usize);
    row_hits(inner, rows.len(), offset, Hit::Service);
    let table = Table::new(
        rows.into_iter().skip(offset),
        [
            Constraint::Length(2),
            Constraint::Length(16),
            Constraint::Fill(3),
            Constraint::Fill(2),
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .column_spacing(2);
    frame.render_widget(table, inner);
    if empty && inner.height > 1 {
        let row = Rect::new(inner.x, inner.y + 1, inner.width, 1);
        if app.overview.filter.is_empty() && !app.overview.filtering {
            mouse::hints(frame, row, &[("n", "add a service")], Alignment::Left);
        } else {
            frame.render_widget(
                Paragraph::new("nothing matches the filter").style(theme::muted()),
                row,
            );
        }
    }
}

fn kv<'a>(key: &'a str, value: Vec<Span<'a>>, key_w: usize) -> Line<'a> {
    let mut spans = vec![Span::styled(format!("{key:<key_w$}"), theme::muted())];
    spans.extend(value);
    Line::from(spans)
}

fn service_lines<'a>(
    app: &App,
    detail: Option<&'a ServiceDetail>,
    service: &'a Service,
) -> Vec<Line<'a>> {
    let w = 10;
    let mut lines = Vec::new();
    let source = match &service.source {
        Source::Git { .. } => {
            let built = detail.is_some() && service.live_deployment_id.is_some();
            vec![
                Span::styled(service.source.label(), theme::text()),
                Span::styled(
                    if built {
                        "  built on deploy"
                    } else {
                        "  not built yet"
                    },
                    theme::muted(),
                ),
            ]
        }
        Source::Image { image } => vec![Span::styled(image.clone(), theme::text())],
        Source::Database { kind, version } => {
            vec![
                Span::styled(format!("{kind} {version}"), theme::text()),
                Span::styled(format!("  :{}", kind.port()), theme::muted()),
            ]
        }
    };
    lines.push(kv("source", source, w));
    let Some(d) = detail else {
        lines.push(Line::styled("loading…", theme::muted()));
        return lines;
    };
    if let Some(info) = &d.database {
        let access = match info.public_port {
            Some(p) => vec![
                Span::styled(format!("public :{p}"), theme::text()),
                Span::styled(
                    format!("  {}", app.server_address(&d.server)),
                    theme::muted(),
                ),
            ],
            None => vec![Span::styled("internal only", theme::muted())],
        };
        lines.push(kv("access", access, w));
        let schedule = app
            .schedules
            .get(&service.id)
            .map(|s| format!("  {}", database::schedule_summary(s)))
            .unwrap_or_default();
        let last = app
            .backups
            .get(&service.id)
            .and_then(|b| database::last_good(b));
        let backup = match last {
            Some(b) => Span::styled(util::ago(b.created_at), theme::text()),
            None => Span::styled("no backups", theme::muted()),
        };
        lines.push(kv(
            "backup",
            vec![backup, Span::styled(schedule, theme::muted())],
            w,
        ));
    } else {
        route_lines(&mut lines, d, service, w);
    }
    if d.volumes.is_empty() {
        lines.push(kv("volumes", vec![Span::styled("none", theme::muted())], w));
    }
    for (i, v) in d.volumes.iter().enumerate() {
        lines.push(kv(
            if i == 0 { "volumes" } else { "" },
            vec![
                Span::styled(v.name.clone(), theme::text()),
                Span::styled(format!("  {}", v.mount_path), theme::muted()),
            ],
            w,
        ));
    }
    let secrets = d.env.iter().filter(|v| v.secret).count();
    lines.push(kv(
        "env",
        vec![
            Span::styled(format!("{} vars", d.env.len()), theme::text()),
            Span::styled(
                if secrets > 0 {
                    format!("  {secrets} secret")
                } else {
                    String::new()
                },
                theme::muted(),
            ),
        ],
        w,
    ));
    let restarts = d
        .container
        .as_ref()
        .map_or(String::new(), |c| format!("  {} restarts", c.restart_count));
    lines.push(kv(
        "restart",
        vec![
            Span::styled(service.restart.as_str(), theme::text()),
            Span::styled(restarts, theme::muted()),
        ],
        w,
    ));
    lines.push(kv(
        "server",
        vec![Span::styled(d.server.name.clone(), theme::text())],
        w,
    ));
    lines
}

/// Routes and the health check of an app service.
fn route_lines<'a>(lines: &mut Vec<Line<'a>>, d: &ServiceDetail, service: &Service, w: usize) {
    if d.domains.is_empty() {
        let port = service
            .port
            .map_or("no port".into(), |p| format!(":{p} internal"));
        lines.push(kv(
            "route",
            vec![Span::styled(
                format!("{} {port}", service.name),
                theme::muted(),
            )],
            w,
        ));
    }
    for (i, dom) in d.domains.iter().enumerate() {
        let scheme = if dom.tls { "https" } else { "http" };
        let port = dom
            .port
            .or(service.port)
            .map_or(String::new(), |p| format!("  → :{p}"));
        lines.push(kv(
            if i == 0 { "route" } else { "" },
            vec![
                Span::styled(format!("{scheme}://{}", dom.host), theme::text()),
                Span::styled(port, theme::muted()),
            ],
            w,
        ));
    }
    let health = match &service.health_path {
        Some(p) if !p.is_empty() => vec![
            Span::styled(p.clone(), theme::text()),
            Span::styled("  checked before switch", theme::muted()),
        ],
        _ => vec![Span::styled("container must stay up 5s", theme::muted())],
    };
    lines.push(kv("health", health, w));
}

fn charts(frame: &mut Frame, area: Rect, stats: Option<&ServiceStats>) {
    if area.height < 5 {
        return;
    }
    let [rule, cpu_h, cpu, mem_h, mem] = Layout::vertical([Constraint::Length(1); 5]).areas(Rect {
        y: area.bottom().saturating_sub(5),
        height: 5,
        ..area
    });
    frame.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(theme::fg(theme::LINE)),
        rule,
    );
    let Some(st) = stats.filter(|s| !s.cpu_history.is_empty()) else {
        frame.render_widget(
            Paragraph::new("no samples yet").style(theme::muted()),
            cpu_h,
        );
        return;
    };
    let cpu_hist: Vec<f64> = st.cpu_history.iter().map(|v| f64::from(*v)).collect();
    let peak = cpu_hist.iter().copied().fold(0.0, f64::max);
    frame.render_widget(
        Paragraph::new("cpu · last hour").style(theme::muted()),
        cpu_h,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{:.0}%", st.cpu_percent), theme::text()),
            Span::styled(format!("  peak {peak:.0}%"), theme::muted()),
        ]))
        .right_aligned(),
        cpu_h,
    );
    frame.render_widget(
        Paragraph::new(util::sparkline(
            &cpu_hist,
            area.width as usize,
            Some(peak.max(10.0)),
        ))
        .style(theme::fg(theme::OK)),
        cpu,
    );
    let mem_hist: Vec<f64> = st.mem_history.iter().map(|v| *v as f64).collect();
    frame.render_widget(Paragraph::new("memory").style(theme::muted()), mem_h);
    let limit = if st.mem_limit > 0 {
        format!(" / {}", util::bytes(st.mem_limit))
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(util::bytes(st.mem_used), theme::text()),
            Span::styled(limit, theme::muted()),
        ]))
        .right_aligned(),
        mem_h,
    );
    let top = mem_hist.iter().copied().fold(0.0, f64::max) * 1.25;
    frame.render_widget(
        Paragraph::new(util::sparkline(&mem_hist, area.width as usize, Some(top)))
            .style(theme::fg(theme::INFO)),
        mem,
    );
}

fn service_detail(frame: &mut Frame, area: Rect, app: &App, s: &Service) {
    let block = panel(
        title(&s.name, false),
        Some(Line::styled(s.state.as_str(), util::state_style(s.state))),
        false,
    );
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let detail = app.details.get(&s.id);
    domain_targets(inner, detail);
    frame.render_widget(Paragraph::new(service_lines(app, detail, s)), inner);
    charts(frame, inner, app.stats.get(&s.id));
}

fn domain_targets(area: Rect, detail: Option<&ServiceDetail>) {
    let Some(detail) = detail.filter(|d| d.database.is_none()) else {
        return;
    };
    for (i, domain) in detail
        .domains
        .iter()
        .take(area.height.saturating_sub(1) as usize)
        .enumerate()
    {
        let row = Rect::new(area.x, area.y + 1 + i as u16, area.width, 1);
        let width = Span::raw(crate::keys::domain_url(domain)).width();
        mouse::span_target(row, 10, width, Hit::Domain(i));
    }
}

fn activity_panel(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel(title("Activity", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    if app.activity.is_empty() {
        frame.render_widget(Paragraph::new("quiet so far").style(theme::muted()), inner);
        return;
    }
    let height = inner.height as usize;
    let offset = app
        .overview
        .activity_offset
        .min(app.activity.len().saturating_sub(height));
    mouse::hit(area, Hit::List(List::Activity(height)));
    let lines: Vec<Line> = app
        .activity
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, a)| {
            if a.service_id
                .as_ref()
                .is_some_and(|id| app.all_services().any(|s| s.id == *id))
            {
                mouse::hit(
                    Rect::new(inner.x, inner.y + (i - offset) as u16, inner.width, 1),
                    Hit::Activity(i),
                );
            }
            let name = a.service_name.clone().unwrap_or_default();
            let room = (inner.width as usize).saturating_sub(7 + 12);
            let msg = truncate(&a.message, room);
            let detail_room = room.saturating_sub(msg.chars().count() + 2);
            let detail = a
                .detail
                .as_deref()
                .map(|d| format!("  {}", truncate(d, detail_room)))
                .unwrap_or_default();
            Line::from(vec![
                Span::styled(format!("{}  ", util::clock(a.at)), theme::muted()),
                Span::styled(format!("{:<11} ", truncate(&name, 11)), theme::text()),
                Span::styled(msg, util::level_style(a.level)),
                Span::styled(
                    if detail_room > 3 {
                        detail
                    } else {
                        String::new()
                    },
                    theme::muted(),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

// Service

fn service(frame: &mut Frame, area: Rect, app: &App) {
    let Some(view) = &app.service else { return };
    let service = view
        .detail
        .as_ref()
        .map(|d| &d.service)
        .or_else(|| app.all_services().find(|s| s.id == view.id));
    let [head, tabs, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .spacing(1)
    .areas(area);
    let mut spans = Vec::new();
    if let Some(d) = &view.detail {
        spans.push(Span::styled(
            format!(" {} / {} / ", d.project.name, d.environment.name),
            theme::muted(),
        ));
    }
    if let Some(s) = service {
        spans.push(Span::styled(s.name.clone(), theme::text()));
        spans.push(Span::raw("  "));
        spans.push(util::state_glyph(s.state, app.tick));
        spans.push(Span::styled(
            format!(" {}", s.state.as_str()),
            util::state_style(s.state),
        ));
        let live = view
            .deployments
            .iter()
            .find(|d| d.status == DeploymentStatus::Live);
        let mut meta = Vec::new();
        let info = view.info();
        if let Some(info) = info {
            meta.push(format!("{} {}", info.kind, info.version));
        } else if let Some(d) = live {
            meta.push(
                d.commit
                    .as_deref()
                    .map_or(format!("#{}", d.number), |c| util::short_sha(c).to_string()),
            );
        }
        if let Some(c) = view.detail.as_ref().and_then(|d| d.container.as_ref())
            && let Some(at) = c.started_at
            && s.state == ServiceState::Running
        {
            meta.push(format!("up {}", util::ago(at).trim_end_matches(" ago")));
        }
        if let Some(info) = info {
            meta.push(match info.public_port {
                Some(p) => format!("public :{p}"),
                None => "internal only".into(),
            });
        }
        if !meta.is_empty() {
            spans.push(Span::styled(
                format!("   {}", meta.join(" · ")),
                theme::muted(),
            ));
        }
        if info.is_none()
            && let Some(domain) = view.detail.as_ref().and_then(|d| d.domains.first())
        {
            let separator = if meta.is_empty() { "   " } else { " · " };
            spans.push(Span::styled(separator, theme::muted()));
            let url = Span::styled(crate::keys::domain_url(domain), theme::muted());
            mouse::span_target(
                head,
                spans.iter().map(Span::width).sum(),
                url.width(),
                Hit::Domain(0),
            );
            spans.push(url);
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), head);

    let mut tab_spans = vec![Span::raw(" ")];
    let mut offset = 1;
    for t in view.tabs() {
        let w = Span::raw(t.title()).width();
        mouse::span_target(tabs, offset, w, Hit::Tab(t));
        offset += w + 4;
        let style = if t == view.tab {
            theme::accent().add_modifier(ratatui::style::Modifier::UNDERLINED)
        } else {
            theme::muted()
        };
        tab_spans.push(Span::styled(t.title(), style));
        tab_spans.push(Span::raw("    "));
    }
    frame.render_widget(Paragraph::new(Line::from(tab_spans)), tabs);

    match view.tab {
        Tab::Overview => service_overview(frame, body, app, view),
        Tab::Deployments => deployments_tab(frame, body, app, view),
        Tab::Logs => logs_tab(frame, body, app, view),
        Tab::Database => database_tab(frame, body, app, view),
        Tab::Backups => backups_tab(frame, body, app, view),
        Tab::Env => env_tab(frame, body, view),
        Tab::Domains => domains_tab(frame, body, view),
        Tab::Volumes => volumes_tab(frame, body, view),
        Tab::Settings => settings_tab(frame, body, view),
    }
}

fn with_rail(area: Rect) -> (Rect, Rect) {
    let [main, rail] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(42)])
        .spacing(1)
        .areas(area);
    (main, rail)
}

const CONTAINER_H: u16 = 9;

fn rail(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let [c_area, d_area] = Layout::vertical([Constraint::Length(CONTAINER_H), Constraint::Fill(1)])
        .spacing(1)
        .areas(area);
    container_panel(frame, c_area, view);
    deployments_panel(frame, d_area, app, view);
}

fn container_panel(frame: &mut Frame, c_area: Rect, view: &ServiceView) {
    let block = panel(title("Container", false), None, false);
    let inner = inside(&block, c_area);
    frame.render_widget(block, c_area);
    let lines = match view.detail.as_ref().and_then(|d| d.container.as_ref()) {
        Some(c) => vec![
            kv(
                "id",
                vec![Span::styled(
                    c.id.chars().take(12).collect::<String>(),
                    theme::text(),
                )],
                10,
            ),
            kv(
                "image",
                vec![Span::styled(
                    truncate(&c.image, (inner.width as usize).saturating_sub(10)),
                    theme::text(),
                )],
                10,
            ),
            kv(
                "started",
                vec![
                    Span::styled(c.started_at.map_or("—".into(), util::clock), theme::text()),
                    Span::styled(
                        c.started_at
                            .map_or(String::new(), |t| format!("  {}", util::ago(t))),
                        theme::muted(),
                    ),
                ],
                10,
            ),
            kv(
                "state",
                vec![
                    Span::styled(c.state.clone(), theme::text()),
                    Span::styled(
                        c.exit_code
                            .filter(|c| *c != 0)
                            .map_or(String::new(), |e| format!("  exit {e}")),
                        theme::fg(theme::ERR),
                    ),
                ],
                10,
            ),
            kv(
                "restarts",
                vec![Span::styled(c.restart_count.to_string(), theme::text())],
                10,
            ),
            kv(
                "ports",
                vec![Span::styled(
                    if c.ports.is_empty() {
                        "—".into()
                    } else {
                        c.ports.join(", ")
                    },
                    theme::text(),
                )],
                10,
            ),
            kv(
                "network",
                vec![Span::styled(c.networks.join(", "), theme::text())],
                10,
            ),
        ],
        None => vec![Line::styled("no container running", theme::muted())],
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn deployments_panel(frame: &mut Frame, d_area: Rect, app: &App, view: &ServiceView) {
    let block = panel(title("Deployments", false), None, false);
    let inner = inside(&block, d_area);
    frame.render_widget(block, d_area);
    let height = inner.height as usize;
    mouse::hit(d_area, Hit::List(List::Deployments));
    let offset = scroll_offset(
        view.deployment_row
            .min(view.deployments.len().saturating_sub(1)),
        height,
    );
    let lines: Vec<Line> = view
        .deployments
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, d)| {
            mouse::hit(
                Rect::new(inner.x, inner.y + (i - offset) as u16, inner.width, 1),
                Hit::DeployRow(i),
            );
            let label = d
                .commit
                .as_deref()
                .map_or(format!("#{}", d.number), |c| util::short_sha(c).to_string());
            let status = match d.status {
                DeploymentStatus::Superseded => String::new(),
                s => s.as_str().to_string(),
            };
            Line::from(vec![
                util::deployment_glyph(d.status, app.tick),
                Span::raw(" "),
                Span::styled(format!("{label:<9}"), theme::text()),
                Span::styled(format!("{status:<11}"), util::deployment_style(d.status)),
                Span::styled(
                    util::ago(d.created_at).trim_end_matches(" ago").to_string(),
                    theme::muted(),
                ),
            ])
            .style(if i == view.deployment_row {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    if lines.is_empty() {
        if !view.logs.searching {
            mouse::hints(
                frame,
                inner,
                &[("d", "deploy this service")],
                Alignment::Left,
            );
        }
        return;
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn service_overview(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let (main, rail_area) = with_rail(area);
    if let Some(d) = &view.detail {
        let block = panel(title("Overview", true), None, true);
        let inner = inside(&block, main).inner(Margin::new(1, 1));
        frame.render_widget(block, main);
        domain_targets(inner, Some(d));
        frame.render_widget(
            Paragraph::new(service_lines(app, Some(d), &d.service)),
            inner,
        );
        charts(frame, inner, app.stats.get(&view.id));
    }
    rail(frame, rail_area, app, view);
}

fn hint_at(frame: &mut Frame, area: Rect, column: u16, row: u16, key: &str, label: &str) {
    let rect = Rect::new(
        area.x + column,
        area.y + row,
        area.width.saturating_sub(column),
        1,
    )
    .intersection(area);
    mouse::hints(frame, rect, &[(key, label)], Alignment::Left);
}

fn rule_line(width: u16) -> Line<'static> {
    Line::styled("─".repeat(width as usize), theme::fg(theme::LINE))
}

fn database_tab(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let (main, rail_area) = with_rail(area);
    let (Some(info), Some(d)) = (view.info(), view.detail.as_ref()) else {
        let block = panel(title("Connection", true), None, true);
        let inner = inside(&block, main);
        frame.render_widget(block, main);
        frame.render_widget(
            Paragraph::new(format!("{} loading", util::spinner(app.tick))).style(theme::muted()),
            inner,
        );
        return;
    };
    let block = panel(
        title("Connection", true),
        Some(Line::styled(
            format!("{} {}", info.kind, info.version),
            theme::muted(),
        )),
        true,
    );
    let inner = inside(&block, main).inner(Margin::new(1, 1));
    frame.render_widget(block, main);

    let w = 14;
    let env = d.environment.name.clone();
    let mut lines = vec![
        kv(
            "engine",
            vec![
                Span::styled(info.kind.image(), theme::text()),
                Span::styled(format!("  {}", info.version), theme::muted()),
            ],
            w,
        ),
        kv(
            "host",
            vec![
                Span::styled(info.host.clone(), theme::text()),
                Span::styled(
                    format!("  service name · reachable inside {env}"),
                    theme::muted(),
                ),
            ],
            w,
        ),
        kv(
            "port",
            vec![Span::styled(info.port.to_string(), theme::text())],
            w,
        ),
    ];
    if let Some(user) = &info.username {
        lines.push(kv(
            "user",
            vec![Span::styled(user.clone(), theme::text())],
            w,
        ));
    }
    if let Some(db) = &info.database {
        lines.push(kv(
            "database",
            vec![Span::styled(db.clone(), theme::text())],
            w,
        ));
    }
    let password = if view.reveal {
        Span::styled(info.password.clone(), theme::text())
    } else {
        Span::styled("•".repeat(16), theme::muted())
    };
    let pw_w = password.width() as u16;
    let pw_y = lines.len() as u16;
    lines.push(kv("password", vec![password], w));
    lines.push(Line::raw(""));
    lines.push(rule_line(inner.width));
    lines.push(Line::raw(""));
    let url_w = (inner.width as usize).saturating_sub(w + 8);
    let internal_y = lines.len() as u16;
    lines.push(kv(
        "internal",
        vec![Span::styled(
            truncate(
                &database::masked(&info.internal_url, &info.password, view.reveal),
                url_w,
            ),
            theme::text(),
        )],
        w,
    ));
    let public = match (&info.public_url, info.public_port) {
        (Some(url), _) => Span::styled(
            truncate(&database::masked(url, &info.password, view.reveal), url_w),
            theme::text(),
        ),
        (None, Some(p)) => Span::styled(
            format!("published on :{p} · server address unknown"),
            theme::muted(),
        ),
        (None, None) => Span::styled("not exposed", theme::muted()),
    };
    let public_width = public.width();
    lines.push(kv("public", vec![public], w));
    let internal_width = lines[internal_y as usize].spans[1].width();
    frame.render_widget(Paragraph::new(lines), inner);

    let w = w as u16;
    let rows = lines_of(inner, internal_y + 2);
    let value = |row: u16, width: u16| {
        Rect::new(inner.x + w, inner.y + row, width, 1).intersection(rows[row as usize])
    };
    mouse::key_target(value(pw_y, pw_w), "v");
    hint_at(
        frame,
        inner,
        w + pw_w + 3,
        pw_y,
        "v",
        if view.reveal { "hide" } else { "reveal" },
    );
    let copy_x = inner.width.saturating_sub(6);
    mouse::key_target(value(internal_y, internal_width as u16), "c");
    hint_at(frame, inner, copy_x, internal_y, "c", "copy");
    if info.public_url.is_some() {
        mouse::key_target(value(internal_y + 1, public_width as u16), "C");
        hint_at(frame, inner, copy_x, internal_y + 1, "C", "copy");
    }
    if inner.height > internal_y + 4 {
        frame.render_widget(
            Paragraph::new(format!(
                "Other services in {env} connect with the internal URL. It is this service's {}.",
                database::url_var(info.kind)
            ))
            .style(theme::muted())
            .wrap(Wrap { trim: true }),
            Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 2),
        );
    }

    let [public_a, password_a, container_a] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(CONTAINER_H),
    ])
    .spacing(1)
    .areas(rail_area);
    let block = panel(title("Public access", false), None, false);
    let inner = inside(&block, public_a);
    frame.render_widget(block, public_a);
    let lines = match info.public_port {
        Some(p) => vec![
            Line::styled(format!("● public on :{p}"), theme::fg(theme::OK)),
            Line::styled(
                format!("open to the internet at {}", app.server_address(&d.server)),
                theme::muted(),
            ),
        ],
        None => vec![
            Line::styled("○ internal only", theme::muted()),
            Line::styled(format!("only services in {env} connect"), theme::muted()),
        ],
    };
    frame.render_widget(Paragraph::new(lines), inner);
    if inner.height >= 4 {
        let label = if info.public_port.is_some() {
            "make internal again"
        } else {
            "expose on a server port"
        };
        hint_at(frame, inner, 0, 3, "p", label);
    }

    let block = panel(title("Password", false), None, false);
    let inner = inside(&block, password_a);
    frame.render_widget(block, password_a);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("replaces it inside the database", theme::muted()),
            Line::raw(""),
            Line::raw(""),
            Line::styled("services that use it need a redeploy", theme::muted()),
        ]),
        inner,
    );
    if inner.height >= 3 {
        hint_at(frame, inner, 0, 2, "R", "rotate password");
    }
    container_panel(frame, container_a, view);
}

fn backups_tab(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let (main, rail_area) = with_rail(area);
    let backups = app
        .backups
        .get(&view.id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let total: u64 = backups.iter().map(|b| b.size_bytes).sum();
    let where_of = |b: &Backup| match &b.storage_id {
        None => "local".to_string(),
        Some(id) => app.storage.name(id).unwrap_or("s3").to_string(),
    };
    let mut places: Vec<String> = backups.iter().map(where_of).collect();
    places.sort();
    places.dedup();
    let places = match places.as_slice() {
        [] => "local".to_string(),
        [one] => one.clone(),
        many => {
            let (last, rest) = many.split_last().unwrap_or((&many[0], &[]));
            format!("{} and {last}", rest.join(", "))
        }
    };
    let note = format!(
        "{} backup{} · {} · {places}",
        backups.len(),
        if backups.len() == 1 { "" } else { "s" },
        util::bytes(total)
    );
    let now = chrono::Utc::now();
    let rows: Vec<Row> = backups
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let selected = i == view.row;
            let (glyph, age, file) = match b.status {
                BackupStatus::Running => (
                    Span::styled(util::spinner(app.tick), theme::fg(theme::INFO)),
                    util::duration((now - b.created_at).num_seconds()),
                    Span::styled("backing up…", theme::fg(theme::INFO)),
                ),
                BackupStatus::Done => (
                    Span::styled("●", theme::fg(theme::OK)),
                    util::ago(b.created_at),
                    Span::styled(b.file_name.clone(), theme::text()),
                ),
                BackupStatus::Failed => (
                    Span::styled("✕", theme::fg(theme::ERR)),
                    util::ago(b.created_at),
                    Span::styled(
                        b.error.clone().unwrap_or_else(|| "failed".into()),
                        theme::fg(theme::ERR),
                    ),
                ),
            };
            let size = if b.status == BackupStatus::Done {
                Span::styled(util::bytes(b.size_bytes), theme::text())
            } else {
                Span::styled("—", theme::muted())
            };
            Row::new(vec![
                Cell::from(glyph),
                Cell::from(Line::from(vec![
                    Span::styled(
                        database::created(b.created_at, now),
                        if selected {
                            theme::accent()
                        } else {
                            theme::text()
                        },
                    ),
                    Span::styled(format!("  {age}"), theme::muted()),
                ])),
                Cell::from(Span::styled(b.trigger.as_str(), theme::muted())),
                Cell::from(size),
                Cell::from(Span::styled(
                    where_of(b),
                    if b.storage_id.is_some() {
                        theme::text()
                    } else {
                        theme::muted()
                    },
                )),
                Cell::from(file),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    ListPanel {
        heading: "Backups",
        note: &note,
        header: Row::new(["", "CREATED", "TRIGGER", "SIZE", "WHERE", "FILE"]),
        widths: vec![
            Constraint::Length(2),
            Constraint::Length(22),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(12),
            Constraint::Fill(1),
        ],
        empty: ("b", "back up now"),
    }
    .render(frame, main, rows, view.row);

    let [sched_a, _] =
        Layout::vertical([Constraint::Length(11), Constraint::Fill(1)]).areas(rail_area);
    let block = panel(title("Schedule", false), None, false);
    let inner = inside(&block, sched_a);
    frame.render_widget(block, sched_a);
    let Some(s) = app.schedules.get(&view.id) else {
        frame.render_widget(
            Paragraph::new(format!("{} loading", util::spinner(app.tick))).style(theme::muted()),
            inner,
        );
        return;
    };
    mouse::key_target(sched_a, "e");
    let w = 10;
    let local = database::local_hour(
        s.hour_utc,
        chrono::Offset::fix(chrono::Local::now().offset()),
    );
    let lines = vec![
        kv(
            "daily",
            vec![if s.enabled {
                Span::styled("● on", theme::fg(theme::OK))
            } else {
                Span::styled("○ off", theme::muted())
            }],
            w,
        ),
        kv(
            "at",
            vec![
                Span::styled(format!("{:02}:00 UTC", s.hour_utc), theme::text()),
                Span::styled(format!("  {local}"), theme::muted()),
            ],
            w,
        ),
        kv(
            "keep",
            vec![
                Span::styled(s.keep.to_string(), theme::text()),
                Span::styled("  per store", theme::muted()),
            ],
            w,
        ),
        kv(
            "store",
            vec![match &s.storage_id {
                None => Span::styled("local", theme::muted()),
                Some(id) => Span::styled(
                    app.storage.name(id).unwrap_or("s3").to_string(),
                    theme::text(),
                ),
            }],
            w,
        ),
        kv(
            "next",
            vec![match database::next_run(s, now) {
                Some(n) => Span::styled(n, theme::text()),
                None => Span::styled("—", theme::muted()),
            }],
            w,
        ),
        Line::raw(""),
        Line::raw(""),
        Line::styled("manual backups are never pruned", theme::muted()),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
    if inner.height >= 7 {
        hint_at(frame, inner, 0, 6, "e", "edit schedule");
    }
}

fn log_line<'a>(line: &'a LogLine, logs: &LogBuffer, search: &str, current: bool) -> Line<'a> {
    let mut spans = Vec::new();
    if logs.timestamps {
        spans.push(Span::styled(
            line.ts.map_or_else(|| " ".repeat(12), util::clock_ms),
            theme::muted(),
        ));
        spans.push(Span::raw("  "));
    }
    let base = match (line.stream, level_of(&line.line)) {
        (LogStream::System, _) => theme::fg(theme::INFO),
        (_, Some(Level::Error)) => theme::fg(theme::ERR),
        (_, Some(Level::Warn)) => theme::accent(),
        (_, Some(Level::Debug)) => theme::muted(),
        (LogStream::Stderr, _) => theme::text(),
        _ => theme::text(),
    };
    let text = line.line.replace('\t', "    ");
    if search.is_empty() {
        spans.push(Span::styled(text, base));
    } else {
        let lower = text.to_lowercase();
        let needle = search.to_lowercase();
        let mut pos = 0;
        while let Some(found) = lower[pos..].find(&needle) {
            let start = pos + found;
            let end = start + needle.len();
            if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                break;
            }
            spans.push(Span::styled(text[pos..start].to_string(), base));
            let hit = if current {
                theme::chip()
            } else {
                theme::accent().add_modifier(ratatui::style::Modifier::UNDERLINED)
            };
            spans.push(Span::styled(text[start..end].to_string(), hit));
            pos = end;
        }
        spans.push(Span::styled(text[pos..].to_string(), base));
    }
    Line::from(spans)
}

/// Draw a log buffer into `area`, honoring follow, offset, wrap and filters.
fn log_view(
    frame: &mut Frame,
    area: Rect,
    logs: &LogBuffer,
    empty: &str,
    viewport: &std::cell::Cell<usize>,
) {
    viewport.set(area.height as usize);
    mouse::hit(area, Hit::Logs);
    let visible = logs.visible();
    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(empty.to_string()).style(theme::muted()),
            area,
        );
        return;
    }
    let matches = logs.matches();
    let current = matches.get(logs.match_idx).copied();
    let height = area.height as usize;
    let (start, end) = if let Some(start) = logs.step_anchor {
        (start, (start + height).min(visible.len()))
    } else {
        let end = visible.len() - logs.offset.min(visible.len().saturating_sub(height));
        (end.saturating_sub(height), end)
    };
    let lines: Vec<Line> = visible[start..end]
        .iter()
        .enumerate()
        .map(|(n, i)| {
            log_line(
                &logs.lines[*i],
                logs,
                &logs.search,
                Some(start + n) == current,
            )
        })
        .collect();
    let mut p = Paragraph::new(Text::from(lines));
    if logs.wrap {
        p = p.wrap(Wrap { trim: false });
        if logs.step_anchor.is_none() {
            let rendered = p.line_count(area.width);
            p = p.scroll((rendered.saturating_sub(height) as u16, 0));
        }
    }
    frame.render_widget(p, area);
}

fn logs_tab(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let (main, rail_area) = with_rail(area);
    let logs = &view.logs;
    let state = if logs.follow {
        Span::styled(" · following", theme::fg(theme::OK))
    } else {
        Span::styled(" · paused", theme::accent())
    };
    let mut right = vec![];
    if logs.errors_only {
        right.push(Span::styled("errors only · ", theme::accent()));
    }
    right.push(Span::styled(
        format!(
            "stdout + stderr · {} line{}",
            logs.lines.len(),
            if logs.lines.len() == 1 { "" } else { "s" }
        ),
        theme::muted(),
    ));
    let block = panel(
        vec![Span::styled("Logs", theme::text()), state],
        Some(Line::from(right)),
        true,
    );
    let inner = inside(&block, main);
    frame.render_widget(block, main);
    mouse::hit(main, Hit::Logs);
    let [lines_a, bar] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).areas(inner);
    let empty = match &logs.ended {
        Some(e) => format!("no logs · {e}"),
        None => format!("{} waiting for output", util::spinner(app.tick)),
    };
    log_view(frame, lines_a, logs, &empty, &app.log_viewport);
    if !bar.is_empty() {
        let [rule, line] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(bar);
        frame.render_widget(
            Paragraph::new("─".repeat(rule.width as usize)).style(theme::fg(theme::LINE)),
            rule,
        );
        mouse::hit(line, Hit::Search);
        let mut left = vec![Span::styled("/ ", theme::accent())];
        let mut left_area = line;
        if logs.search.is_empty() && !logs.searching {
            left.push(Span::styled("search logs", theme::muted()));
        } else {
            left.push(Span::styled(logs.search.clone(), theme::text()));
        }
        if logs.searching {
            left.push(Span::styled("▏", theme::accent()));
        }
        if !logs.search.is_empty() && !logs.searching {
            let n = logs.matches().len();
            let info = if n == 0 {
                "no matches".to_string()
            } else {
                format!("{} of {n} matches", logs.match_idx.min(n - 1) + 1)
            };
            let hints = [("n", "next"), ("N", "prev")];
            let width = mouse::hints_width(&hints) as u16;
            if n > 0 && line.width >= width + 20 {
                mouse::hints(frame, line, &hints, Alignment::Right);
                left_area.width -= width + 3;
            }
            if left_area.width >= Span::raw(&info).width() as u16 + 10 {
                frame.render_widget(
                    Paragraph::new(info.clone())
                        .style(theme::muted())
                        .right_aligned(),
                    left_area,
                );
                left_area.width -= Span::raw(info).width() as u16 + 2;
            }
        } else if logs.ended.is_some() && !logs.searching && line.width >= 34 {
            let hints = [("ctrl+r", "reconnect")];
            mouse::hints(frame, line, &hints, Alignment::Right);
            left_area.width -= mouse::hints_width(&hints) as u16 + 3;
        }
        frame.render_widget(Paragraph::new(Line::from(left)), left_area);
    }
    rail(frame, rail_area, app, view);
}

fn deployments_tab(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let block = panel(
        title("Deployments", true),
        Some(Line::styled(
            format!("{} total", view.deployments.len()),
            theme::muted(),
        )),
        true,
    );
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    mouse::hit(area, Hit::List(List::Service));
    let header = Row::new([
        "", "#", "STATUS", "COMMIT", "MESSAGE", "TRIGGER", "STARTED", "TOOK",
    ])
    .style(theme::muted());
    let rows: Vec<Row> = view
        .deployments
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let selected = i == view.row;
            let took = d.finished_at.map_or_else(
                || {
                    if d.status.is_finished() {
                        "—".into()
                    } else {
                        util::duration((chrono::Utc::now() - d.created_at).num_seconds())
                    }
                },
                |f| util::duration((f - d.created_at).num_seconds()),
            );
            Row::new(vec![
                Cell::from(util::deployment_glyph(d.status, app.tick)),
                Cell::from(Span::styled(
                    d.number.to_string(),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                )),
                Cell::from(Span::styled(
                    d.status.as_str(),
                    util::deployment_style(d.status),
                )),
                Cell::from(d.commit.as_deref().map_or("—", util::short_sha).to_string()),
                Cell::from(Span::styled(
                    d.error
                        .clone()
                        .filter(|_| d.status == DeploymentStatus::Failed)
                        .or_else(|| d.commit_message.clone())
                        .or_else(|| d.image.clone())
                        .unwrap_or_default(),
                    if d.status == DeploymentStatus::Failed {
                        theme::fg(theme::ERR)
                    } else {
                        theme::muted()
                    },
                )),
                Cell::from(Span::styled(d.trigger.clone(), theme::muted())),
                Cell::from(Span::styled(util::ago(d.created_at), theme::muted())),
                Cell::from(Span::styled(took, theme::muted())),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    if rows.is_empty() {
        mouse::hints(
            frame,
            inner,
            &[("d", "deploy this service")],
            Alignment::Left,
        );
        return;
    }
    let offset = scroll_offset(view.row, inner.height.saturating_sub(1) as usize);
    row_hits(inner, rows.len(), offset, Hit::Row);
    let table = Table::new(
        rows.into_iter().skip(offset),
        [
            Constraint::Length(2),
            Constraint::Length(4),
            Constraint::Length(11),
            Constraint::Length(8),
            Constraint::Fill(1),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .column_spacing(2);
    frame.render_widget(table, inner);
}

/// A titled table panel used by the env, domains and volumes tabs.
struct ListPanel<'a> {
    heading: &'a str,
    note: &'a str,
    header: Row<'a>,
    widths: Vec<Constraint>,
    empty: (&'a str, &'a str),
}

impl ListPanel<'_> {
    fn render(self, frame: &mut Frame, area: Rect, rows: Vec<Row>, row: usize) {
        let block = panel(
            title(self.heading, true),
            Some(Line::styled(self.note.to_string(), theme::muted())),
            true,
        );
        let inner = inside(&block, area);
        frame.render_widget(block, area);
        mouse::hit(area, Hit::List(List::Service));
        if rows.is_empty() {
            mouse::hints(frame, inner, &[self.empty], Alignment::Left);
            return;
        }
        let offset = scroll_offset(row, inner.height.saturating_sub(1) as usize);
        row_hits(inner, rows.len(), offset, Hit::Row);
        let table = Table::new(rows.into_iter().skip(offset), self.widths)
            .header(self.header.style(theme::muted()))
            .column_spacing(2);
        frame.render_widget(table, inner);
    }
}

fn env_tab(frame: &mut Frame, area: Rect, view: &ServiceView) {
    let vars = view
        .detail
        .as_ref()
        .map(|d| d.env.as_slice())
        .unwrap_or_default();
    let rows = vars
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let selected = i == view.row;
            let value = if v.secret && !view.reveal {
                "•".repeat(v.value.chars().count().clamp(6, 16))
            } else {
                v.value.clone()
            };
            Row::new(vec![
                Cell::from(Span::styled(
                    v.key.clone(),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                )),
                Cell::from(Span::styled(
                    value,
                    if v.secret && !view.reveal {
                        theme::muted()
                    } else {
                        theme::text()
                    },
                )),
                Cell::from(Span::styled(
                    if v.secret { "secret" } else { "" },
                    theme::muted(),
                )),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    ListPanel {
        heading: "Environment variables",
        note: "changes apply on next deploy",
        header: Row::new(["KEY", "VALUE", ""]),
        widths: vec![
            Constraint::Length(28),
            Constraint::Fill(1),
            Constraint::Length(8),
        ],
        empty: ("a", "add a variable"),
    }
    .render(frame, area, rows, view.row);
}

fn domains_tab(frame: &mut Frame, area: Rect, view: &ServiceView) {
    let (doms, port) = view
        .detail
        .as_ref()
        .map_or((&[][..], None), |d| (d.domains.as_slice(), d.service.port));
    let rows = doms
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let selected = i == view.row;
            let target = d.port.or(port).map_or("—".into(), |p| format!(":{p}"));
            Row::new(vec![
                Cell::from(Span::styled(
                    d.host.clone(),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                )),
                Cell::from(Span::styled(
                    if d.tls { "https · auto cert" } else { "http" },
                    if d.tls {
                        theme::fg(theme::OK)
                    } else {
                        theme::muted()
                    },
                )),
                Cell::from(target),
                Cell::from(Span::styled(util::ago(d.created_at), theme::muted())),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    ListPanel {
        heading: "Domains",
        note: "routed by the proxy",
        header: Row::new(["HOST", "PROTOCOL", "TARGET", "ADDED"]),
        widths: vec![
            Constraint::Fill(1),
            Constraint::Length(18),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
        empty: ("a", "add a domain, then point its A record at this server"),
    }
    .render(frame, area, rows, view.row);
}

fn volumes_tab(frame: &mut Frame, area: Rect, view: &ServiceView) {
    let vols = view
        .detail
        .as_ref()
        .map(|d| d.volumes.as_slice())
        .unwrap_or_default();
    let rows = vols
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let selected = i == view.row;
            Row::new(vec![
                Cell::from(Span::styled(
                    v.name.clone(),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                )),
                Cell::from(v.mount_path.clone()),
                Cell::from(Span::styled(
                    format!("{}-{}-{}", brand::PREFIX, v.service_id, v.name),
                    theme::muted(),
                )),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    ListPanel {
        heading: "Volumes",
        note: "persist across deploys",
        header: Row::new(["NAME", "MOUNT", "DOCKER VOLUME"]),
        widths: vec![
            Constraint::Length(18),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ],
        empty: ("a", "add a volume"),
    }
    .render(frame, area, rows, view.row);
}

fn settings_tab(frame: &mut Frame, area: Rect, view: &ServiceView) {
    let block = panel(
        title("Settings", true),
        Some(Line::styled("⏎ to edit", theme::muted())),
        true,
    );
    let inner = inside(&block, area).inner(Margin::new(1, 1));
    frame.render_widget(block, area);
    let Some(d) = &view.detail else { return };
    if area.width >= 23 {
        mouse::key_target(Rect::new(area.right() - 11, area.y, 9, 1), "⏎");
    }
    let s = &d.service;
    let w = 14;
    let val = |v: String| vec![Span::styled(v, theme::text())];
    let opt = |v: &Option<String>, none: &str| match v {
        Some(v) if !v.is_empty() => vec![Span::styled(v.clone(), theme::text())],
        _ => vec![Span::styled(none.to_string(), theme::muted())],
    };
    let mut lines = vec![kv("name", val(s.name.clone()), w)];
    match &s.source {
        Source::Git {
            repo,
            branch,
            dockerfile,
            context,
        } => {
            lines.push(kv("repository", val(repo.clone()), w));
            lines.push(kv("branch", val(branch.clone()), w));
            lines.push(kv(
                "dockerfile",
                val(format!("{context}/{dockerfile}")
                    .trim_start_matches("./")
                    .to_string()),
                w,
            ));
        }
        Source::Image { image } => lines.push(kv("image", val(image.clone()), w)),
        Source::Database { kind, version } => {
            lines.push(kv("database", val(format!("{kind} {version}")), w))
        }
    }
    lines.extend([
        kv(
            "port",
            s.port
                .map_or(vec![Span::styled("none", theme::muted())], |p| {
                    val(p.to_string())
                }),
            w,
        ),
        kv("command", opt(&s.command, "image default"), w),
        kv(
            "health path",
            opt(&s.health_path, "none · container must stay up 5s"),
            w,
        ),
        kv("restart", val(s.restart.as_str().into()), w),
        kv("server", val(d.server.name.clone()), w),
        kv("id", vec![Span::styled(s.id.clone(), theme::muted())], w),
        Line::raw(""),
        Line::from(vec![
            Span::styled("X", theme::fg(theme::ERR)),
            Span::styled("  delete this service and its containers", theme::muted()),
        ]),
    ]);
    let delete_row = lines.len() as u16 - 1;
    if delete_row < inner.height {
        mouse::key_target(
            Rect::new(
                inner.x,
                inner.y + delete_row,
                (lines[delete_row as usize].width() as u16).min(inner.width),
                1,
            ),
            "X",
        );
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

// Deploy

fn deploy(frame: &mut Frame, area: Rect, app: &App) {
    let Some(view) = &app.deploy else { return };
    let d = &view.deployment;
    let detail = app.service.as_ref().and_then(|v| v.detail.as_ref());
    let [head, sub, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let body = Rect {
        y: body.y + 1,
        height: body.height.saturating_sub(1),
        ..body
    };
    let mut spans = Vec::new();
    if let Some(det) = detail {
        spans.push(Span::styled(
            format!(" {} / {} / ", det.project.name, det.environment.name),
            theme::muted(),
        ));
        spans.push(Span::styled(det.service.name.clone(), theme::text()));
        spans.push(Span::raw("  "));
    }
    spans.push(util::deployment_glyph(d.status, app.tick));
    let verb = match d.status {
        DeploymentStatus::Queued => "queued".to_string(),
        DeploymentStatus::Building | DeploymentStatus::Starting => "deploying".to_string(),
        s => s.as_str().to_string(),
    };
    spans.push(Span::styled(
        format!(" {verb} #{}", d.number),
        util::deployment_style(d.status),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), head);
    let mut meta = vec![Span::raw(" ")];
    if let Some(det) = detail {
        meta.push(Span::styled(
            format!("{}   ", det.service.source.label()),
            theme::muted(),
        ));
    }
    if let Some(c) = &d.commit {
        meta.push(Span::styled(
            format!("{}  ", util::short_sha(c)),
            theme::text(),
        ));
    }
    if let Some(m) = &d.commit_message {
        meta.push(Span::styled(
            format!("{}  ", truncate(m, 60)),
            theme::muted(),
        ));
    }
    let elapsed = d.finished_at.unwrap_or_else(chrono::Utc::now) - d.created_at;
    meta.push(Span::styled(
        format!(
            "· {} · {}",
            d.trigger,
            util::duration(elapsed.num_seconds())
        ),
        theme::muted(),
    ));
    frame.render_widget(Paragraph::new(Line::from(meta)), sub);

    let [pipe, out] = Layout::horizontal([Constraint::Length(42), Constraint::Fill(1)])
        .spacing(1)
        .areas(body);
    let block = panel(title("Pipeline", false), None, false);
    let inner = inside(&block, pipe).inner(Margin::new(0, 1));
    frame.render_widget(block, pipe);
    let show_progress = inner.height > 6;
    let step_area = Rect {
        height: inner.height - if show_progress { 4 } else { 0 },
        ..inner
    };
    let step_count = step_area.height.div_ceil(2) as usize;
    let offset = view
        .pipeline_offset
        .min(d.steps.len().saturating_sub(step_count));
    mouse::hit(pipe, Hit::List(List::Pipeline(step_count)));
    let lines: Vec<Line> = d
        .steps
        .iter()
        .skip(offset)
        .take(step_count)
        .enumerate()
        .flat_map(|(i, s)| {
            mouse::hit(
                Rect::new(step_area.x, step_area.y + i as u16 * 2, step_area.width, 1),
                Hit::Pipeline(s.kind),
            );
            let took = match (s.started_at, s.finished_at) {
                (Some(a), Some(b)) => util::duration((b - a).num_seconds()),
                (Some(a), None) if s.status == StepStatus::Running => {
                    util::duration((chrono::Utc::now() - a).num_seconds())
                }
                _ => String::new(),
            };
            let name_style = if s.status == StepStatus::Pending {
                theme::muted()
            } else {
                theme::text()
            };
            let detail = truncate(
                s.detail.as_deref().unwrap_or(""),
                (inner.width as usize).saturating_sub(22),
            );
            let pad =
                (inner.width as usize).saturating_sub(12 + detail.chars().count() + took.len());
            [
                Line::from(vec![
                    util::step_glyph(s.status, app.tick),
                    Span::raw("  "),
                    Span::styled(format!("{:<9}", s.kind.as_str()), name_style),
                    Span::styled(detail, theme::muted()),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(took, theme::muted()),
                ]),
                Line::raw(""),
            ]
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), step_area);

    let total = d
        .steps
        .iter()
        .filter(|s| s.status != StepStatus::Skipped)
        .count()
        .max(1);
    let done = d
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Done)
        .count();
    let pct = if d.status == DeploymentStatus::Live {
        100
    } else {
        done * 100 / total
    };
    if show_progress {
        let foot = Rect {
            y: inner.bottom().saturating_sub(4),
            height: 4,
            ..inner
        };
        let [rule, label, bar, note] = Layout::vertical([Constraint::Length(1); 4]).areas(foot);
        frame.render_widget(
            Paragraph::new("─".repeat(rule.width as usize)).style(theme::fg(theme::LINE)),
            rule,
        );
        frame.render_widget(Paragraph::new("progress").style(theme::muted()), label);
        frame.render_widget(
            Paragraph::new(format!("{pct}%"))
                .style(theme::text())
                .right_aligned(),
            label,
        );
        let filled = bar.width as usize * pct / 100;
        let color = match d.status {
            DeploymentStatus::Failed | DeploymentStatus::Cancelled => theme::ERR,
            DeploymentStatus::Live => theme::OK,
            _ => theme::INFO,
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("━".repeat(filled), theme::fg(color)),
                Span::styled(
                    "━".repeat(bar.width as usize - filled),
                    theme::fg(theme::LINE),
                ),
            ])),
            bar,
        );
        let msg = match (&d.error, d.status) {
            (Some(e), _) => Span::styled(truncate(e, note.width as usize), theme::fg(theme::ERR)),
            (None, DeploymentStatus::Live) => {
                Span::styled("live · old container removed", theme::fg(theme::OK))
            }
            _ => Span::styled("old container serves until healthy", theme::muted()),
        };
        frame.render_widget(Paragraph::new(msg), note);
    }

    let live = if d.status.is_finished() {
        Span::styled(" · done", theme::muted())
    } else {
        Span::styled(" · live", theme::fg(theme::INFO))
    };
    let block = panel(
        vec![Span::styled("Build output", theme::text()), live],
        Some(Line::styled(
            format!("{} lines", view.logs.lines.len()),
            theme::muted(),
        )),
        true,
    );
    let inner = inside(&block, out);
    frame.render_widget(block, out);
    mouse::hit(out, Hit::Logs);
    log_view(
        frame,
        inner,
        &view.logs,
        &format!("{} waiting for output", util::spinner(app.tick)),
        &app.log_viewport,
    );
}

// Servers

fn servers(frame: &mut Frame, area: Rect, app: &App) {
    let key = app.info.as_ref().and_then(|i| i.ssh_public_key.clone());
    let [list, key_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(if key.is_some() { 6 } else { 0 }),
    ])
    .spacing(1)
    .areas(area);
    let block = panel(
        title("Servers", true),
        Some(Line::styled(
            "deploy targets of this daemon",
            theme::muted(),
        )),
        true,
    );
    let inner = inside(&block, list);
    frame.render_widget(block, list);
    mouse::hit(list, Hit::List(List::Servers));
    let header = Row::new([
        "", "NAME", "KIND", "HOST", "KEY", "DOCKER", "PROXY", "CHECKED",
    ])
    .style(theme::muted());
    let rows: Vec<Row> = app
        .servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = i == app.server_row;
            let (glyph, style) = match s.status {
                ServerStatus::Online => ("●", theme::fg(theme::OK)),
                ServerStatus::Offline => ("✕", theme::fg(theme::ERR)),
                ServerStatus::Unknown => ("○", theme::muted()),
            };
            let host = match s.kind {
                ServerKind::Local => "this machine".to_string(),
                ServerKind::Ssh => format!(
                    "{}@{}:{}",
                    s.user.as_deref().unwrap_or("root"),
                    s.host.as_deref().unwrap_or("?"),
                    s.port.unwrap_or(22)
                ),
            };
            let key = match (s.kind, &s.ssh_key_secret_id) {
                (ServerKind::Local, _) => Span::styled("—", theme::muted()),
                (_, None) => Span::styled("daemon key", theme::muted()),
                (_, Some(id)) => Span::styled(
                    app.secrets.name(id).unwrap_or("secret").to_string(),
                    theme::text(),
                ),
            };
            let (proxy, pstyle) = match s.proxy {
                ProxyStatus::Running => ("running", theme::fg(theme::OK)),
                ProxyStatus::Stopped => ("stopped", theme::fg(theme::ERR)),
                ProxyStatus::Missing => ("not set up · p", theme::accent()),
            };
            Row::new(vec![
                Cell::from(Span::styled(glyph, style)),
                Cell::from(Span::styled(
                    s.name.clone(),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                )),
                Cell::from(Span::styled(
                    if s.kind == ServerKind::Local {
                        "local"
                    } else {
                        "ssh"
                    },
                    theme::muted(),
                )),
                Cell::from(host),
                Cell::from(key),
                Cell::from(Span::styled(
                    s.docker_version.clone().unwrap_or("—".into()),
                    theme::muted(),
                )),
                Cell::from(Span::styled(proxy, pstyle)),
                Cell::from(Span::styled(
                    s.last_checked_at.map_or("never".into(), util::ago),
                    theme::muted(),
                )),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    let offset = scroll_offset(app.server_row, inner.height.saturating_sub(1) as usize);
    let widths = [
        Constraint::Length(2),
        Constraint::Length(16),
        Constraint::Length(6),
        Constraint::Fill(1),
        Constraint::Length(16),
        Constraint::Length(10),
        Constraint::Length(15),
        Constraint::Length(10),
    ];
    row_hits(inner, rows.len(), offset, Hit::Server);
    let columns = Layout::horizontal(widths).spacing(2).split(inner);
    for (i, server) in app
        .servers
        .iter()
        .enumerate()
        .skip(offset)
        .take(inner.height.saturating_sub(1) as usize)
    {
        if server.proxy == ProxyStatus::Missing {
            let proxy = columns[6];
            let row = Rect::new(proxy.x, inner.y + 1 + (i - offset) as u16, proxy.width, 1);
            mouse::span_target(row, 0, "not set up · p".chars().count(), Hit::SetupProxy(i));
        }
    }
    frame.render_widget(
        Table::new(rows.into_iter().skip(offset), widths)
            .header(header)
            .column_spacing(2),
        inner,
    );
    if let Some(key) = key {
        let block = panel(title("Daemon SSH key", false), None, false);
        let inner = inside(&block, key_area);
        frame.render_widget(block, key_area);
        let [description, value, hints] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(
                "Add this key to ~/.ssh/authorized_keys on a server before adding it, or pick your own key:",
            )
            .style(theme::muted()),
            description,
        );
        frame.render_widget(
            Paragraph::new(key)
                .style(theme::text())
                .wrap(Wrap { trim: true }),
            value,
        );
        mouse::key_target(value, "ctrl+k");
        mouse::hints(
            frame,
            hints,
            &[("K", "change key"), ("ctrl+k", "copy key")],
            Alignment::Right,
        );
    }
}

// Overlays

fn dialog(
    frame: &mut Frame,
    area: Rect,
    width: u16,
    height: u16,
    heading: Vec<Span<'static>>,
) -> Rect {
    let rect = area.centered(
        Constraint::Length(width.min(area.width.saturating_sub(4))),
        Constraint::Length(height.min(area.height)),
    );
    mouse::modal(rect);
    frame.render_widget(Clear, rect);
    let mut spans = vec![Span::raw(" ")];
    spans.extend(heading);
    spans.push(Span::raw(" "));
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::muted())
        .style(theme::text().bg(theme::SURFACE))
        .title_top(Line::from(spans));
    let inner = block.inner(rect).inner(Margin::new(2, 1));
    frame.render_widget(block, rect);
    inner
}

fn confirm(frame: &mut Frame, area: Rect, c: &Confirm, tick: usize) {
    let width = 72u16.min(area.width.saturating_sub(4));
    let message = Paragraph::new(c.message.clone())
        .style(theme::text())
        .wrap(Wrap { trim: true });
    let msg_h = message.line_count(width.saturating_sub(6)) as u16;
    let error: Vec<Line> = c
        .error
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled("✕ ", theme::fg(theme::ERR)),
                    Span::styled(l.clone(), theme::text()),
                ])
            } else {
                Line::styled(l.clone(), theme::muted())
            }
        })
        .collect();
    let error = Paragraph::new(error).wrap(Wrap { trim: true });
    let err_h = if c.error.is_empty() {
        0
    } else {
        error.line_count(width.saturating_sub(6)) as u16 + 1
    };
    let inner = dialog(
        frame,
        area,
        width,
        msg_h + err_h + 6,
        vec![Span::styled(c.title.clone(), theme::text())],
    );
    let [msg, err, rule, keys] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(err_h),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(message, msg);
    if err_h > 0 {
        frame.render_widget(
            error,
            Rect {
                y: err.y + 1,
                height: err.height - 1,
                ..err
            },
        );
    }
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    let style = if c.danger {
        Style::new().fg(theme::BG).bg(theme::ERR)
    } else {
        theme::chip()
    };
    if let Some((since, label)) = c.busy {
        let wait = match c.action {
            ConfirmAction::RestoreBackup(..) => "the database is busy until this finishes",
            _ => "waiting for the daemon",
        };
        frame.render_widget(Paragraph::new(wait).style(theme::muted()), keys);
        let chip = format!(
            " {} {label}… {} ",
            util::spinner(tick),
            util::duration(since.elapsed().as_secs() as i64)
        );
        frame.render_widget(
            Paragraph::new(Span::styled(chip, style)).right_aligned(),
            keys,
        );
        return;
    }
    if !c.error.is_empty() {
        mouse::hints(frame, keys, &[("esc", "close")], Alignment::Left);
        return;
    }
    let remaining = action_chip(frame, keys, "y", c.verb, style);
    mouse::hints(frame, remaining, &[("esc", "keep")], Alignment::Left);
}

fn action_chip(frame: &mut Frame, area: Rect, key: &str, label: &str, style: Style) -> Rect {
    let chip = Span::styled(format!(" {key} {label} "), style);
    let width = (chip.width() as u16).min(area.width);
    let rect = Rect::new(area.right() - width, area.y, width, area.height);
    frame.render_widget(Paragraph::new(chip), rect);
    mouse::key_target(rect, key);
    Rect {
        width: area.width.saturating_sub(width + 1),
        ..area
    }
}

/// A secret shown once, with copy.
fn reveal(frame: &mut Frame, area: Rect, r: &Reveal) {
    let width = 84u16.min(area.width.saturating_sub(4));
    let text_w = width.saturating_sub(6);
    let message = Paragraph::new(r.message.clone())
        .style(theme::text())
        .wrap(Wrap { trim: true });
    let note = Paragraph::new(r.note.clone())
        .style(theme::muted())
        .wrap(Wrap { trim: true });
    let (msg_h, note_h) = (
        message.line_count(text_w) as u16,
        note.line_count(text_w) as u16,
    );
    let inner = dialog(
        frame,
        area,
        width,
        msg_h + note_h + 10,
        vec![
            Span::styled(r.title.clone(), theme::text()),
            Span::styled(format!(" · {}", r.subtitle), theme::muted()),
        ],
    );
    let [msg, _, value, _, note_a, rule, keys] = Layout::vertical([
        Constraint::Length(msg_h),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(note_h),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(message, msg);
    frame.render_widget(
        Paragraph::new(format!(" {}", r.value)).style(theme::text().bg(theme::BG)),
        value,
    );
    frame.render_widget(note, note_a);
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    let width = (Span::raw(&r.value).width() + 1).min(value.width as usize) as u16;
    mouse::key_target(Rect { width, ..value }, "c");
    let remaining = action_chip(frame, keys, "⏎", "done", theme::chip());
    mouse::hints(
        frame,
        remaining,
        &[("c", &format!("copy {}", r.what.trim_start_matches("the ")))],
        Alignment::Left,
    );
}

fn palette(frame: &mut Frame, area: Rect, p: &Palette) {
    let commands = p.commands();
    let h = commands.len() as u16 + 7;
    let inner = dialog(
        frame,
        area,
        56,
        h,
        vec![Span::styled("Command", theme::text())],
    );
    let [input, _, rows, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    mouse::hit(input, Hit::PaletteInput);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(": ", theme::accent()),
            Span::styled(p.input.clone(), theme::text()),
            Span::styled("▏", theme::accent()),
        ])),
        input,
    );
    mouse::hit(rows, Hit::List(List::Palette));
    let offset = scroll_offset(p.row, rows.height as usize);
    let mut lines = Vec::new();
    for (i, c) in commands
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows.height as usize)
    {
        let selected = i == p.row;
        let label = format!(
            " {:<width$}",
            c.label(),
            width = (inner.width as usize).saturating_sub(10)
        );
        mouse::hit(
            Rect::new(rows.x, rows.y + (i - offset) as u16, rows.width, 1),
            Hit::PaletteRow(i),
        );
        lines.push(
            Line::from(vec![
                Span::styled(
                    label,
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                ),
                Span::styled(format!("{:>8}", c.key()), theme::muted()),
            ])
            .style(if selected {
                Style::new().bg(theme::LINE)
            } else {
                Style::new()
            }),
        );
    }
    if commands.is_empty() {
        lines.push(Line::styled(" no command matches", theme::muted()));
    }
    frame.render_widget(Paragraph::new(lines), rows);
    mouse::hints(
        frame,
        hints,
        &[("⏎", "run"), ("esc", "close")],
        Alignment::Left,
    );
}

fn help(frame: &mut Frame, area: Rect, app: &App) {
    let mut global = vec![
        (":", "command palette"),
        ("S", "servers"),
        (",", "settings"),
        ("ctrl+r", "refresh"),
        ("q", "quit"),
    ];
    global.retain(|(key, _)| {
        !matches!(
            (app.screen, *key),
            (Screen::Settings, ",") | (Screen::Servers, "S")
        )
    });
    let (mut screen_keys, _) = keys_for(app);
    screen_keys.retain(|(key, _)| !matches!(*key, ":" | "esc"));
    match app.screen {
        Screen::Overview => screen_keys.extend([
            ("tab", "switch pane"),
            ("N", "new project"),
            ("E", "new environment"),
            ("x", "delete selected"),
        ]),
        Screen::Service => {
            screen_keys.push(("⇧tab", "previous tab"));
            if app.service.as_ref().is_some_and(|v| v.tab == Tab::Logs) {
                screen_keys.extend([
                    ("n", "next match"),
                    ("N", "previous match"),
                    ("g", "top"),
                    ("G", "bottom"),
                ]);
            }
        }
        _ => {}
    }
    let height = (global.len().max(screen_keys.len()) + 9) as u16;
    let inner = dialog(
        frame,
        area,
        72,
        height,
        vec![
            Span::styled("Keys", theme::text()),
            Span::styled(
                format!(
                    " · {} {}",
                    brand::NAME,
                    app.info.as_ref().map_or("", |i| i.version.as_str())
                ),
                theme::muted(),
            ),
        ],
    );
    let [body, instructions, close] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(inner);
    let [left, right] = Layout::horizontal([Constraint::Fill(1); 2])
        .spacing(2)
        .areas(body);
    for (column, heading, hints) in [
        (left, "everywhere", global),
        (right, "this screen", screen_keys),
    ] {
        frame.render_widget(Paragraph::new(heading).style(theme::muted()), column);
        for (row, (key, label)) in hints.into_iter().enumerate() {
            hint_at(frame, column, 0, row as u16 + 1, key, label);
        }
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("click to select · double-click to open", theme::muted()),
            Line::styled("wheel to scroll · drag to copy text", theme::muted()),
            Line::styled("shift-drag uses the terminal’s selection", theme::muted()),
        ]),
        instructions,
    );
    mouse::hints(frame, close, &[("esc", "close help")], Alignment::Right);
}

// Settings

const SETTING_KEY_W: usize = 14;

/// A key/value row of a settings panel. `hint` is a key shown, and clickable, at the right edge.
fn setting_row(
    frame: &mut Frame,
    line: Rect,
    key: &str,
    value: Vec<Span<'static>>,
    hint: Option<(&'static str, &'static str)>,
) -> Rect {
    if line.height == 0 || line.width == 0 {
        return Rect::ZERO;
    }
    let hint_w = hint.map_or(0, |hint| mouse::hints_width(&[hint]) as u16);
    let content_width = value.iter().map(Span::width).sum::<usize>();
    let mut spans = vec![Span::styled(
        format!("{key:<SETTING_KEY_W$}"),
        theme::muted(),
    )];
    spans.extend(value);
    let value_w = line.width.saturating_sub(hint_w + u16::from(hint_w > 0));
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            width: value_w,
            ..line
        },
    );
    if let Some(hint) = hint {
        mouse::hints(frame, line, &[hint], Alignment::Right);
    }
    let value_x = SETTING_KEY_W as u16;
    Rect::new(
        line.x + value_x.min(value_w),
        line.y,
        (content_width as u16).min(value_w.saturating_sub(value_x)),
        1,
    )
}

/// The rows of a panel body, one line each.
fn lines_of(area: Rect, n: u16) -> Vec<Rect> {
    (0..n)
        .map(|i| {
            Rect::new(
                area.x,
                area.y + i.min(area.height),
                area.width,
                u16::from(i < area.height),
            )
        })
        .collect()
}

fn settings(frame: &mut Frame, area: Rect, app: &App) {
    let [row1, row2, row3] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Fill(1),
    ])
    .spacing(1)
    .areas(area);
    if let Some(s) = &app.settings.settings {
        instance_panel(frame, row1, app, s);
        let [proxy, security, maintenance] = Layout::horizontal([Constraint::Fill(1); 3])
            .spacing(1)
            .areas(row2);
        proxy_panel(frame, proxy, s);
        security_panel(frame, security, s);
        maintenance_panel(frame, maintenance, s);
    } else {
        let block = panel(title("Instance", false), None, false);
        let inner = inside(&block, row1);
        frame.render_widget(block, row1);
        let status = if app.connection_error.is_some() {
            "not connected".into()
        } else {
            format!("{} loading", util::spinner(app.tick))
        };
        frame.render_widget(Paragraph::new(status).style(theme::muted()), inner);
    }
    let [left, servers] = Layout::horizontal([Constraint::Fill(2), Constraint::Fill(1)])
        .spacing(1)
        .areas(row3);
    let contexts_h = (app.settings.contexts.contexts.len() as u16 + 3).clamp(5, 9);
    let [backups, contexts] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(contexts_h)])
            .spacing(1)
            .areas(left);
    instance_backups_panel(frame, backups, app);
    contexts_panel(frame, contexts, app);
    let [secrets, storage, servers] = Layout::vertical([Constraint::Fill(1); 3])
        .spacing(1)
        .areas(servers);
    secrets_summary(frame, secrets, app);
    storage_summary(frame, storage, app);
    servers_summary(frame, servers, app);
}

/// A key hint on the last line of a summary panel, e.g. "s open secrets".
fn panel_link(frame: &mut Frame, inner: Rect, key: &'static str, label: &str) {
    if inner.height < 3 {
        return;
    }
    hint_at(frame, inner, 0, inner.height - 1, key, label);
}

fn storage_summary(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel(title("Storage", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let list = &app.storage.list;
    let room = inner.height.saturating_sub(2) as usize;
    let rows = lines_of(inner, room as u16);
    if list.is_empty()
        && let Some(row) = rows.first()
    {
        setting_row(
            frame,
            *row,
            "none yet",
            vec![Span::styled(
                "backups stay in the data directory",
                theme::muted(),
            )],
            None,
        );
    }
    for (row, s) in rows.iter().zip(list) {
        let (glyph, label, style) = crate::picker::test_state(s);
        let detail = match &s.last_test {
            Some(t) if !t.ok => t.error.clone().unwrap_or_default(),
            _ => String::new(),
        };
        setting_row(
            frame,
            *row,
            &truncate(&s.name, SETTING_KEY_W - 1),
            vec![
                Span::styled(format!("{glyph} "), style),
                Span::styled(label, theme::text()),
                Span::styled(format!("  {detail}"), theme::muted()),
            ],
            None,
        );
    }
    panel_link(frame, inner, "o", "open storage");
}

fn secrets_summary(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel(title("Secrets", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let list = &app.secrets.list;
    let count = |k: SecretKind| list.iter().filter(|s| s.kind == k).count();
    let in_use = list.iter().filter(|s| !s.used_by.is_empty()).count();
    let rows = lines_of(inner, 2);
    setting_row(
        frame,
        rows[0],
        "secrets",
        vec![
            Span::styled(list.len().to_string(), theme::text()),
            Span::styled(
                format!(
                    "  {} ssh keys · {} s3 · {} generic",
                    count(SecretKind::SshKey),
                    count(SecretKind::S3Credentials),
                    count(SecretKind::Generic)
                ),
                theme::muted(),
            ),
        ],
        None,
    );
    if let Some(row) = rows.get(1) {
        setting_row(
            frame,
            *row,
            "in use",
            vec![Span::styled(in_use.to_string(), theme::text())],
            None,
        );
    }
    panel_link(frame, inner, "s", "open secrets");
}

fn instance_panel(frame: &mut Frame, area: Rect, app: &App, s: &InstanceSettings) {
    let block = panel(title("Instance", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(4)
        .areas(inner);
    let update = app
        .settings
        .release
        .as_ref()
        .filter(|r| r.update_available)
        .and_then(|r| r.latest.clone());
    let mut version = vec![Span::styled(s.version.clone(), theme::text())];
    match (&app.updating, &update, &app.settings.release) {
        (Some(to), _, _) => version.push(Span::styled(
            format!("  updating to {to}…"),
            theme::fg(theme::INFO),
        )),
        (None, Some(latest), _) => version.push(Span::styled(
            format!("  update available {latest}"),
            theme::accent(),
        )),
        (None, None, Some(_)) => version.push(Span::styled("  up to date", theme::muted())),
        _ => {}
    }
    let rows = lines_of(left, 4);
    setting_row(
        frame,
        rows[0],
        "version",
        version,
        update.as_ref().map(|_| ("u", "update")),
    );
    setting_row(
        frame,
        rows[1],
        "hostname",
        vec![Span::styled(s.hostname.clone(), theme::text())],
        None,
    );
    setting_row(
        frame,
        rows[2],
        "public ip",
        vec![
            Span::styled(
                s.public_ip.clone().unwrap_or_else(|| "unknown".into()),
                theme::text(),
            ),
            Span::styled(
                if s.public_ip_override.is_some() {
                    "  set by hand"
                } else {
                    "  auto-detected"
                },
                theme::muted(),
            ),
        ],
        Some(("i", "edit")),
    );
    if let Some(row) = rows.get(3) {
        setting_row(
            frame,
            *row,
            "from a shell",
            vec![Span::styled(
                format!("{} update", brand::CLI_BIN),
                theme::text(),
            )],
            None,
        );
    }
    let rows = lines_of(right, 3);
    let value = |v: String| vec![Span::styled(v, theme::text())];
    setting_row(frame, rows[0], "listen", value(s.listen.clone()), None);
    setting_row(
        frame,
        rows[1],
        "data dir",
        vec![
            Span::styled(s.data_dir.clone(), theme::text()),
            Span::styled(
                format!("  {}", util::bytes(s.data_dir_size)),
                theme::muted(),
            ),
        ],
        None,
    );
    setting_row(
        frame,
        rows[2],
        "docker",
        value(s.docker_version.clone().unwrap_or_else(|| "unknown".into())),
        None,
    );
}

fn proxy_panel(frame: &mut Frame, area: Rect, s: &InstanceSettings) {
    let block = panel(title("Proxy", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let rows = lines_of(inner, 3);
    let (glyph, label, style) = match s.proxy {
        ProxyStatus::Running => ("●", "running", theme::fg(theme::OK)),
        ProxyStatus::Stopped => ("○", "stopped", theme::fg(theme::ERR)),
        ProxyStatus::Missing => ("○", "not set up", theme::muted()),
    };
    setting_row(
        frame,
        rows[0],
        "status",
        vec![
            Span::styled(glyph, style),
            Span::styled(format!(" {label}"), theme::text()),
        ],
        Some(("r", "restart")),
    );
    let image = s.proxy_image.rsplit('/').next().unwrap_or(&s.proxy_image);
    setting_row(
        frame,
        rows[1],
        "image",
        vec![Span::styled(image.to_string(), theme::muted())],
        None,
    );
    let toggle = setting_row(
        frame,
        rows[2],
        "auto-manage",
        vec![if s.manage_proxy {
            Span::styled("● on", theme::fg(theme::OK))
        } else {
            Span::styled("○ off", theme::muted())
        }],
        Some(("m", "toggle")),
    );
    mouse::key_target(toggle, "m");
}

fn security_panel(frame: &mut Frame, area: Rect, s: &InstanceSettings) {
    let block = panel(title("Security", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let rows = lines_of(inner, 3);
    setting_row(
        frame,
        rows[0],
        "api token",
        vec![Span::styled("••••••••••••", theme::muted())],
        Some(("t", "rotate")),
    );
    let (key, comment) = s
        .ssh_public_key
        .as_deref()
        .map(short_key)
        .unwrap_or_else(|| ("none".into(), String::new()));
    let public_key = setting_row(
        frame,
        rows[1],
        "ssh key",
        vec![Span::styled(key, theme::text())],
        s.ssh_public_key.as_ref().map(|_| ("k", "copy")),
    );
    if s.ssh_public_key.is_some() {
        mouse::key_target(public_key, "k");
    }
    setting_row(
        frame,
        rows[2],
        "",
        vec![Span::styled(comment, theme::muted())],
        None,
    );
}

fn maintenance_panel(frame: &mut Frame, area: Rect, s: &InstanceSettings) {
    let block = panel(title("Maintenance", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let rows = lines_of(inner, 3);
    let muted = |t: String| vec![Span::styled(t, theme::muted())];
    match s.reclaimable {
        Some(r) => {
            setting_row(
                frame,
                rows[0],
                "reclaimable",
                vec![Span::styled(util::bytes(r.total()), theme::text())],
                Some(("c", "prune")),
            );
            setting_row(
                frame,
                rows[1],
                "",
                muted(match r.images {
                    0 => "no old build images".into(),
                    1 => "1 old build image".into(),
                    n => format!("{n} old build images"),
                }),
                None,
            );
            setting_row(
                frame,
                rows[2],
                "",
                muted(format!(
                    "+ build cache {}",
                    util::bytes(r.build_cache_bytes)
                )),
                None,
            );
        }
        None => {
            setting_row(
                frame,
                rows[0],
                "reclaimable",
                muted("unknown".into()),
                Some(("c", "prune")),
            );
            setting_row(
                frame,
                rows[1],
                "",
                muted("Docker did not answer".into()),
                None,
            );
        }
    }
}

fn contexts_panel(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.settings.focus == SettingsFocus::Contexts;
    let block = panel(title("Contexts", focused), None, focused);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    mouse::hit(area, Hit::List(List::Contexts));
    let names = app.settings.context_names();
    let name_w = 18;
    let offset = scroll_offset(app.settings.row, inner.height.saturating_sub(1) as usize);
    row_hits(inner, names.len(), offset, Hit::Context);
    let mut lines = vec![Line::styled(
        format!("  {:<name_w$}URL", "NAME"),
        theme::muted(),
    )];
    for (i, name) in names
        .iter()
        .enumerate()
        .skip(offset)
        .take(inner.height.saturating_sub(1) as usize)
    {
        let current = *name == app.context;
        let selected = i == app.settings.row;
        let url = app.settings.contexts.contexts[name].url.clone();
        let tag = if current { "current" } else { "" };
        let name = Span::styled(
            format!("{:<name_w$}", truncate(name, name_w - 1)),
            if selected {
                theme::accent()
            } else {
                theme::text()
            },
        );
        let url = Span::styled(url, theme::muted());
        let url_start = 2 + name.width();
        let used = url_start + url.width() + tag.len();
        let pad = (inner.width as usize).saturating_sub(used);
        mouse::span_target(
            Rect::new(inner.x, inner.y + 1 + (i - offset) as u16, inner.width, 1),
            url_start,
            url.width(),
            Hit::ContextUrl(i),
        );
        lines.push(
            Line::from(vec![
                Span::styled(
                    if current { "● " } else { "○ " },
                    if current {
                        theme::accent()
                    } else {
                        theme::muted()
                    },
                ),
                name,
                url,
                Span::raw(" ".repeat(pad)),
                Span::styled(tag, theme::text()),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            }),
        );
    }
    if names.is_empty() {
        lines.push(Line::styled(
            format!(
                "  no saved contexts · {} login <url> --token <token>",
                brand::CLI_BIN
            ),
            theme::muted(),
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
    if inner.height > names.len() as u16 + 2
        && let Ok(path) = client::Config::path()
    {
        frame.render_widget(
            Paragraph::new(format!("saved in {}", path.display())).style(theme::muted()),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}

fn servers_summary(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel(title("Servers", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    mouse::hit(area, Hit::OpenServers);
    mouse::span_target(area, 2, "Servers".len(), Hit::OpenServers);
    let online = app
        .servers
        .iter()
        .filter(|s| s.status == ServerStatus::Online)
        .count();
    let proxies = app
        .servers
        .iter()
        .filter(|s| s.proxy == ProxyStatus::Running)
        .count();
    let rows = lines_of(inner, 2);
    setting_row(
        frame,
        rows[0],
        "servers",
        vec![
            Span::styled(app.servers.len().to_string(), theme::text()),
            Span::styled(
                format!("  {online} online · {} offline", app.servers.len() - online),
                theme::muted(),
            ),
        ],
        None,
    );
    setting_row(
        frame,
        rows[1],
        "proxies",
        vec![Span::styled(format!("{proxies} running"), theme::text())],
        None,
    );
    if inner.height > 3 {
        let line = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
        mouse::hints(
            frame,
            line,
            &[("S", "open the servers screen")],
            Alignment::Left,
        );
    }
}

// Secrets

/// The non-secret half shortened for the list: a short SSH key, or the access key id.
fn public_short(s: &Secret) -> String {
    match (&s.public_part, s.kind) {
        (None, _) => "—".into(),
        (Some(k), SecretKind::SshKey) => short_key(k).0,
        (Some(id), _) if id.chars().count() > 14 => {
            let n = id.chars().count();
            let head: String = id.chars().take(6).collect();
            let tail: String = id.chars().skip(n - 4).collect();
            format!("{head}…{tail}")
        }
        (Some(id), _) => id.clone(),
    }
}

fn secrets(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.secrets;
    let [list, detail] = Layout::vertical([Constraint::Fill(1), Constraint::Length(10)])
        .spacing(1)
        .areas(area);
    let note = format!(
        "sealed with the master key · {} secret{}",
        view.list.len(),
        if view.list.len() == 1 { "" } else { "s" }
    );
    let block = panel(
        title("Secrets", true),
        Some(Line::styled(note, theme::muted())),
        true,
    );
    let inner = inside(&block, list);
    frame.render_widget(block, list);
    mouse::hit(inner, Hit::List(List::Secrets));
    if view.list.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("no secrets yet · ", theme::muted()),
                Span::styled("a", theme::accent()),
                Span::styled(" to add or import one, ", theme::muted()),
                Span::styled("g", theme::accent()),
                Span::styled(" to generate an ssh key", theme::muted()),
            ])),
            inner,
        );
    } else {
        let rows: Vec<Row> = view
            .list
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let selected = i == view.row;
                let users: Vec<String> = s.used_by.iter().map(ToString::to_string).collect();
                Row::new(vec![
                    Cell::from(Span::styled(
                        s.name.clone(),
                        if selected {
                            theme::accent()
                        } else {
                            theme::text()
                        },
                    )),
                    Cell::from(Span::styled(s.kind.label(), theme::muted())),
                    Cell::from(Span::styled(
                        public_short(s),
                        if s.public_part.is_some() {
                            theme::text()
                        } else {
                            theme::muted()
                        },
                    )),
                    if users.is_empty() {
                        Cell::from(Span::styled("—", theme::muted()))
                    } else {
                        Cell::from(Span::styled(users.join(", "), theme::text()))
                    },
                    Cell::from(Span::styled(util::ago(s.updated_at), theme::muted())),
                ])
                .style(if selected {
                    theme::selected()
                } else {
                    Style::new()
                })
            })
            .collect();
        let offset = scroll_offset(view.row, inner.height.saturating_sub(1) as usize);
        row_hits(inner, rows.len(), offset, Hit::Item);
        frame.render_widget(
            Table::new(
                rows.into_iter().skip(offset),
                [
                    Constraint::Length(22),
                    Constraint::Length(16),
                    Constraint::Length(30),
                    Constraint::Fill(1),
                    Constraint::Length(10),
                ],
            )
            .header(
                Row::new(["NAME", "KIND", "PUBLIC PART", "USED BY", "UPDATED"])
                    .style(theme::muted()),
            )
            .column_spacing(2),
            inner,
        );
    }
    secret_detail(frame, detail, app);
}

fn secret_detail(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.secrets;
    let Some(s) = view.selected() else {
        let block = panel(title("secret", false), None, false);
        frame.render_widget(block, area);
        return;
    };
    let block = panel(title(&s.name, false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let [left, right] = Layout::horizontal([Constraint::Fill(2), Constraint::Fill(3)])
        .spacing(4)
        .areas(inner);
    let text = |t: String| vec![Span::styled(t, theme::text())];
    let rows = lines_of(left, left.height);
    let mut info: Vec<(&str, Vec<Span<'static>>)> = vec![
        ("kind", text(s.kind.label().to_string())),
        (
            "created",
            text(database::created(s.created_at, chrono::Utc::now())),
        ),
        ("updated", text(util::ago(s.updated_at))),
    ];
    if s.used_by.is_empty() {
        info.push(("used by", vec![Span::styled("nothing", theme::muted())]));
    }
    for (i, u) in s.used_by.iter().enumerate() {
        info.push((if i == 0 { "used by" } else { "" }, text(u.to_string())));
    }
    info.push(("id", vec![Span::styled(s.id.clone(), theme::muted())]));
    for (row, (k, v)) in rows.iter().zip(info) {
        setting_row(frame, *row, k, v, None);
    }

    let rows = lines_of(right, right.height);
    let mut i = 0;
    let mut row = |frame: &mut Frame,
                   k: &str,
                   v: Vec<Span<'static>>,
                   hint: Option<(&'static str, &'static str)>| {
        if let Some(r) = rows.get(i) {
            let value = setting_row(frame, *r, k, v, hint);
            if let Some((key, _)) = hint {
                mouse::key_target(value, key);
            }
        }
        i += 1;
    };
    let value_w = (right.width as usize).saturating_sub(SETTING_KEY_W + 20);
    match s.kind {
        SecretKind::SshKey => {
            let key = s.public_part.clone().unwrap_or_default();
            let mut parts = key.split_whitespace();
            let body = format!(
                "{} {}",
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default()
            );
            let comment = parts.collect::<Vec<_>>().join(" ");
            row(
                frame,
                "public key",
                text(truncate(&body, value_w)),
                Some(("c", "copy")),
            );
            row(frame, "", vec![Span::styled(comment, theme::muted())], None);
        }
        SecretKind::S3Credentials => row(
            frame,
            "access key id",
            text(s.public_part.clone().unwrap_or_default()),
            Some(("c", "copy")),
        ),
        SecretKind::Generic => {}
    }
    let label = match s.kind {
        SecretKind::SshKey => "private key",
        SecretKind::S3Credentials => "secret key",
        SecretKind::Generic => "value",
    };
    let revealed_at = view.revealed.as_ref().map(|(_, at)| *at);
    match view.value() {
        Some(value) => {
            let value = crate::app::secret_text(value);
            let mut lines = value.lines().filter(|l| !l.is_empty());
            let first = lines.next().unwrap_or_default().to_string();
            row(
                frame,
                label,
                text(truncate(&first, value_w)),
                Some(("C", "copy value")),
            );
            let more = lines.count();
            if more > 0 {
                row(
                    frame,
                    "",
                    vec![Span::styled(
                        format!("+ {more} more lines · C copies all"),
                        theme::muted(),
                    )],
                    None,
                );
            }
            row(
                frame,
                "",
                vec![Span::styled(
                    format!(
                        "● revealed {} · recorded in the activity feed",
                        revealed_at.map(util::clock).unwrap_or_default()
                    ),
                    theme::fg(theme::INFO),
                )],
                Some(("v", "hide")),
            );
            row(frame, "", Vec::new(), None);
            row(
                frame,
                "",
                vec![Span::styled(
                    "Hidden again when you move to another secret or leave this screen.",
                    theme::muted(),
                )],
                None,
            );
        }
        None => {
            row(
                frame,
                label,
                vec![Span::styled("••••••••••••••••", theme::muted())],
                Some(("v", "reveal")),
            );
            row(frame, "", Vec::new(), None);
            if s.kind == SecretKind::SshKey {
                row(
                    frame,
                    "",
                    vec![Span::styled(
                        "Add it to ~/.ssh/authorized_keys, then pick it on Servers (K).",
                        theme::muted(),
                    )],
                    None,
                );
            }
            row(
                frame,
                "",
                vec![Span::styled(
                    "Revealing a value is recorded in the activity feed.",
                    theme::muted(),
                )],
                None,
            );
        }
    }
}

// Storage

fn storage(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.storage;
    let [list, detail] = Layout::vertical([Constraint::Fill(1), Constraint::Length(9)])
        .spacing(1)
        .areas(area);
    let block = panel(
        title("Storage", true),
        Some(Line::styled(
            "S3-compatible buckets for backups",
            theme::muted(),
        )),
        true,
    );
    let inner = inside(&block, list);
    frame.render_widget(block, list);
    mouse::hit(inner, Hit::List(List::Storage));
    if view.list.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("no storage connections yet · ", theme::muted()),
                Span::styled("a", theme::accent()),
                Span::styled(
                    " to add an S3, R2, B2 or MinIO bucket; backups stay in the data directory until then",
                    theme::muted(),
                ),
            ])),
            inner,
        );
    } else {
        let rows: Vec<Row> = view
            .list
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let selected = i == view.row;
                let (glyph, label, style) = crate::picker::test_state(s);
                let test = if view.testing.contains(&s.id) {
                    Line::from(Span::styled(
                        format!("{} testing…", util::spinner(app.tick)),
                        theme::fg(theme::INFO),
                    ))
                } else {
                    match &s.last_test {
                        Some(t) if !t.ok => Line::from(vec![
                            Span::styled("failed ", theme::fg(theme::ERR)),
                            Span::styled(
                                format!(
                                    "{} · {}",
                                    t.error.as_deref().unwrap_or("no reason given"),
                                    util::ago(t.at)
                                ),
                                theme::muted(),
                            ),
                        ]),
                        _ => Line::styled(label, theme::muted()),
                    }
                };
                Row::new(vec![
                    Cell::from(Span::styled(glyph, style)),
                    Cell::from(Span::styled(
                        s.name.clone(),
                        if selected {
                            theme::accent()
                        } else {
                            theme::text()
                        },
                    )),
                    Cell::from(Span::styled(s.endpoint.clone(), theme::muted())),
                    Cell::from(Span::styled(crate::picker::bucket_path(s), theme::text())),
                    Cell::from(test),
                ])
                .style(if selected {
                    theme::selected()
                } else {
                    Style::new()
                })
            })
            .collect();
        let offset = scroll_offset(view.row, inner.height.saturating_sub(1) as usize);
        row_hits(inner, rows.len(), offset, Hit::Item);
        frame.render_widget(
            Table::new(
                rows.into_iter().skip(offset),
                [
                    Constraint::Length(2),
                    Constraint::Length(18),
                    Constraint::Fill(1),
                    Constraint::Length(28),
                    Constraint::Length(40),
                ],
            )
            .header(
                Row::new(["", "NAME", "ENDPOINT", "BUCKET / PREFIX", "LAST TEST"])
                    .style(theme::muted()),
            )
            .column_spacing(2),
            inner,
        );
    }
    storage_detail(frame, detail, app);
}

fn storage_detail(frame: &mut Frame, area: Rect, app: &App) {
    let Some(s) = app.storage.selected() else {
        frame.render_widget(panel(title("storage", false), None, false), area);
        return;
    };
    let block = panel(title(&s.name, false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(4)
        .areas(inner);
    let text = |t: String| vec![Span::styled(t, theme::text())];
    let creds = app
        .secrets
        .list
        .iter()
        .find(|c| c.id == s.credentials_secret_id);
    let prefix = if s.path_prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", s.path_prefix)
    };
    let rows = lines_of(left, 6);
    let lines: [(&str, Vec<Span<'static>>); 6] = [
        ("endpoint", text(s.endpoint.clone())),
        ("region", text(s.region.clone())),
        ("bucket", text(s.bucket.clone())),
        (
            "path prefix",
            vec![
                Span::styled(
                    if s.path_prefix.is_empty() {
                        "none".to_string()
                    } else {
                        s.path_prefix.clone()
                    },
                    theme::text(),
                ),
                Span::styled(
                    format!("  files go to {prefix}<service id>/<file>"),
                    theme::muted(),
                ),
            ],
        ),
        (
            "addressing",
            vec![
                Span::styled(
                    if s.path_style {
                        "path-style"
                    } else {
                        "virtual-hosted"
                    },
                    theme::text(),
                ),
                Span::styled(
                    if s.path_style {
                        "  <endpoint>/<bucket>"
                    } else {
                        "  <bucket>.<endpoint>"
                    },
                    theme::muted(),
                ),
            ],
        ),
        (
            "credentials",
            match creds {
                Some(c) => vec![
                    Span::styled(c.name.clone(), theme::text()),
                    Span::styled(
                        format!("  {}", c.public_part.clone().unwrap_or_default()),
                        theme::muted(),
                    ),
                ],
                None => vec![Span::styled("unknown secret", theme::fg(theme::ERR))],
            },
        ),
    ];
    for (row, (k, v)) in rows.iter().zip(lines) {
        setting_row(frame, *row, k, v, None);
    }
    let rows = lines_of(right, 4);
    let test = match &s.last_test {
        _ if app.storage.testing.contains(&s.id) => vec![Span::styled(
            format!("{} testing…", util::spinner(app.tick)),
            theme::fg(theme::INFO),
        )],
        None => vec![Span::styled("never tested", theme::muted())],
        Some(t) if t.ok => vec![
            Span::styled("● ", theme::fg(theme::OK)),
            Span::styled("ok", theme::text()),
            Span::styled(
                format!(
                    "  put, get and delete in {}ms · {}",
                    t.latency_ms.unwrap_or_default(),
                    util::ago(t.at)
                ),
                theme::muted(),
            ),
        ],
        Some(t) => vec![
            Span::styled("✕ ", theme::fg(theme::ERR)),
            Span::styled("failed", theme::text()),
            Span::styled(format!("  {}", util::ago(t.at)), theme::muted()),
        ],
    };
    if let Some(r) = rows.first() {
        setting_row(frame, *r, "last test", test, Some(("t", "test")));
    }
    if let (Some(r), Some(err)) = (
        rows.get(1),
        s.last_test.as_ref().and_then(|t| t.error.clone()),
    ) {
        setting_row(
            frame,
            *r,
            "",
            vec![Span::styled(err, theme::fg(theme::ERR))],
            None,
        );
    }
    if let Some(r) = rows.get(2) {
        setting_row(
            frame,
            *r,
            "created",
            text(database::created(s.created_at, chrono::Utc::now())),
            None,
        );
    }
}

// Instance backups

fn instance_backups_panel(frame: &mut Frame, area: Rect, app: &App) {
    let view = &app.settings;
    let focused = view.focus == SettingsFocus::Backups;
    let total: u64 = view.backups.iter().map(|b| b.size_bytes).sum();
    let note = format!(
        "{} archive{} · {}",
        view.backups.len(),
        if view.backups.len() == 1 { "" } else { "s" },
        util::bytes(total)
    );
    let block = panel(
        title("Instance backups", focused),
        Some(Line::styled(note, theme::muted())),
        focused,
    );
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    mouse::hit(inner, Hit::List(List::InstanceBackups));
    let [list, _, footer] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(u16::from(inner.height > 4)),
        Constraint::Length(2.min(inner.height.saturating_sub(1))),
    ])
    .areas(inner);
    let store_name = |id: &Option<Id>| match id {
        None => "local".to_string(),
        Some(id) => app.storage.name(id).unwrap_or("s3").to_string(),
    };
    if view.backups.is_empty() {
        frame.render_widget(
            Paragraph::new("no instance backups yet").style(theme::muted()),
            list,
        );
        hint_at(frame, list, 24, 0, "b", "back up now");
    } else {
        let now = chrono::Utc::now();
        let rows: Vec<Row> = view
            .backups
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let selected = i == view.backup_row;
                let (glyph, age, file) = match b.status {
                    BackupStatus::Running => (
                        Span::styled(util::spinner(app.tick), theme::fg(theme::INFO)),
                        util::duration((now - b.created_at).num_seconds()),
                        Span::styled("backing up…", theme::fg(theme::INFO)),
                    ),
                    BackupStatus::Done => (
                        Span::styled("●", theme::fg(theme::OK)),
                        util::ago(b.created_at),
                        Span::styled(b.file_name.clone(), theme::text()),
                    ),
                    BackupStatus::Failed => (
                        Span::styled("✕", theme::fg(theme::ERR)),
                        util::ago(b.created_at),
                        Span::styled(
                            b.error.clone().unwrap_or_else(|| "failed".into()),
                            theme::fg(theme::ERR),
                        ),
                    ),
                };
                let size = if b.status == BackupStatus::Done {
                    Span::styled(util::bytes(b.size_bytes), theme::text())
                } else {
                    Span::styled("—", theme::muted())
                };
                Row::new(vec![
                    Cell::from(glyph),
                    Cell::from(Line::from(vec![
                        Span::styled(
                            database::created(b.created_at, now),
                            if selected && focused {
                                theme::accent()
                            } else {
                                theme::text()
                            },
                        ),
                        Span::styled(format!("  {age}"), theme::muted()),
                    ])),
                    Cell::from(Span::styled(b.trigger.as_str(), theme::muted())),
                    Cell::from(size),
                    Cell::from(Span::styled(
                        store_name(&b.storage_id),
                        if b.storage_id.is_some() {
                            theme::text()
                        } else {
                            theme::muted()
                        },
                    )),
                    Cell::from(file),
                ])
                .style(if selected {
                    theme::selected()
                } else {
                    Style::new()
                })
            })
            .collect();
        let offset = scroll_offset(view.backup_row, list.height.saturating_sub(1) as usize);
        row_hits(list, rows.len(), offset, Hit::Item);
        frame.render_widget(
            Table::new(
                rows.into_iter().skip(offset),
                [
                    Constraint::Length(2),
                    Constraint::Length(22),
                    Constraint::Length(8),
                    Constraint::Length(6),
                    Constraint::Length(14),
                    Constraint::Fill(1),
                ],
            )
            .header(
                Row::new(["", "CREATED", "TRIGGER", "SIZE", "WHERE", "FILE"]).style(theme::muted()),
            )
            .column_spacing(2),
            list,
        );
    }
    let lines = lines_of(footer, footer.height);
    let schedule = match &view.schedule {
        Some(s) if s.enabled => vec![
            Span::styled(format!("daily {:02}:00 UTC", s.hour_utc), theme::text()),
            Span::styled(
                format!(
                    "  keep {} · to {} · {}",
                    s.keep,
                    store_name(&s.storage_id),
                    if s.has_passphrase {
                        "passphrase stored"
                    } else {
                        "no passphrase stored"
                    }
                ),
                theme::muted(),
            ),
        ],
        Some(_) => vec![Span::styled("off", theme::muted())],
        None => vec![Span::styled(
            format!("{} loading", util::spinner(app.tick)),
            theme::muted(),
        )],
    };
    if let Some(line) = lines.first() {
        let value = setting_row(frame, *line, "schedule", schedule, Some(("e", "edit")));
        mouse::key_target(value, "e");
    }
    if let Some(line) = lines.get(1) {
        setting_row(
            frame,
            *line,
            "restore",
            vec![
                Span::styled(
                    format!(
                        "{} restore <file|s3://bucket/key> --passphrase-file <file>",
                        brand::DAEMON_BIN
                    ),
                    theme::text(),
                ),
                Span::styled("  on a new machine", theme::muted()),
            ],
            None,
        );
    }
}
