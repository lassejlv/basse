//! Provider connections: the Providers screen, the add form, what a token sees and removing a
//! connection, following the "SET-09" to "SET-12" artboards in Paper.

use crate::app::{App, Confirm, ConfirmAction, Msg, Overlay, Screen};
use crate::form::{Field, Form, FormKind};
use crate::mouse::{self, Hit, List};
use crate::picker::{PickFor, PickItem, PickKind, Picker};
use crate::theme;
use crate::util::{self, truncate};
use crate::view::{inside, lines_of, panel, row_hits, scroll_offset, setting_row, title};
use common::lb::{CreateProvider, Provider, ProviderDiscovery, ProviderKind, ProviderStatus};
use common::{Id, SecretKind, brand};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};
use std::collections::{HashMap, HashSet};

const ADD_NOTE: [&str; 2] = [
    "Hetzner: a read & write project token.",
    "Cloudflare: Zone DNS edit, optionally Load Balancing edit.",
];

/// Results of provider requests, applied on the UI thread.
pub enum ProvidersMsg {
    List(Vec<Provider>),
    Checked(Id, Result<Provider, String>),
    Discovery(Id, Result<ProviderDiscovery, String>),
    /// The services whose load balancer uses each provider, for itself or for DNS.
    Users(HashMap<Id, Vec<Id>>),
}

/// The Providers screen and the provider data the load balancer forms share.
#[derive(Default)]
pub struct ProvidersView {
    pub list: Vec<Provider>,
    pub loaded: bool,
    pub row: usize,
    /// Providers being checked right now.
    pub checking: HashSet<Id>,
    /// The provider whose discovery screen is open.
    pub open: Option<Id>,
    /// What each provider's token sees, as last asked.
    pub discovery: HashMap<Id, Result<ProviderDiscovery, String>>,
    /// Row of the discovery screen's first table.
    pub detail_row: usize,
    /// Service ids whose load balancer uses each provider.
    pub users: HashMap<Id, Vec<Id>>,
    /// Screen to return to on esc.
    pub back: Option<Screen>,
}

impl ProvidersView {
    pub fn selected(&self) -> Option<&Provider> {
        self.list.get(self.row)
    }

    pub fn get(&self, id: &str) -> Option<&Provider> {
        self.list.iter().find(|p| p.id == id)
    }

    pub fn name(&self, id: &str) -> String {
        self.get(id)
            .map_or_else(|| id.to_string(), |p| p.name.clone())
    }

    pub fn opened(&self) -> Option<&Provider> {
        self.get(self.open.as_deref()?)
    }

    pub fn discovered(&self, id: &str) -> Option<&ProviderDiscovery> {
        self.discovery.get(id)?.as_ref().ok()
    }
}

/// "4 servers · load balancing": what a token reaches.
pub fn reaches(p: &Provider) -> String {
    let mut s = p.account.clone().unwrap_or_default();
    if p.kind == ProviderKind::Cloudflare && p.load_balancing {
        if !s.is_empty() {
            s.push_str(" · ");
        }
        s.push_str("load balancing");
    }
    s
}

fn status_glyph(p: &Provider) -> Span<'static> {
    match p.status {
        ProviderStatus::Connected => Span::styled("●", theme::fg(theme::OK)),
        ProviderStatus::Error => Span::styled("✕", theme::fg(theme::ERR)),
        ProviderStatus::Unknown => Span::styled("○", theme::muted()),
    }
}

impl App {
    pub fn open_providers(&mut self) {
        if self.screen != Screen::Providers {
            self.providers.back = Some(self.screen);
        }
        self.screen = Screen::Providers;
        self.providers.open = None;
        self.load_provider_users();
        self.refresh();
    }

    pub fn close_providers(&mut self) {
        if self.providers.open.take().is_some() {
            return;
        }
        self.screen = match self.providers.back.take() {
            Some(Screen::Service) if self.service.is_some() => Screen::Service,
            Some(back @ (Screen::Servers | Screen::Settings | Screen::Secrets)) => back,
            _ => Screen::Overview,
        };
        self.refresh();
    }

