//! Drawing. Every screen follows the Paper design: a top bar, bordered panels, a key bar.

use crate::app::*;
use crate::mouse::{self, Hit};
use crate::theme;
use crate::util::{self, truncate};
use common::*;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
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
    }
    key_bar(frame, keys, app);
    if app.overlay.is_some() {
        mouse::hit(area, Hit::Block);
    }
    match &app.overlay {
        Some(Overlay::Form(form)) => {
            dim(frame, area);
            form.render(frame, area, app.tick);
        }
        Some(Overlay::Confirm(c)) => {
            dim(frame, area);
            confirm(frame, area, c);
        }
        Some(Overlay::Palette(p)) => {
            dim(frame, area);
            palette(frame, area, p);
        }
        Some(Overlay::Help) => {
            dim(frame, area);
            help(frame, area, app);
        }
        None => {}
    }
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
        (Some(_), _) => Span::styled("○ disconnected", theme::fg(theme::ERR)),
        (None, Some(_)) => Span::styled("● connected", theme::fg(theme::OK)),
        (None, None) => Span::styled(
            format!("{} connecting", util::spinner(app.tick)),
            theme::muted(),
        ),
    });
    frame.render_widget(Paragraph::new(Line::from(left)), area);

    let Some(info) = &app.info else { return };
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
    frame.render_widget(Paragraph::new(right), area);
}

type Keys = Vec<(&'static str, &'static str)>;

fn keys_for(app: &App) -> (Keys, Keys) {
    let right = vec![("?", "help"), ("q", "quit")];
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
            ("p", "set up proxy"),
            ("x", "remove"),
            ("esc", "back"),
        ],
    };
    let right = match app.screen {
        Screen::Overview => right,
        Screen::Deploy => vec![("q", "quit")],
        _ => vec![("esc", "back"), ("q", "quit")],
    };
    (left, right)
}

fn key_spans(keys: &[(&'static str, &'static str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (k, l)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(*k, theme::accent()));
        spans.push(Span::styled(format!(" {l}"), theme::muted()));
    }
    spans
}

/// Make each key hint clickable; `x` is where the first hint starts.
fn key_hits(keys: &[(&'static str, &'static str)], mut x: u16, y: u16) {
    for (i, (k, l)) in keys.iter().enumerate() {
        if i > 0 {
            x += 3;
        }
        let w = (k.chars().count() + 1 + l.chars().count()) as u16;
        if let Some((code, mods)) = mouse::key_for_label(k) {
            mouse::hit(Rect::new(x, y, w, 1), Hit::Key(code, mods));
        }
        x += w;
    }
}

fn keys_width(keys: &[(&'static str, &'static str)]) -> u16 {
    let text: usize = keys
        .iter()
        .map(|(k, l)| k.chars().count() + 1 + l.chars().count())
        .sum();
    (text + keys.len().saturating_sub(1) * 3) as u16
}

fn key_bar(frame: &mut Frame, area: Rect, app: &App) {
    let (left, right) = keys_for(app);
    frame.render_widget(Paragraph::new(Line::from(key_spans(&left))), area);
    key_hits(&left, area.x, area.y);
    if app.toast.is_none() {
        key_hits(
            &right,
            area.right().saturating_sub(keys_width(&right)),
            area.y,
        );
    }
    let right = match &app.toast {
        Some(t) => {
            let style = if t.error {
                theme::fg(theme::ERR)
            } else {
                theme::fg(theme::OK)
            };
            let glyph = if t.error { "✕ " } else { "✓ " };
            let max = area.width as usize / 2;
            Line::from(Span::styled(
                format!("{glyph}{}", truncate(&t.text, max)),
                style,
            ))
        }
        None => Line::from(key_spans(&right)),
    };
    frame.render_widget(Paragraph::new(right.right_aligned()), area);
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
    let table_h = (services.len() as u16 + 4).clamp(6, main.height / 2);
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
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("no services here yet · press ", theme::muted()),
                    Span::styled("n", theme::accent()),
                    Span::styled(" to add one", theme::muted()),
                ])),
                inner,
            );
        }
    }
    activity_panel(frame, activity, app);
}

fn empty_state(frame: &mut Frame, area: Rect) {
    let block = panel(title("Welcome", true), None, true);
    let inner = inside(&block, area).inner(Margin::new(2, 1));
    frame.render_widget(block, area);
    let lines = vec![
        Line::styled("Nothing deployed yet.", theme::text()),
        Line::raw(""),
        Line::from(vec![
            Span::styled("N", theme::accent()),
            Span::styled(
                "  create a project, with a production environment",
                theme::muted(),
            ),
        ]),
        Line::from(vec![
            Span::styled("n", theme::accent()),
            Span::styled(
                "  add a service from a git repo, an image or a database",
                theme::muted(),
            ),
        ]),
        Line::from(vec![
            Span::styled("S", theme::accent()),
            Span::styled(
                "  servers and the proxy that serves your domains",
                theme::muted(),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
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

    let Some(tree) = &app.tree else {
        frame.render_widget(
            Paragraph::new(format!("{} loading", util::spinner(app.tick))).style(theme::muted()),
            inner,
        );
        return;
    };
    mouse::hit(projects, Hit::Pane(Pane::Sidebar));
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
        lines.push(Line::from(vec![
            Span::styled("N", theme::accent()),
            Span::styled(" new project", theme::muted()),
        ]));
    }
    let offset = scroll_offset(
        app.overview.side + app.overview.side.min(tree.projects.len()),
        inner.height as usize,
    );
    frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
    for (line, i) in item_lines {
        if let Some(dy) = line
            .checked_sub(offset)
            .filter(|dy| *dy < inner.height as usize)
        {
            mouse::hit(
                Rect::new(inner.x, inner.y + dy as u16, inner.width, 1),
                Hit::Side(i),
            );
        }
    }

    let block = panel(title("Servers", false), None, false);
    let inner = inside(&block, servers);
    frame.render_widget(block, servers);
    let lines: Vec<Line> = app
        .servers
        .iter()
        .map(|s| {
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
            let name = truncate(&s.name, inner.width as usize - 12);
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
    let block = panel(heading, Some(right), focused);
    let inner = inside(&block, area);
    frame.render_widget(block, area);

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
    mouse::hit(area, Hit::Pane(Pane::Services));
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
    if empty {
        let msg = if app.overview.filter.is_empty() {
            "no services yet · n to add one"
        } else {
            "nothing matches the filter"
        };
        frame.render_widget(
            Paragraph::new(msg).style(theme::muted()),
            Rect {
                y: inner.y + 1,
                height: 1,
                ..inner
            },
        );
    }
}

fn kv<'a>(key: &'a str, value: Vec<Span<'a>>, key_w: usize) -> Line<'a> {
    let mut spans = vec![Span::styled(format!("{key:<key_w$}"), theme::muted())];
    spans.extend(value);
    Line::from(spans)
}

fn service_lines<'a>(
    detail: Option<&'a ServiceDetail>,
    service: &'a Service,
    reveal: bool,
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
    let _ = reveal;
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
    frame.render_widget(Paragraph::new(service_lines(detail, s, false)), inner);
    charts(frame, inner, app.stats.get(&s.id));
}

fn activity_panel(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel(title("Activity", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    if app.activity.is_empty() {
        frame.render_widget(Paragraph::new("quiet so far").style(theme::muted()), inner);
        return;
    }
    let lines: Vec<Line> = app
        .activity
        .iter()
        .take(inner.height as usize)
        .map(|a| {
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
        if let Some(d) = live {
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
        if let Some(dom) = view.detail.as_ref().and_then(|d| d.domains.first()) {
            meta.push(format!(
                "{}://{}",
                if dom.tls { "https" } else { "http" },
                dom.host
            ));
        }
        if !meta.is_empty() {
            spans.push(Span::styled(
                format!("   {}", meta.join(" · ")),
                theme::muted(),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), head);

    let mut tab_spans = vec![Span::raw(" ")];
    let mut x = tabs.x + 1;
    for t in Tab::ALL {
        let w = t.title().chars().count() as u16;
        mouse::hit(Rect::new(x, tabs.y, w, 1), Hit::Tab(t));
        x += w + 4;
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

fn rail(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let container_h = 9;
    let [c_area, d_area] = Layout::vertical([Constraint::Length(container_h), Constraint::Fill(1)])
        .spacing(1)
        .areas(area);
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
                    truncate(&c.image, inner.width as usize - 10),
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

    let block = panel(title("Deployments", false), None, false);
    let inner = inside(&block, d_area);
    frame.render_widget(block, d_area);
    let mut lines: Vec<Line> = view
        .deployments
        .iter()
        .take(inner.height.saturating_sub(2) as usize)
        .map(|d| {
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
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("never deployed · ", theme::muted()),
            Span::styled("d", theme::accent()),
            Span::styled(" to deploy", theme::muted()),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn service_overview(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let (main, rail_area) = with_rail(area);
    if let Some(d) = &view.detail {
        let block = panel(title("Overview", true), None, true);
        let inner = inside(&block, main).inner(Margin::new(1, 1));
        frame.render_widget(block, main);
        frame.render_widget(
            Paragraph::new(service_lines(Some(d), &d.service, view.reveal)),
            inner,
        );
        charts(frame, inner, app.stats.get(&view.id));
    }
    rail(frame, rail_area, app, view);
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
    let end = visible.len().saturating_sub(logs.offset);
    let start = end.saturating_sub(height);
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
        let rendered = p.line_count(area.width);
        p = p.scroll((rendered.saturating_sub(height) as u16, 0));
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
    let show_bar = logs.searching || !logs.search.is_empty() || logs.ended.is_some();
    let [lines_a, bar] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(if show_bar { 2 } else { 0 }),
    ])
    .areas(inner);
    let empty = match &logs.ended {
        Some(e) => format!("no logs · {e}"),
        None => format!("{} waiting for output", util::spinner(app.tick)),
    };
    log_view(frame, lines_a, logs, &empty, &app.log_viewport);
    if show_bar {
        let [rule, line] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(bar);
        frame.render_widget(
            Paragraph::new("─".repeat(rule.width as usize)).style(theme::fg(theme::LINE)),
            rule,
        );
        if logs.searching || !logs.search.is_empty() {
            let mut left = vec![
                Span::styled("/ ", theme::accent()),
                Span::styled(logs.search.clone(), theme::text()),
            ];
            if logs.searching {
                left.push(Span::styled("▏", theme::accent()));
            }
            frame.render_widget(Paragraph::new(Line::from(left)), line);
            let n = logs.matches().len();
            let info = if n == 0 {
                "no matches".to_string()
            } else {
                format!(
                    "{} of {n} matches · n next · N prev",
                    logs.match_idx.min(n - 1) + 1
                )
            };
            frame.render_widget(
                Paragraph::new(info).style(theme::muted()).right_aligned(),
                line,
            );
        } else if let Some(e) = &logs.ended {
            frame.render_widget(
                Paragraph::new(format!("○ {e} · ctrl+r to reconnect")).style(theme::muted()),
                line,
            );
        }
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
        frame.render_widget(
            Paragraph::new("no deployments yet · d to deploy").style(theme::muted()),
            inner,
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
    empty: &'a str,
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
        if rows.is_empty() {
            frame.render_widget(
                Paragraph::new(self.empty.to_string()).style(theme::muted()),
                inner,
            );
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
        empty: "no variables · a to add one",
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
        empty: "no domains · a to add one, then point its A record at this server",
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
        empty: "no volumes · a to add one",
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
    let lines: Vec<Line> = d
        .steps
        .iter()
        .flat_map(|s| {
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
    frame.render_widget(Paragraph::new(lines), inner);

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
    if inner.height > 6 {
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
    let header =
        Row::new(["", "NAME", "KIND", "HOST", "DOCKER", "PROXY", "CHECKED"]).style(theme::muted());
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
    row_hits(inner, rows.len(), 0, Hit::Server);
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Length(16),
                Constraint::Length(6),
                Constraint::Fill(1),
                Constraint::Length(10),
                Constraint::Length(15),
                Constraint::Length(10),
            ],
        )
        .header(header)
        .column_spacing(2),
        inner,
    );
    if let Some(key) = key {
        let block = panel(title("Daemon SSH key", false), None, false);
        let inner = inside(&block, key_area);
        frame.render_widget(block, key_area);
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "Add this key to ~/.ssh/authorized_keys on a server before adding it:",
                    theme::muted(),
                ),
                Line::styled(key, theme::text()),
            ])
            .wrap(Wrap { trim: true }),
            inner,
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

fn confirm(frame: &mut Frame, area: Rect, c: &Confirm) {
    let inner = dialog(
        frame,
        area,
        64,
        8,
        vec![Span::styled(c.title.clone(), theme::text())],
    );
    let [msg, _, keys] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(c.message.clone())
            .style(theme::text())
            .wrap(Wrap { trim: true }),
        msg,
    );
    let yes = if c.danger {
        Style::new().fg(theme::BG).bg(theme::ERR)
    } else {
        theme::chip()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("esc", theme::accent()),
            Span::styled(" keep", theme::muted()),
        ])),
        keys,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(" y confirm ", yes)).right_aligned(),
        keys,
    );
    mouse::hit(
        Rect::new(keys.x, keys.y, 8, 1),
        Hit::Key(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    mouse::hit(
        Rect::new(keys.right().saturating_sub(11), keys.y, 11, 1),
        Hit::Key(
            crossterm::event::KeyCode::Char('y'),
            crossterm::event::KeyModifiers::NONE,
        ),
    );
}

fn palette(frame: &mut Frame, area: Rect, p: &Palette) {
    let commands = p.commands();
    let h = commands.len() as u16 + 6;
    let inner = dialog(
        frame,
        area,
        56,
        h,
        vec![Span::styled("Command", theme::text())],
    );
    let mut lines = vec![
        Line::from(vec![
            Span::styled(": ", theme::accent()),
            Span::styled(p.input.clone(), theme::text()),
            Span::styled("▏", theme::accent()),
        ]),
        Line::raw(""),
    ];
    for (i, c) in commands.iter().enumerate() {
        let selected = i == p.row;
        let label = format!(" {:<width$}", c.label(), width = inner.width as usize - 10);
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
    for i in 0..commands.len() {
        let y = inner.y + 2 + i as u16;
        if y < inner.bottom() {
            mouse::hit(Rect::new(inner.x, y, inner.width, 1), Hit::PaletteRow(i));
        }
    }
    if commands.is_empty() {
        lines.push(Line::styled(" no command matches", theme::muted()));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn help(frame: &mut Frame, area: Rect, app: &App) {
    let sections: [(&str, &[(&str, &str)]); 4] = [
        (
            "everywhere",
            &[
                (":", "command palette"),
                ("?", "this help"),
                ("S", "servers"),
                ("ctrl+r", "refresh"),
                ("esc", "back"),
                ("q", "quit"),
                ("click", "select · double-click opens"),
                ("wheel", "scroll lists and logs"),
                ("drag", "select text, copied on release"),
                ("⇧ drag", "terminal's own selection"),
            ],
        ),
        (
            "overview",
            &[
                ("↑↓ jk", "move"),
                ("←→ tab", "switch pane"),
                ("⏎", "open / fold"),
                ("n", "new service"),
                ("N", "new project"),
                ("E", "new environment"),
                ("x", "delete selected"),
                ("/", "filter"),
            ],
        ),
        (
            "service",
            &[
                ("tab 1-7", "switch tab"),
                ("d", "deploy"),
                ("r", "restart"),
                ("s", "stop / start"),
                ("l", "logs"),
                ("a", "add (env, domain, volume)"),
            ],
        ),
        (
            "logs",
            &[
                ("f", "follow"),
                ("/", "search"),
                ("n N", "next / prev match"),
                ("t", "timestamps"),
                ("w", "wrap"),
                ("e", "errors only"),
                ("g G", "top / bottom"),
            ],
        ),
    ];
    let height = sections
        .iter()
        .map(|(_, k)| k.len() as u16 + 2)
        .sum::<u16>()
        + 2;
    mouse::hit(
        area,
        Hit::Key(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
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
    let mut lines = Vec::new();
    for (name, keys) in sections {
        lines.push(Line::styled(name, theme::muted()));
        for (k, l) in keys {
            lines.push(Line::from(vec![
                Span::styled(format!("  {k:<10}"), theme::accent()),
                Span::styled(*l, theme::text()),
            ]));
        }
        lines.push(Line::raw(""));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