    pub fn load_providers(&self) {
        self.spawn(|c| async move {
            match c.providers().await {
                Ok(list) => Msg::Providers(ProvidersMsg::List(list)),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    /// Asks every service that could have a load balancer for it: providers don't say who uses
    /// them.
    pub fn load_provider_users(&self) {
        let ids: Vec<Id> = self
            .all_services()
            .filter(|s| crate::replicas::scalable(s))
            .map(|s| s.id.clone())
            .collect();
        self.spawn(|c| async move {
            let lbs = futures_util::future::join_all(ids.iter().map(|id| c.service_lb(id))).await;
            let mut users: HashMap<Id, Vec<Id>> = HashMap::new();
            for (id, lb) in ids.iter().zip(lbs) {
                let Ok(Some(lb)) = lb else { continue };
                users
                    .entry(lb.config.provider_id.clone())
                    .or_default()
                    .push(id.clone());
                if let Some(dns) = lb.config.dns_provider_id {
                    users.entry(dns).or_default().push(id.clone());
                }
            }
            Msg::Providers(ProvidersMsg::Users(users))
        });
    }

    pub fn load_discovery(&self, id: &str) {
        let id = id.to_string();
        self.spawn(|c| async move {
            let result = c.discover_provider(&id).await.map_err(|e| format!("{e:#}"));
            Msg::Providers(ProvidersMsg::Discovery(id, result))
        });
    }

    pub fn select_provider(&mut self, row: usize) {
        if self.providers.open.is_some() {
            self.providers.detail_row = row.min(self.discovery_rows().saturating_sub(1));
        } else {
            self.providers.row = row.min(self.providers.list.len().saturating_sub(1));
        }
    }

    /// Rows of the discovery screen's first table.
    pub fn discovery_rows(&self) -> usize {
        let Some(p) = self.providers.opened() else {
            return 0;
        };
        self.providers
            .discovered(&p.id)
            .map_or(0, |d| match p.kind {
                ProviderKind::Hetzner => d.servers.len(),
                ProviderKind::Cloudflare => d.zones.len(),
            })
    }

    /// ⏎ on the list: what the token sees.
    pub fn open_discovery(&mut self) {
        let Some(id) = self.providers.selected().map(|p| p.id.clone()) else {
            return;
        };
        self.providers.open = Some(id.clone());
        self.providers.detail_row = 0;
        self.load_discovery(&id);
    }

    pub fn check_provider(&mut self) {
        let Some(p) = self.providers.opened().or(self.providers.selected()) else {
            return;
        };
        let (id, name) = (p.id.clone(), p.name.clone());
        if !self.providers.checking.insert(id.clone()) {
            return;
        }
        self.progress(format!("checking {name}…"));
        if self.providers.open.is_some() {
            self.load_discovery(&id);
        }
        self.spawn(|c| async move {
            let result = c.check_provider(&id).await.map_err(|e| format!("{e:#}"));
            Msg::Providers(ProvidersMsg::Checked(id, result))
        });
    }

    pub fn form_add_provider(&mut self) {
        let kinds = ProviderKind::ALL
            .iter()
            .map(|k| k.label().to_string())
            .collect();
        let form = Form::new(
            FormKind::AddProvider,
            "Add provider",
            vec![
                Field::choice("kind", "kind", kinds).hint("←→ choose"),
                Field::text("name", "name")
                    .placeholder(ProviderKind::Hetzner.as_str())
                    .hint("shown in lists"),
                Field::text("token", "api token")
                    .masked()
                    .hint("saved as a secret"),
                Field::pick("token_secret", "or secret", "", "— none").hint("a generic secret"),
            ],
        )
        .subtitle("connect a cloud account")
        .note(ADD_NOTE.map(String::from).to_vec())
        .submit("add & check", None);
        self.overlay = Some(Overlay::Form(form));
        self.load_secrets();
    }

    /// ⏎ on the add form's secret field: the generic secrets.
    pub fn pick_token_secret(&mut self, form: Form) {
        let mut none = PickItem::new(None, "— none", "paste the token instead");
        none.glyph = None;
        let mut items = vec![none];
        items.extend(
            self.secrets
                .list
                .iter()
                .filter(|s| s.kind == SecretKind::Generic)
                .map(|s| {
                    let mut item = PickItem::new(Some(s.id.clone()), &s.name, "generic secret");
                    item.note = match s.used_by.len() {
                        0 => "unused".into(),
                        n => format!("used by {n}"),
                    };
                    item
                }),
        );
        let current = Some(form.raw("token_secret")).filter(|v| !v.is_empty());
        let row = current
            .as_ref()
            .and_then(|c| items.iter().position(|i| i.id.as_ref() == Some(c)))
            .unwrap_or(0);
        self.overlay = Some(Overlay::Picker(Picker {
            title: "Token secret".into(),
            subtitle: "add provider".into(),
            kind: PickKind::Fixed,
            items,
            row,
            target: PickFor::Field(Box::new(form), "token_secret"),
            current,
            note: vec![Line::styled(
                "Generic secrets that hold an API token; add them on the Secrets screen.",
                theme::muted(),
            )],
            busy: false,
            git: None,
        }));
    }

    pub fn submit_add_provider(&mut self) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        let kind = ProviderKind::ALL[form.choice("kind").min(ProviderKind::ALL.len() - 1)];
        let token = form.get("token");
        let secret = form.raw("token_secret");
        let (token, token_secret_id) = match (token.is_empty(), secret.is_empty()) {
            (true, true) => {
                return form.fail("paste the API token, or pick a secret that holds it");
            }
            (false, false) => return form.fail("use either a token or a secret, not both"),
            (false, true) => (Some(token), None),
            (true, false) => (None, Some(secret)),
        };
        let name = Some(form.get("name"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| kind.as_str().to_string());
        let body = CreateProvider {
            name,
            kind,
            token,
            token_secret_id,
        };
        form.busy = true;
        form.error = None;
        self.spawn(|c| async move {
            match c.create_provider(&body).await {
                Ok(p) => Msg::FormDone {
                    toast: format!("{} connected · {}", p.name, reaches(&p)),
                    then: crate::app::Then::Stay,
                },
                Err(e) => Msg::FormError(format!("{e:#}")),
            }
        });
    }

    /// d on the list. A provider in use is refused up front with the services that use it.
    pub fn ask_remove_provider(&mut self) {
        let Some(p) = self.providers.selected().cloned() else {
            return;
        };
        let secret = self
            .secrets
            .name(&p.token_secret_id)
            .map_or("its token secret".to_string(), |n| {
                format!("its token secret, {n},")
            });
        let mut error = Vec::new();
        let users = self.providers.users.get(&p.id).cloned().unwrap_or_default();
        for id in &users {
            match self.service_path(id) {
                Some((project, env, s)) => error.push(format!(
                    "{} in {} / {} uses it for its load balancer",
                    s.name, project.name, env.name
                )),
                None => error.push(format!("{id} uses it for its load balancer")),
            }
        }
        if let Some(first) = users.first().and_then(|id| self.service_path(id)) {
            error.push(format!(
                "Turn that load balancer off first: {}, Settings, l, x disable.",
                first.2.name
            ));
        }
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: format!("Remove provider · {}", p.name),
            message: format!("Remove {}? {} is kept.", p.name, capitalize(&secret)),
            danger: true,
            action: ConfirmAction::DeleteProvider(p.id),
            verb: "remove",
            busy: None,
            error,
        }));
    }

    pub fn apply_providers(&mut self, msg: ProvidersMsg) {
        match msg {
            ProvidersMsg::List(list) => {
                let view = &mut self.providers;
                let selected = view.selected().map(|p| p.id.clone());
                view.list = list;
                view.loaded = true;
                view.row = selected
                    .and_then(|id| view.list.iter().position(|p| p.id == id))
                    .unwrap_or(view.row)
                    .min(view.list.len().saturating_sub(1));
                self.refill_lb_picker();
            }
            ProvidersMsg::Checked(id, result) => {
                self.providers.checking.remove(&id);
                match result {
                    Ok(p) => {
                        match p.status {
                            ProviderStatus::Error => self.error(format!(
                                "{}: {}",
                                p.name,
                                p.problem.clone().unwrap_or_default()
                            )),
                            _ => self.toast(format!("{} connected · {}", p.name, reaches(&p))),
                        }
                        if let Some(slot) = self.providers.list.iter_mut().find(|x| x.id == p.id) {
                            *slot = p;
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            ProvidersMsg::Discovery(id, result) => {
                self.providers.discovery.insert(id, result);
                self.refill_lb_picker();
            }
            ProvidersMsg::Users(users) => self.providers.users = users,
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The Providers screen: the list and the selected connection, or what one token sees.
pub(crate) fn render(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(p) = app.providers.opened() {
        return discovery(frame, area, app, p);
    }
    let view = &app.providers;
    let [list, detail] = Layout::vertical([Constraint::Fill(1), Constraint::Length(8)])
        .spacing(1)
        .areas(area);
    let block = panel(
        title("Providers", true),
        Some(Line::styled(
            "cloud accounts for servers and load balancers",
            theme::muted(),
        )),
        true,
    );
    let inner = inside(&block, list);
    frame.render_widget(block, list);
    mouse::hit(inner, Hit::List(List::Providers));
    if view.list.is_empty() {
        let text = if view.loaded {
            vec![
                Span::styled("no providers yet · ", theme::muted()),
                Span::styled("a", theme::accent()),
                Span::styled(
                    " to connect Hetzner Cloud or Cloudflare for managed load balancers",
                    theme::muted(),
                ),
            ]
        } else {
            vec![Span::styled(
                format!("{} loading", util::spinner(app.tick)),
                theme::muted(),
            )]
        };
        frame.render_widget(Paragraph::new(Line::from(text)), inner);
    } else {
        let rows: Vec<Row> = view
            .list
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let selected = i == view.row;
                let status = if view.checking.contains(&p.id) {
                    Line::styled(
                        format!("{} checking…", util::spinner(app.tick)),
                        theme::fg(theme::INFO),
                    )
                } else {
                    match p.status {
                        ProviderStatus::Error => Line::from(vec![
                            Span::styled("error ", theme::fg(theme::ERR)),
                            Span::styled(p.problem.clone().unwrap_or_default(), theme::muted()),
                        ]),
                        status => Line::styled(status.as_str(), theme::text()),
                    }
                };
                let reach = reaches(p);
                Row::new(vec![
                    Cell::from(status_glyph(p)),
                    Cell::from(Span::styled(
                        p.name.clone(),
                        if selected {
                            theme::accent()
                        } else {
                            theme::text()
                        },
                    )),
                    Cell::from(Span::styled(p.kind.label(), theme::text())),
                    Cell::from(status),
                    Cell::from(Span::styled(
                        if reach.is_empty() {
                            "—".into()
                        } else {
                            reach
                        },
                        theme::muted(),
                    )),
                    Cell::from(Span::styled(
                        p.last_checked_at.map(util::ago).unwrap_or_default(),
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
        let offset = scroll_offset(view.row, inner.height.saturating_sub(1) as usize);
        row_hits(inner, rows.len(), offset, Hit::Item);
        frame.render_widget(
            Table::new(
                rows.into_iter().skip(offset),
                [
                    Constraint::Length(2),
                    Constraint::Length(18),
                    Constraint::Length(16),
                    Constraint::Length(30),
                    Constraint::Fill(1),
                    Constraint::Length(14),
                ],
            )
            .header(
                Row::new(["", "NAME", "KIND", "STATUS", "REACHES", "LAST CHECKED"])
                    .style(theme::muted()),
            )
            .column_spacing(2),
            inner,
        );
    }
    provider_detail(frame, detail, app);
}

fn provider_detail(frame: &mut Frame, area: Rect, app: &App) {
    let Some(p) = app.providers.selected() else {
        frame.render_widget(panel(title("provider", false), None, false), area);
        return;
    };
    let block = panel(title(&p.name, false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        .spacing(4)
        .areas(inner);
    let text = |t: String| vec![Span::styled(t, theme::text())];
    let token = match app.secrets.name(&p.token_secret_id) {
        Some(n) => vec![
            Span::styled(n.to_string(), theme::text()),
            Span::styled("  generic secret", theme::muted()),
        ],
        None => vec![Span::styled("a generic secret", theme::muted())],
    };
    let mut reach = text(p.account.clone().unwrap_or_else(|| "—".into()));
    if let Some(d) = app.providers.discovered(&p.id) {
        let n = d.load_balancers.len();
        if p.kind == ProviderKind::Hetzner {
            reach.push(Span::styled(
                format!("  {n} load balancer{}", if n == 1 { "" } else { "s" }),
                theme::muted(),
            ));
        }
    }
    if p.kind == ProviderKind::Cloudflare && p.load_balancing {
        reach.push(Span::styled("  load balancing", theme::muted()));
    }
    let users = app.providers.users.get(&p.id).cloned().unwrap_or_default();
    let used = match users.first().and_then(|id| app.service_path(id)) {
        Some((project, env, s)) => {
            let mut spans = vec![
                Span::styled(format!("{} load balancer", s.name), theme::text()),
                Span::styled(format!("  {} / {}", project.name, env.name), theme::muted()),
            ];
            if users.len() > 1 {
                spans.push(Span::styled(
                    format!(" · {} more", users.len() - 1),
                    theme::muted(),
                ));
            }
            spans
        }
        None => vec![Span::styled("nothing yet", theme::muted())],
    };
    let rows = lines_of(left, 4);
    for (row, (k, v)) in rows.iter().zip([
        ("kind", text(p.kind.label().into())),
        ("token", token),
        ("reaches", reach),
        ("used by", used),
    ]) {
        setting_row(frame, *row, k, v, None);
    }
    let rows = lines_of(right, 3);
    let checked = p.last_checked_at.map(util::ago).unwrap_or_default();
    let check = match p.status {
        _ if app.providers.checking.contains(&p.id) => vec![Span::styled(
            format!("{} checking…", util::spinner(app.tick)),
            theme::fg(theme::INFO),
        )],
        ProviderStatus::Connected => vec![
            Span::styled("● ", theme::fg(theme::OK)),
            Span::styled("ok", theme::text()),
            Span::styled(format!("  {checked}"), theme::muted()),
        ],
        ProviderStatus::Error => vec![
            Span::styled("✕ ", theme::fg(theme::ERR)),
            Span::styled("failed", theme::text()),
            Span::styled(
                format!("  {} · {checked}", p.problem.clone().unwrap_or_default()),
                theme::muted(),
            ),
        ],
        ProviderStatus::Unknown => vec![Span::styled("never checked", theme::muted())],
    };
    setting_row(frame, rows[0], "last check", check, None);
    setting_row(
        frame,
        rows[1],
        "created",
        text(crate::database::created(p.created_at, chrono::Utc::now())),
        None,
    );
    setting_row(frame, rows[2], "", Vec::new(), Some(("c", "check")));
}

/// "SET-11": what a token sees. Hetzner: the servers matched by address and its load balancers;
/// Cloudflare: the zones it can edit.
fn discovery(frame: &mut Frame, area: Rect, app: &App, p: &Provider) {
    let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)])
        .spacing(1)
        .areas(area);
    let (glyph, status_style) = match p.status {
        ProviderStatus::Connected => ("●", theme::fg(theme::OK)),
        ProviderStatus::Error => ("✕", theme::fg(theme::ERR)),
        ProviderStatus::Unknown => ("○", theme::muted()),
    };
    let mut facts = vec![p.kind.label().to_string()];
    if let Some(a) = &p.account {
        facts.push(match p.kind {
            ProviderKind::Hetzner => format!("{a} in the project"),
            ProviderKind::Cloudflare => a.clone(),
        });
    }
    if let Some(at) = p.last_checked_at {
        facts.push(format!("checked {}", util::ago(at)));
    }
    let mut spans = vec![Span::raw(" ")];
    mouse::span_target(
        head,
        1,
        "Providers".len(),
        Hit::Key(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    spans.extend([
        Span::styled("Providers", theme::muted()),
        Span::styled(" / ", theme::muted()),
        Span::styled(p.name.clone(), theme::text()),
        Span::raw("  "),
        Span::styled(glyph, status_style),
        Span::styled(format!(" {}", p.status.as_str()), status_style),
        Span::styled(format!("   {}", facts.join(" · ")), theme::muted()),
    ]);
    frame.render_widget(Paragraph::new(Line::from(spans)), head);
    let d = match app.providers.discovery.get(&p.id) {
        Some(Ok(d)) => d,
        Some(Err(e)) => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("✕ ", theme::fg(theme::ERR)),
                    Span::styled(e.clone(), theme::text()),
                ])),
                body,
            );
            return;
        }
        None => {
            frame.render_widget(
                Paragraph::new(format!(
                    "{} asking {} what the token sees",
                    util::spinner(app.tick),
                    p.kind.label()
                ))
                .style(theme::muted()),
                body,
            );
            return;
        }
    };
    match p.kind {
        ProviderKind::Hetzner => hetzner(frame, body, app, d),
        ProviderKind::Cloudflare => cloudflare(frame, body, app, p, d),
    }
}

fn hetzner(frame: &mut Frame, area: Rect, app: &App, d: &ProviderDiscovery) {
    let servers_h = d.servers.len() as u16 + 5;
    let lbs_h = d.load_balancers.len().max(1) as u16 + 3;
    let [servers, lbs, _] = Layout::vertical([
        Constraint::Length(servers_h),
        Constraint::Length(lbs_h),
        Constraint::Fill(1),
    ])
    .spacing(1)
    .areas(area);
    let block = panel(
        title("Servers", true),
        Some(Line::styled("matched by public IP", theme::muted())),
        true,
    );
    let inner = inside(&block, servers);
    frame.render_widget(block, servers);
    mouse::hit(inner, Hit::List(List::Providers));
    let dash = || Span::styled("—", theme::muted());
    let rows: Vec<Row> = d
        .servers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let selected = i == app.providers.detail_row;
            let name = app
                .servers
                .iter()
                .find(|s| s.id == m.server_id)
                .map_or(m.server_id.clone(), |s| s.name.clone());
            let mut cells = vec![
                Cell::from(match m.hetzner {
                    Some(_) => Span::styled("●", theme::fg(theme::OK)),
                    None => Span::styled("○", theme::muted()),
                }),
                Cell::from(Span::styled(
                    name,
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                )),
            ];
            match &m.hetzner {
                Some(h) => {
                    let private = h
                        .private_networks
                        .iter()
                        .map(|n| {
                            let net = d
                                .networks
                                .iter()
                                .find(|x| x.id == n.network_id)
                                .map_or(n.network_id.to_string(), |x| x.name.clone());
                            format!("{net} · {}", n.ip)
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    cells.extend([
                        Cell::from(Span::styled(h.name.clone(), theme::text())),
                        Cell::from(Span::styled(
                            m.address.clone().unwrap_or_default(),
                            theme::muted(),
                        )),
                        Cell::from(Span::styled(h.location.clone(), theme::text())),
                        Cell::from(Span::styled(h.network_zone.clone(), theme::text())),
                        Cell::from(if private.is_empty() {
                            dash()
                        } else {
                            Span::styled(private, theme::text())
                        }),
                    ]);
                }
                None => cells.extend([
                    Cell::from(Span::styled("not in this project", theme::muted())),
                    Cell::from(Span::styled(
                        m.address.clone().unwrap_or_default(),
                        theme::muted(),
                    )),
                    Cell::from(dash()),
                    Cell::from(dash()),
                    Cell::from(dash()),
                ]),
            }
            Row::new(cells).style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    let n = rows.len();
    row_hits(inner, n, 0, Hit::Item);
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Length(16),
                Constraint::Length(20),
                Constraint::Length(16),
                Constraint::Length(10),
                Constraint::Length(14),
                Constraint::Fill(1),
            ],
        )
        .header(
            Row::new([
                "",
                "SERVER",
                "HETZNER",
                "ADDRESS",
                "LOCATION",
                "ZONE",
                "PRIVATE NETWORK",
            ])
            .style(theme::muted()),
        )
        .column_spacing(2),
        inner,
    );
    if inner.height as usize > n + 2 {
        frame.render_widget(
            Paragraph::new(
                "Servers are matched to Hetzner by public IP. A load balancer only reaches servers in its own network zone.",
            )
            .style(theme::muted()),
            Rect::new(inner.x, inner.y + n as u16 + 2, inner.width, 1),
        );
    }

    let block = panel(
        title("Load balancers", true),
        Some(Line::styled("in this project", theme::muted())),
        true,
    );
    let inner = inside(&block, lbs);
    frame.render_widget(block, lbs);
    if d.load_balancers.is_empty() {
        frame.render_widget(
            Paragraph::new("none in this project yet").style(theme::muted()),
            inner,
        );
        return;
    }
    let rows: Vec<Row> = d
        .load_balancers
        .iter()
        .map(|l| {
            Row::new(vec![
                Cell::from(if l.managed {
                    Span::styled("●", theme::fg(theme::OK))
                } else {
                    Span::styled("○", theme::muted())
                }),
                Cell::from(Span::styled(l.name.clone(), theme::text())),
                Cell::from(Span::styled(l.lb_type.clone(), theme::text())),
                Cell::from(Span::styled(l.location.clone(), theme::text())),
                Cell::from(Span::styled(
                    l.public_ipv4.clone().unwrap_or_default(),
                    theme::muted(),
                )),
                Cell::from(if l.managed {
                    Span::styled(format!("by {}", brand::NAME.to_lowercase()), theme::text())
                } else {
                    Span::styled(
                        format!("not managed by {}", brand::NAME.to_lowercase()),
                        theme::muted(),
                    )
                }),
            ])
        })
        .collect();
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Length(20),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(18),
                Constraint::Fill(1),
            ],
        )
        .header(
            Row::new(["", "NAME", "TYPE", "LOCATION", "ADDRESS", "MANAGED"]).style(theme::muted()),
        )
        .column_spacing(2),
        inner,
    );
}

fn cloudflare(frame: &mut Frame, area: Rect, app: &App, p: &Provider, d: &ProviderDiscovery) {
    let zones_h = d.zones.len().max(1) as u16 + 5;
    let [zones, _] = Layout::vertical([Constraint::Length(zones_h), Constraint::Fill(1)])
        .spacing(1)
        .areas(area);
    let block = panel(
        title("Zones", true),
        Some(Line::styled("the token can edit DNS in", theme::muted())),
        true,
    );
    let inner = inside(&block, zones);
    frame.render_widget(block, zones);
    mouse::hit(inner, Hit::List(List::Providers));
    let rows: Vec<Row> = d
        .zones
        .iter()
        .enumerate()
        .map(|(i, z)| {
            let selected = i == app.providers.detail_row;
            Row::new(vec![
                Cell::from(Span::styled(
                    "●",
                    if z.status == "active" {
                        theme::fg(theme::OK)
                    } else {
                        theme::muted()
                    },
                )),
                Cell::from(Span::styled(
                    z.name.clone(),
                    if selected {
                        theme::accent()
                    } else {
                        theme::text()
                    },
                )),
                Cell::from(Span::styled(z.status.clone(), theme::text())),
                Cell::from(Span::styled(z.account_name.clone(), theme::muted())),
            ])
            .style(if selected {
                theme::selected()
            } else {
                Style::new()
            })
        })
        .collect();
    let n = rows.len();
    row_hits(inner, n, 0, Hit::Item);
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Length(28),
                Constraint::Length(12),
                Constraint::Fill(1),
            ],
        )
        .header(Row::new(["", "ZONE", "STATUS", "ACCOUNT"]).style(theme::muted()))
        .column_spacing(2),
        inner,
    );
    let note = if p.load_balancing {
        "Load Balancing is on this account: Cloudflare Load Balancing is offered."
    } else {
        "No Load Balancing on this account: Cloudflare DNS spreads traffic with proxied records."
    };
    if inner.height as usize > n + 2 {
        frame.render_widget(
            Paragraph::new(truncate(note, inner.width as usize)).style(theme::muted()),
            Rect::new(inner.x, inner.y + n as u16 + 2, inner.width, 1),
        );
    }
}

/// The Providers summary on the Settings screen.
pub(crate) fn summary(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel(title("Providers", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    let list = &app.providers.list;
    let connected = list
        .iter()
        .filter(|p| p.status == ProviderStatus::Connected)
        .count();
    let value = if list.is_empty() {
        vec![Span::styled(
            "none yet · Hetzner Cloud and Cloudflare for load balancers",
            theme::muted(),
        )]
    } else {
        vec![
            Span::styled(list.len().to_string(), theme::text()),
            Span::styled(
                format!(
                    "  {connected} connected · {} need attention",
                    list.len() - connected
                ),
                theme::muted(),
            ),
        ]
    };
    if let Some(row) = lines_of(inner, 1).first() {
        setting_row(frame, *row, "providers", value, None);
    }
    if inner.height >= 3 {
        let line = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
        mouse::hints(frame, line, &[("p", "open providers")], Alignment::Left);
    }
}
