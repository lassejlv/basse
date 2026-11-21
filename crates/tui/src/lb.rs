//! Placement across servers and managed load balancers: the placement dialog, the load balancer
//! form, its plan, status, drift and turning it off, following the "SVC-33" to "SVC-39"
//! artboards in Paper.

use crate::app::{App, Msg, Overlay, Tab};
use crate::form::{Field, Form, FormKind};
use crate::mouse::{self, Hit};
use crate::picker::{PickFor, PickItem, PickKind, Picker};
use crate::theme;
use crate::util::{self, truncate};
use crate::view::{action_chip, dialog, inside, panel, rule_line, title};
use common::lb::*;
use common::{Id, Placement, PlacementStrategy, ScaleCapacity, ServerStatus, ServiceDetail, brand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Results of placement and load balancer requests, applied on the UI thread.
pub enum LbMsg {
    /// Room on each server, for the placement dialog.
    Capacity(Id, Result<ScaleCapacity, String>),
    /// The placement dialog's request finished.
    Placed(Result<String, String>),
    Plan(Id, Result<LbPlan, String>),
    Enabled(Id, Result<ServiceLb, String>),
    Status(Id, Result<Option<ServiceLb>, String>),
    Synced(Id, Result<ServiceLb, String>),
    Disabled(Id, Result<LbTeardown, String>),
}

/// How a strategy spreads replicas, as the Scaling panel says it.
pub fn strategy_note(s: PlacementStrategy) -> &'static str {
    match s {
        PlacementStrategy::Spread => "one replica per server before doubling up",
        PlacementStrategy::Pack => "fills servers in order before using the next",
    }
}

/// "Hetzner", "Cloudflare DNS": a mode in a few words.
pub fn mode_short(mode: LbMode) -> &'static str {
    match mode {
        LbMode::Hetzner => "Hetzner",
        LbMode::CloudflareDns => "Cloudflare DNS",
        LbMode::CloudflareLb => "Cloudflare LB",
    }
}

fn state_style(state: LbSyncState) -> Style {
    match state {
        LbSyncState::InSync => theme::fg(theme::OK),
        LbSyncState::Pending => theme::muted(),
        LbSyncState::Drift | LbSyncState::Error => theme::fg(theme::ERR),
    }
}

fn state_glyph(state: LbSyncState) -> &'static str {
    match state {
        LbSyncState::Pending => "○",
        _ => "●",
    }
}

fn healthy(lb: &ServiceLb) -> usize {
    lb.targets
        .iter()
        .filter(|t| t.provider_health.as_deref() == Some("healthy"))
        .count()
}

/// The Load balancer row of the Scaling panel: "● Hetzner · lb11 · in sync · 3/3 targets healthy".
pub fn summary_line(lb: Option<&ServiceLb>) -> Vec<Span<'static>> {
    let Some(lb) = lb else {
        return vec![Span::styled("○ off", theme::muted())];
    };
    let style = state_style(lb.state);
    let mut what = mode_short(lb.config.mode).to_string();
    if lb.config.mode == LbMode::Hetzner {
        what.push_str(&format!(
            " · {}",
            lb.config.lb_type.as_deref().unwrap_or("lb11")
        ));
    }
    vec![
        Span::styled(format!("{} ", state_glyph(lb.state)), style),
        Span::styled(format!("{what} · "), theme::text()),
        Span::styled(lb.state.as_str(), style),
        Span::styled(
            format!(" · {}/{} targets healthy", healthy(lb), lb.targets.len()),
            theme::muted(),
        ),
    ]
}

fn money(amount: f64, currency: &str) -> String {
    match currency {
        "EUR" => format!("€{amount:.2}"),
        c => format!("{amount:.2} {c}"),
    }
}

/// "10k", "1.5k" or "40": a connection limit in a few characters, so it fits a form field.
fn compact(n: u32) -> String {
    match n {
        0..1000 => n.to_string(),
        n if n.is_multiple_of(1000) => format!("{}k", n / 1000),
        n => format!("{:.1}k", f64::from(n) / 1000.0),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// Placement

/// A server in the placement dialog.
pub struct PlacementRow {
    pub server_id: Id,
    pub name: String,
    pub address: String,
    pub local: bool,
    pub checked: bool,
    /// Why it can't take replicas, e.g. "offline", with when it was last checked.
    pub down: Option<(String, String)>,
}

/// Picks the servers a service's replicas may run on, the strategy and volumes per server.
pub struct PlacementDialog {
    pub service_id: Id,
    pub name: String,
    pub home: Id,
    pub rows: Vec<PlacementRow>,
    pub row: usize,
    pub strategy: PlacementStrategy,
    pub volumes_per_server: bool,
    /// Volumes of the service, as name and mount path.
    pub volumes: Vec<(String, String)>,
    pub capacity: Option<Result<ScaleCapacity, String>>,
    pub busy: bool,
}

impl PlacementDialog {
    pub fn chosen(&self) -> Vec<Id> {
        self.rows
            .iter()
            .filter(|r| r.checked)
            .map(|r| r.server_id.clone())
            .collect()
    }

    /// Several servers would each write their volumes without a copy per server.
    pub fn refused(&self) -> bool {
        self.chosen().len() > 1 && !self.volumes.is_empty() && !self.volumes_per_server
    }

    fn ready(&self) -> bool {
        !self.chosen().is_empty() && !self.refused()
    }

    pub fn toggle(&mut self, i: usize) {
        if let Some(r) = self.rows.get_mut(i)
            && (r.down.is_none() || r.checked)
        {
            r.checked = !r.checked;
        }
    }

    fn placement(&self) -> Placement {
        let chosen = self.chosen();
        Placement {
            servers: if chosen == [self.home.clone()] {
                Vec::new()
            } else {
                chosen
            },
            strategy: self.strategy,
            volumes_per_server: self.volumes_per_server,
        }
    }
}

impl App {
    fn open_detail(&self) -> Option<&ServiceDetail> {
        self.service.as_ref().and_then(|v| v.detail.as_ref())
    }

    /// p on the Settings tab: which servers replicas run on.
    pub fn open_placement(&mut self) {
        let Some(d) = self.open_detail() else {
            return;
        };
        let s = &d.service;
        if !crate::replicas::scalable(s) {
            return self.error(crate::replicas::unsupported(s));
        }
        let placement = &s.scaling.placement;
        let chosen = placement.servers_or(&s.server_id);
        let mut servers: Vec<&common::Server> = chosen
            .iter()
            .filter_map(|id| self.servers.iter().find(|x| &x.id == id))
            .collect();
        servers.extend(self.servers.iter().filter(|x| !chosen.contains(&x.id)));
        let rows = servers
            .into_iter()
            .map(|x| PlacementRow {
                server_id: x.id.clone(),
                name: x.name.clone(),
                address: self.server_address(x),
                local: x.kind == common::ServerKind::Local,
                checked: chosen.contains(&x.id),
                down: (x.status == ServerStatus::Offline).then(|| {
                    let checked = x
                        .last_checked_at
                        .map_or(String::new(), |at| format!("checked {}, ", util::ago(at)));
                    (
                        "offline".to_string(),
                        format!("{checked}can't take replicas"),
                    )
                }),
            })
            .collect();
        let id = s.id.clone();
        self.overlay = Some(Overlay::Placement(PlacementDialog {
            service_id: s.id.clone(),
            name: s.name.clone(),
            home: s.server_id.clone(),
            rows,
            row: 0,
            strategy: placement.strategy,
            volumes_per_server: placement.volumes_per_server,
            volumes: d
                .volumes
                .iter()
                .map(|v| (v.name.clone(), v.mount_path.clone()))
                .collect(),
            capacity: None,
            busy: false,
        }));
        self.spawn(|c| async move {
            let capacity = c.scale_capacity(&id).await.map_err(|e| format!("{e:#}"));
            Msg::Lb(LbMsg::Capacity(id, capacity))
        });
    }

    fn submit_placement(&mut self) {
        let Some(Overlay::Placement(d)) = self.overlay.as_mut() else {
            return;
        };
        if d.busy || !d.ready() {
            return;
        }
        d.busy = true;
        let (id, name, body) = (d.service_id.clone(), d.name.clone(), d.placement());
        let n = d.chosen().len();
        self.spawn(move |c| async move {
            Msg::Lb(LbMsg::Placed(
                c.set_placement(&id, &body)
                    .await
                    .map(|_| {
                        format!(
                            "{name} runs on {n} server{} · replicas move in the background",
                            plural(n)
                        )
                    })
                    .map_err(|e| format!("{e:#}")),
            ))
        });
    }

    /// Changes one part of the placement from the Scaling panel.
    pub fn change_placement(&mut self, edit: impl FnOnce(&mut Placement) -> String) {
        let Some(d) = self.open_detail() else {
            return;
        };
        let mut p = d.service.scaling.placement.clone();
        let id = d.service.id.clone();
        let toast = edit(&mut p);
        self.act(toast, |c| async move { c.set_placement(&id, &p).await });
    }

    // Load balancer

    /// l on the Settings tab: the load balancer's status, or the form that turns one on.
    pub fn open_lb(&mut self) {
        let Some(d) = self.open_detail() else {
            return;
        };
        let s = &d.service;
        if !crate::replicas::scalable(s) {
            return self.error(crate::replicas::unsupported(s));
        }
        let has = d.lb.is_some();
        let id = s.id.clone();
        let hetzner =
            d.lb.as_ref()
                .filter(|lb| lb.config.mode == LbMode::Hetzner)
                .map(|lb| lb.config.provider_id.clone());
        self.load_providers();
        if let Some(provider) = hetzner
            && !self.providers.discovery.contains_key(&provider)
        {
            self.load_discovery(&provider);
        }
        if !has {
            return self.form_enable_lb();
        }
        if let Some(view) = self.service.as_mut() {
            view.lb_open = true;
            view.row = 0;
        }
        self.spawn(|c| async move {
            let lb = c.service_lb(&id).await.map_err(|e| format!("{e:#}"));
            Msg::Lb(LbMsg::Status(id, lb))
        });
    }

    pub fn close_lb(&mut self) {
        if let Some(view) = self.service.as_mut() {
            view.lb_open = false;
            view.row = 0;
        }
    }

    pub fn form_enable_lb(&mut self) {
        let Some(d) = self.open_detail() else {
            return;
        };
        let s = &d.service;
        let config = d.lb.as_ref().map(|lb| lb.config.clone());
        let providers = &self.providers;
        let provider = config
            .as_ref()
            .and_then(|c| providers.get(&c.provider_id))
            .or_else(|| {
                providers.list.iter().find(|p| {
                    p.kind == ProviderKind::Hetzner && p.status == ProviderStatus::Connected
                })
            })
            .or(providers.list.first());
        let mode = config.as_ref().map_or(
            match provider.map(|p| p.kind) {
                Some(ProviderKind::Cloudflare) => LbMode::CloudflareDns,
                _ => LbMode::Hetzner,
            },
            |c| c.mode,
        );
        let modes = LbMode::ALL.iter().map(|m| m.label().to_string()).collect();
        let network = config.as_ref().and_then(|c| c.network_id);
        let dns = config.as_ref().and_then(|c| c.dns_provider_id.clone());
        let lb_type = config
            .as_ref()
            .and_then(|c| c.lb_type.clone())
            .unwrap_or_else(|| "lb11".into());
        let location = config
            .as_ref()
            .and_then(|c| c.location.clone())
            .unwrap_or_default();
        let all_domains: Vec<String> = d.domains.iter().map(|x| x.host.clone()).collect();
        let disc = provider.and_then(|p| providers.discovered(&p.id));
        let type_detail = disc
            .and_then(|x| x.lb_types.iter().find(|t| t.name == lb_type))
            .map_or(String::new(), |t| {
                format!(
                    "{} targets · {} connections",
                    t.max_targets,
                    compact(t.max_connections)
                )
            });
        let location_detail = disc
            .and_then(|x| x.locations.iter().find(|l| l.name == location))
            .map_or_else(
                || {
                    if location.is_empty() {
                        "where the first Hetzner server is".to_string()
                    } else {
                        String::new()
                    }
                },
                |l| format!("{} · {}", l.city, l.network_zone),
            );
        let net = network.map(|id| {
            disc.and_then(|x| x.networks.iter().find(|n| n.id == id))
                .map_or((id.to_string(), String::new()), |n| {
                    (n.name.clone(), n.ip_range.clone())
                })
        });
        let fields = vec![
            Field::pick(
                "provider",
                "provider",
                provider.map(|p| p.id.clone()).unwrap_or_default(),
                provider.map(|p| p.name.clone()).unwrap_or_default(),
            )
            .placeholder(
                provider.map_or("connect one on the Providers screen", |p| p.kind.label()),
            ),
            Field::choice("mode", "mode", modes)
                .selected(LbMode::ALL.iter().position(|m| *m == mode).unwrap_or(0)),
            Field::pick("lb_type", "type", lb_type.clone(), lb_type).placeholder(type_detail),
            Field::pick("location", "location", location.clone(), location)
                .placeholder(location_detail),
            Field::pick(
                "network",
                "private network",
                network.map(|n| n.to_string()).unwrap_or_default(),
                net.as_ref()
                    .map_or("— none".into(), |(name, _)| name.clone()),
            )
            .placeholder(net.map_or("public addresses".into(), |(_, range)| range))
            .hint("optional"),
            Field::pick(
                "dns",
                "dns via",
                dns.clone().unwrap_or_default(),
                dns.as_deref()
                    .map_or("— none".into(), |id| providers.name(id)),
            )
            .placeholder(if dns.is_some() {
                ""
            } else {
                "records to add by hand"
            })
            .hint("or show records"),
            Field::text("domains", "domains")
                .value(
                    config
                        .as_ref()
                        .map(|c| c.domains.join(", "))
                        .unwrap_or_default(),
                )
                .placeholder(if all_domains.is_empty() {
                    "the service has no domains yet".to_string()
                } else {
                    all_domains.join(", ")
                })
                .hint("empty means all"),
        ];
        let hetzner = provider
            .filter(|p| p.kind == ProviderKind::Hetzner)
            .map(|p| p.id.clone());
        let form = Form::new(
            FormKind::EnableLb {
                service_id: s.id.clone(),
            },
            "Load balancer",
            fields,
        )
        .subtitle(format!("{} · enable", s.name))
        .note(vec![format!(
            "Targets follow healthy replicas. {} only ever deletes what it created itself.",
            brand::NAME
        )])
        .submit("review plan", None);
        self.overlay = Some(Overlay::Form(form));
        if let Some(id) = hetzner
            && !self.providers.discovery.contains_key(&id)
        {
            self.load_discovery(&id);
        }
    }

    /// ⏎ on a pick field of the load balancer form.
    pub fn pick_lb_field(&mut self, form: Form, key: &'static str) {
        let title = match key {
            "provider" => "Provider",
            "lb_type" => "Load balancer type",
            "location" => "Location",
            "network" => "Private network",
            _ => "DNS via",
        };
        let subtitle = form.subtitle.clone();
        let current = Some(form.raw(key)).filter(|v| !v.is_empty());
        let mut picker = Picker {
            title: title.into(),
            subtitle,
            kind: PickKind::Fixed,
            items: Vec::new(),
            row: 0,
            target: PickFor::Field(Box::new(form), key),
            current,
            note: Vec::new(),
            busy: false,
            git: None,
        };
        self.fill_lb_picker(&mut picker);
        picker.row = picker
            .current
            .as_ref()
            .and_then(|c| picker.items.iter().position(|i| i.id.as_ref() == Some(c)))
            .unwrap_or(0);
        self.overlay = Some(Overlay::Picker(picker));
    }

    /// Rebuild an open load balancer picker once providers or discovery arrive.
    pub fn refill_lb_picker(&mut self) {
        if !matches!(self.overlay, Some(Overlay::Picker(_))) {
            return;
        }
        let Some(Overlay::Picker(mut p)) = self.overlay.take() else {
            return;
        };
        if p.kind == PickKind::Fixed
            && matches!(&p.target, PickFor::Field(f, _) if matches!(f.kind, FormKind::EnableLb { .. }))
        {
            let selected = p.selected().and_then(|i| i.id.clone());
            self.fill_lb_picker(&mut p);
            p.row = selected
                .and_then(|id| p.items.iter().position(|i| i.id.as_ref() == Some(&id)))
                .unwrap_or(p.row)
                .min(p.items.len().saturating_sub(1));
        }
        self.overlay = Some(Overlay::Picker(p));
    }

    fn fill_lb_picker(&self, p: &mut Picker) {
        let PickFor::Field(form, key) = &p.target else {
            return;
        };
        let providers = &self.providers;
        let muted = |s: String| vec![Line::styled(s, theme::muted())];
        let provider = providers.get(&form.raw("provider"));
        match *key {
            "provider" => {
                p.items = providers
                    .list
                    .iter()
                    .map(|x| {
                        let mut item = PickItem::new(Some(x.id.clone()), &x.name, x.kind.label());
                        item.note = match x.status {
                            ProviderStatus::Error => x.problem.clone().unwrap_or_default(),
                            _ => crate::providers::reaches(x),
                        };
                        item
                    })
                    .collect();
                p.note = muted("Connect more on the Providers screen: p in Settings.".into());
            }
            "dns" => {
                p.items = std::iter::once(PickItem::new(
                    None,
                    "— none",
                    "show the records to add by hand",
                ))
                .chain(
                    providers
                        .list
                        .iter()
                        .filter(|x| x.kind == ProviderKind::Cloudflare)
                        .map(|x| {
                            let mut item =
                                PickItem::new(Some(x.id.clone()), &x.name, x.kind.label());
                            item.note = crate::providers::reaches(x);
                            item
                        }),
                )
                .collect();
                p.note = muted(
                    "A Cloudflare connection points the domains' records at the load balancer."
                        .into(),
                );
            }
            _ => {
                p.items = Vec::new();
                let Some(provider) = provider.filter(|x| x.kind == ProviderKind::Hetzner) else {
                    p.note =
                        muted("Only a Hetzner Cloud provider has these; pick one first.".into());
                    return;
                };
                let d = match providers.discovery.get(&provider.id) {
                    Some(Ok(d)) => d,
                    Some(Err(e)) => {
                        p.note = vec![Line::styled(format!("✕ {e}"), theme::fg(theme::ERR))];
                        return;
                    }
                    None => {
                        self.load_discovery(&provider.id);
                        p.note = muted(format!(
                            "{} asking {} what the token sees…",
                            util::spinner(self.tick),
                            provider.name
                        ));
                        return;
                    }
                };
                let location = form.raw("location");
                match *key {
                    "lb_type" => {
                        p.items = d
                            .lb_types
                            .iter()
                            .map(|t| {
                                let mut item = PickItem::new(
                                    Some(t.name.clone()),
                                    &t.name,
                                    format!(
                                        "{} targets · {} connections",
                                        t.max_targets,
                                        compact(t.max_connections)
                                    ),
                                );
                                let price = t
                                    .prices
                                    .iter()
                                    .find(|x| x.location == location)
                                    .or(t.prices.first());
                                item.note = price.map_or(String::new(), |x| {
                                    format!("{}/month", money(x.monthly, &x.currency))
                                });
                                item
                            })
                            .collect();
                        p.note = muted("Hetzner list prices, billed by the hour.".into());
                    }
                    "location" => {
                        p.items = std::iter::once(PickItem::new(
                            None,
                            "— default",
                            "where the first Hetzner server is",
                        ))
                        .chain(d.locations.iter().map(|l| {
                            PickItem::new(
                                Some(l.name.clone()),
                                &l.name,
                                format!("{} · {}", l.city, l.network_zone),
                            )
                        }))
                        .collect();
                        p.note = muted(
                            "A load balancer reaches servers in its own network zone.".into(),
                        );
                    }
                    _ => {
                        p.items =
                            std::iter::once(PickItem::new(
                                None,
                                "— none",
                                "reach the servers on their public addresses",
                            ))
                            .chain(d.networks.iter().map(|n| {
                                PickItem::new(Some(n.id.to_string()), &n.name, &n.ip_range)
                            }))
                            .collect();
                        p.note = muted("Only when every Hetzner server is in the network.".into());
                    }
                }
            }
        }
    }

    /// ⏎ on the form: ask the daemon what enabling would do.
    pub fn submit_lb_plan(&mut self) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        let FormKind::EnableLb { service_id } = form.kind.clone() else {
            return;
        };
        let config = match config_from(form, &self.providers) {
            Ok(c) => c,
            Err(e) => return form.fail(e),
        };
        form.busy = true;
        form.error = None;
        self.spawn(move |c| async move {
            let plan = c
                .plan_lb(&service_id, &config)
                .await
                .map_err(|e| format!("{e:#}"));
            Msg::Lb(LbMsg::Plan(service_id, plan))
        });
    }

    fn enable_lb(&mut self) {
        let Some(Overlay::LbPlan(d)) = self.overlay.as_mut() else {
            return;
        };
        if d.busy {
            return;
        }
        d.busy = true;
        d.error = None;
        let id = d.service_id.clone();
        let body = EnableLb {
            config: d.config.clone(),
            confirm: true,
        };
        self.spawn(move |c| async move {
            let lb = c.enable_lb(&id, &body).await.map_err(|e| format!("{e:#}"));
            Msg::Lb(LbMsg::Enabled(id, lb))
        });
    }

    pub fn sync_lb(&mut self, force: bool) {
        let Some(id) = self.service.as_ref().map(|v| v.id.clone()) else {
            return;
        };
        self.progress(if force {
            "force syncing · overwriting changes made by hand…"
        } else {
            "syncing…"
        });
        self.spawn(move |c| async move {
            let lb = c.sync_lb(&id, force).await.map_err(|e| format!("{e:#}"));
            Msg::Lb(LbMsg::Synced(id, lb))
        });
    }

    /// x on the status view: what turning it off deletes and keeps.
    pub fn ask_disable_lb(&mut self) {
        let Some(d) = self.open_detail() else {
            return;
        };
        let Some(lb) = d.lb.clone() else {
            return;
        };
        self.overlay = Some(Overlay::LbDisable(DisableDialog {
            service_id: d.service.id.clone(),
            name: d.service.name.clone(),
            lb,
            busy: false,
        }));
    }

    fn disable_lb(&mut self, delete: bool) {
        let Some(Overlay::LbDisable(d)) = self.overlay.as_mut() else {
            return;
        };
        if d.busy {
            return;
        }
        d.busy = true;
        let id = d.service_id.clone();
        self.spawn(move |c| async move {
            let t = c
                .disable_lb(&id, delete)
                .await
                .map_err(|e| format!("{e:#}"));
            Msg::Lb(LbMsg::Disabled(id, t))
        });
    }

    /// Store a fresh status in the open service and the cached detail.
    fn set_lb(&mut self, id: &str, lb: Option<ServiceLb>) {
        if let Some(d) = self
            .service
            .as_mut()
            .filter(|v| v.id == id)
            .and_then(|v| v.detail.as_mut())
        {
            d.lb = lb.clone();
        }
        if let Some(d) = self.details.get_mut(id) {
            d.lb = lb;
        }
    }

    pub fn apply_lb(&mut self, msg: LbMsg) {
        match msg {
            LbMsg::Capacity(id, capacity) => {
                if let Some(Overlay::Placement(d)) = self.overlay.as_mut()
                    && d.service_id == id
                {
                    d.capacity = Some(capacity);
                }
            }
            LbMsg::Placed(result) => {
                if matches!(self.overlay, Some(Overlay::Placement(_))) {
                    self.overlay = None;
                }
                match result {
                    Ok(t) => self.toast(t),
                    Err(e) => self.error(e),
                }
                self.refresh();
            }
            LbMsg::Plan(id, result) => {
                let Some(Overlay::Form(form)) = self.overlay.as_mut() else {
                    return;
                };
                if form.kind
                    != (FormKind::EnableLb {
                        service_id: id.clone(),
                    })
                {
                    return;
                }
                let plan = match result {
                    Ok(plan) => plan,
                    Err(e) => return form.fail(e),
                };
                let Ok(config) = config_from(form, &self.providers) else {
                    return;
                };
                let Some(Overlay::Form(mut form)) = self.overlay.take() else {
                    return;
                };
                form.busy = false;
                let name = self
                    .open_detail()
                    .map_or(String::new(), |d| d.service.name.clone());
                self.overlay = Some(Overlay::LbPlan(PlanDialog {
                    service_id: id,
                    name,
                    form: Box::new(form),
                    config,
                    plan,
                    busy: false,
                    error: None,
                }));
            }
            LbMsg::Enabled(id, result) => match result {
                Ok(lb) => {
                    if matches!(self.overlay, Some(Overlay::LbPlan(_))) {
                        self.overlay = None;
                    }
                    self.toast(format!(
                        "{} on · it syncs in the background",
                        lb.config.mode.label()
                    ));
                    self.set_lb(&id, Some(lb));
                    if let Some(view) = self.service.as_mut().filter(|v| v.id == id) {
                        view.tab = Tab::Settings;
                        view.lb_open = true;
                        view.row = 0;
                    }
                    self.load_provider_users();
                }
                Err(e) => {
                    if let Some(Overlay::LbPlan(d)) = self.overlay.as_mut() {
                        d.busy = false;
                        d.error = Some(e);
                    } else {
                        self.error(e);
                    }
                }
            },
            LbMsg::Status(id, result) => match result {
                Ok(lb) => self.set_lb(&id, lb),
                Err(e) => self.error(e),
            },
            LbMsg::Synced(id, result) => match result {
                Ok(lb) => {
                    match lb.state {
                        LbSyncState::Error => self.error(format!(
                            "sync failed · {}",
                            lb.last_error.clone().unwrap_or_default()
                        )),
                        LbSyncState::Drift => {
                            self.error("changed by hand at the provider · f to overwrite")
                        }
                        state => self.toast(format!("synced · {}", state.as_str())),
                    }
                    self.set_lb(&id, Some(lb));
                }
                Err(e) => self.error(e),
            },
            LbMsg::Disabled(id, result) => {
                if matches!(self.overlay, Some(Overlay::LbDisable(_))) {
                    self.overlay = None;
                }
                match result {
                    Ok(t) => {
                        self.toast(format!(
                            "load balancer off · {} deleted, {} kept",
                            t.deleted.len(),
                            t.kept.len()
                        ));
                        self.set_lb(&id, None);
                        self.close_lb();
                        self.load_provider_users();
                    }
                    Err(e) => self.error(e),
                }
            }
        }
    }
}

/// The form's answers as the daemon takes them.
pub fn config_from(
    form: &Form,
    providers: &crate::providers::ProvidersView,
) -> Result<LbConfig, String> {
    let provider_id = form.raw("provider");
    let Some(provider) = providers.get(&provider_id) else {
        return Err("pick a provider; connect one on the Providers screen first".into());
    };
    let mode = LbMode::ALL[form.choice("mode").min(LbMode::ALL.len() - 1)];
    if mode.provider() != provider.kind {
        return Err(format!(
            "{} is a {} connection; {} needs a {} one",
            provider.name,
            provider.kind.label(),
            mode.label(),
            mode.provider().label()
        ));
    }
    if mode == LbMode::CloudflareLb && !provider.load_balancing {
        return Err(format!(
            "{} has no Load Balancing add-on; use Cloudflare DNS",
            provider.name
        ));
    }
    let hetzner = mode == LbMode::Hetzner;
    let opt = |key: &str| Some(form.raw(key)).filter(|v| hetzner && !v.is_empty());
    let network_id = match opt("network") {
        Some(n) => Some(
            n.parse::<i64>()
                .map_err(|_| format!("'{n}' is not a network id"))?,
        ),
        None => None,
    };
    Ok(LbConfig {
        provider_id,
        mode,
        domains: form
            .get("domains")
            .split([',', ' '])
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .collect(),
        lb_type: opt("lb_type"),
        location: opt("location"),
        network_id,
        adopt_lb_id: None,
        dns_provider_id: opt("dns"),
        proxied: None,
        adopt_records: false,
    })
}

/// After a provider is picked, the mode follows its kind.
pub fn provider_picked(form: &mut Form, providers: &crate::providers::ProvidersView) {
    let Some(kind) = providers.get(&form.raw("provider")).map(|p| p.kind) else {
        return;
    };
    let mode = LbMode::ALL[form.choice("mode").min(LbMode::ALL.len() - 1)];
    if mode.provider() == kind {
        return;
    }
    let next = match kind {
        ProviderKind::Hetzner => LbMode::Hetzner,
        ProviderKind::Cloudflare => LbMode::CloudflareDns,
    };
    if let Some(f) = form.field_mut("mode") {
        f.choice = LbMode::ALL.iter().position(|m| *m == next).unwrap_or(0);
    }
    form.refresh();
}

/// What enabling would do, shown before it happens.
pub struct PlanDialog {
    pub service_id: Id,
    pub name: String,
    /// The form, for going back to edit it.
    pub form: Box<Form>,
    pub config: LbConfig,
    pub plan: LbPlan,
    pub busy: bool,
    pub error: Option<String>,
}

/// Turning a load balancer off: delete what was created, or keep it.
pub struct DisableDialog {
    pub service_id: Id,
    pub name: String,
    pub lb: ServiceLb,
    pub busy: bool,
}

// Keys

pub fn placement_keys(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Placement(d)) = app.overlay.as_mut() else {
        return;
    };
    if d.busy {
        return;
    }
    match key.code {
        KeyCode::Esc => app.overlay = None,
        KeyCode::Enter => app.submit_placement(),
        KeyCode::Up | KeyCode::Char('k') => d.row = d.row.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            d.row = (d.row + 1).min(d.rows.len().saturating_sub(1))
        }
        KeyCode::Char(' ') => {
            let row = d.row;
            d.toggle(row);
        }
        KeyCode::Char('s') | KeyCode::Left | KeyCode::Right => {
            d.strategy = match d.strategy {
                PlacementStrategy::Spread => PlacementStrategy::Pack,
                PlacementStrategy::Pack => PlacementStrategy::Spread,
            }
        }
        KeyCode::Char('v') => d.volumes_per_server = !d.volumes_per_server,
        _ => {}
    }
}

pub fn plan_keys(app: &mut App, key: KeyEvent) {
    let Some(Overlay::LbPlan(d)) = app.overlay.as_mut() else {
        return;
    };
    if d.busy {
        return;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.overlay = None,
        KeyCode::Char('y') | KeyCode::Enter => app.enable_lb(),
        KeyCode::Char('b') | KeyCode::BackTab => {
            if let Some(Overlay::LbPlan(d)) = app.overlay.take() {
                app.overlay = Some(Overlay::Form(*d.form));
            }
        }
        _ => {}
    }
}

pub fn disable_keys(app: &mut App, key: KeyEvent) {
    let Some(Overlay::LbDisable(d)) = app.overlay.as_mut() else {
        return;
    };
    if d.busy {
        return;
    }
    let created = d.lb.resources.iter().any(|r| r.created);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.overlay = None,
        KeyCode::Char('y') | KeyCode::Enter => app.disable_lb(created),
        KeyCode::Char('k') if created => app.disable_lb(false),
        _ => {}
    }
}

/// Keys of the status view; returns false for keys it leaves to the tab.
pub fn status_keys(app: &mut App, key: KeyEvent) -> bool {
    let rows = app
        .open_detail()
        .and_then(|d| d.lb.as_ref())
        .map_or(0, |lb| lb.targets.len());
    let Some(view) = app.service.as_mut() else {
        return false;
    };
    match key.code {
        KeyCode::Esc => app.close_lb(),
        KeyCode::Up | KeyCode::Char('k') => view.row = view.row.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => view.row = (view.row + 1).min(rows.saturating_sub(1)),
        KeyCode::Char('s') => app.sync_lb(false),
        KeyCode::Char('f') => app.sync_lb(true),
        KeyCode::Char('x') => app.ask_disable_lb(),
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('1'..='9') => return false,
        _ => {}
    }
    true
}

// Drawing

/// "SVC-34": the servers replicas may run on, with room on each.
pub(crate) fn render_placement(frame: &mut Frame, area: Rect, d: &PlacementDialog, tick: usize) {
    let warning = d.chosen().len() > 1 && !d.volumes.is_empty();
    let rows = d.rows.len() as u16;
    let inner = dialog(
        frame,
        area,
        110,
        rows + 12 + u16::from(!d.volumes.is_empty()),
        vec![
            Span::styled("Placement", theme::text()),
            Span::styled(
                format!(" · {} · servers replicas may run on", d.name),
                theme::muted(),
            ),
        ],
    );
    if inner.height < 6 {
        return;
    }
    let cols = |row: Rect| {
        Layout::horizontal([
            Constraint::Length(4),
            Constraint::Length(26),
            Constraint::Length(19),
            Constraint::Length(18),
            Constraint::Fill(1),
        ])
        .areas::<5>(row)
    };
    let heading = Rect { height: 1, ..inner };
    let [_, name, address, free, replicas] = cols(heading);
    for (area, text) in [
        (name, "SERVER"),
        (address, "ADDRESS"),
        (free, "FREE MEMORY"),
        (replicas, "REPLICAS"),
    ] {
        frame.render_widget(Paragraph::new(text).style(theme::muted()), area);
    }
    let capacity = d.capacity.as_ref().and_then(|c| c.as_ref().ok());
    for (i, r) in d.rows.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let line = Rect::new(inner.x, y, inner.width, 1);
        let selected = i == d.row;
        if selected {
            frame.render_widget(Paragraph::new("").style(Style::new().bg(theme::LINE)), line);
        }
        mouse::hit(line, Hit::PlacementRow(i));
        let [check, name, address, free, replicas] = cols(line);
        let mark = if r.checked { "[x]" } else { "[ ]" };
        frame.render_widget(
            Paragraph::new(mark).style(if r.down.is_some() && !r.checked {
                theme::muted()
            } else {
                theme::text()
            }),
            check,
        );
        let name_style = match (selected, r.down.is_some()) {
            (true, _) => theme::accent(),
            (false, true) => theme::muted(),
            (false, false) => theme::text(),
        };
        let mut name_spans = vec![Span::styled(r.name.clone(), name_style)];
        if r.local {
            name_spans.push(Span::styled("  local", theme::muted()));
        }
        frame.render_widget(Paragraph::new(Line::from(name_spans)), name);
        frame.render_widget(
            Paragraph::new(truncate(&r.address, 18)).style(theme::muted()),
            address,
        );
        if let Some((what, why)) = &r.down {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(what.clone(), theme::fg(theme::ERR)),
                    Span::styled(format!("  {why}"), theme::muted()),
                ])),
                Rect::new(free.x, y, line.right() - free.x, 1),
            );
            continue;
        }
        let room = capacity.and_then(|c| c.servers.iter().find(|s| s.server_id == r.server_id));
        let (free_text, count) = match (room, &d.capacity) {
            (Some(s), _) if s.problem.is_some() => (
                Span::styled(s.problem.clone().unwrap_or_default(), theme::fg(theme::ERR)),
                String::new(),
            ),
            (Some(s), _) => (
                Span::styled(util::bytes(s.mem_free), theme::text()),
                s.replicas.to_string(),
            ),
            (None, None) => (
                Span::styled(util::spinner(tick), theme::muted()),
                String::new(),
            ),
            (None, Some(Ok(_))) => (Span::styled("—", theme::muted()), "0".into()),
            (None, Some(Err(_))) => (Span::styled("unknown", theme::muted()), String::new()),
        };
        frame.render_widget(Paragraph::new(free_text), free);
        frame.render_widget(Paragraph::new(count).style(theme::text()), replicas);
    }
    let mut y = inner.y + rows + 2;
    let label_w = 22u16;
    if y < inner.bottom() {
        let line = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(Paragraph::new("strategy").style(theme::muted()), line);
        let mut x = inner.x + label_w;
        for (j, s) in PlacementStrategy::ALL.into_iter().enumerate() {
            let chip = Span::styled(
                format!(" {} ", s.as_str()),
                if s == d.strategy {
                    theme::chip()
                } else {
                    theme::muted()
                },
            );
            let w = chip.width() as u16;
            let rect = Rect::new(x, y, w, 1).intersection(line);
            frame.render_widget(Paragraph::new(chip), rect);
            mouse::hit(rect, Hit::PlacementStrategy(j));
            x += w + 1;
        }
        let note = match d.strategy {
            PlacementStrategy::Spread => "one replica on each server before doubling up",
            PlacementStrategy::Pack => "fill servers in order before using the next",
        };
        frame.render_widget(
            Paragraph::new(format!("  {note}")).style(theme::muted()),
            Rect::new(x, y, line.right().saturating_sub(x), 1),
        );
        y += 2;
    }
    if y < inner.bottom() {
        let line = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(
            Paragraph::new("volumes per server").style(theme::muted()),
            line,
        );
        let (mark, text) = if d.volumes_per_server {
            ("[x]", "on")
        } else {
            ("[ ]", "off")
        };
        let mut spans = vec![
            Span::styled(mark, theme::text()),
            Span::styled(format!(" {text}"), theme::text()),
        ];
        if d.volumes.is_empty() {
            spans.push(Span::styled("   no volumes", theme::muted()));
        }
        let rect = Rect::new(inner.x + label_w, y, line.width.saturating_sub(label_w), 1);
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
        mouse::hit(
            Rect { width: 7, ..rect }.intersection(line),
            Hit::Key(KeyCode::Char('v'), KeyModifiers::NONE),
        );
        y += 1;
    }
    if warning && y < inner.bottom() {
        let mounts: Vec<String> = d
            .volumes
            .iter()
            .map(|(name, mount)| format!("{name} → {mount}"))
            .collect();
        let text = if d.volumes_per_server {
            format!(
                "{} {} copied per server: each starts empty, replicas don't share files.",
                mounts.join(", "),
                if mounts.len() == 1 { "is" } else { "are" }
            )
        } else {
            format!(
                "{} can't span servers: v keeps a copy per server.",
                mounts.join(", ")
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("! ", theme::fg(theme::ERR)),
                Span::styled(text, theme::text()),
            ])),
            Rect::new(inner.x, y, inner.width, 1),
        );
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
                format!(" {} applying… ", util::spinner(tick)),
                theme::chip(),
            ))
            .right_aligned(),
            keys,
        );
        return;
    }
    let n = d.chosen().len();
    let chip = if d.ready() {
        theme::chip()
    } else {
        Style::new().fg(theme::MUTED).bg(theme::LINE)
    };
    let rest = action_chip(
        frame,
        keys,
        "⏎",
        &format!("use {n} server{}", plural(n)),
        chip,
    );
    mouse::hints(
        frame,
        rest,
        &[
            ("space", "toggle"),
            ("↑↓", "move"),
            ("s", "strategy"),
            ("v", "volumes"),
            ("esc", "cancel"),
        ],
        Alignment::Left,
    );
}

fn verb_style(verb: LbVerb) -> Style {
    match verb {
        LbVerb::Create => theme::fg(theme::OK),
        LbVerb::Adopt => theme::fg(theme::INFO),
        LbVerb::Update => theme::text(),
        LbVerb::Delete => theme::fg(theme::ERR),
    }
}

/// "SVC-36": what enabling does and roughly what it costs.
pub(crate) fn render_plan(frame: &mut Frame, area: Rect, app: &App, d: &PlanDialog, tick: usize) {
    let p = &d.plan;
    let server = |id: &str| {
        app.servers
            .iter()
            .find(|s| s.id == id)
            .map_or(id.to_string(), |s| s.name.clone())
    };
    let network = d.config.network_id.map(|id| {
        app.providers
            .discovered(&d.config.provider_id)
            .and_then(|x| x.networks.iter().find(|n| n.id == id))
            .map_or(format!("network {id}"), |n| n.name.clone())
    });
    let mut body: Vec<Line<'static>> = vec![Line::styled(
        format!("{} will", brand::NAME),
        theme::muted(),
    )];
    for a in &p.actions {
        body.push(Line::from(vec![
            Span::styled(format!("{:<9}", a.verb.as_str()), verb_style(a.verb)),
            Span::styled(format!("{:<25}", a.resource.label()), theme::text()),
            Span::styled(format!("{:<20}", truncate(&a.name, 19)), theme::text()),
            Span::styled(a.detail.clone(), theme::muted()),
        ]));
    }
    if p.actions.is_empty() {
        body.push(Line::styled(
            "change nothing at the provider",
            theme::muted(),
        ));
    }
    body.push(Line::raw(""));
    body.push(Line::styled("targets", theme::muted()));
    for t in &p.targets {
        let mut detail = t.address.clone();
        if let Some(n) = &network {
            detail.push_str(&format!(" · {n}"));
        }
        body.push(Line::from(vec![
            Span::styled(
                format!("{:<26}", truncate(&server(&t.server_id), 25)),
                theme::text(),
            ),
            Span::styled(detail, theme::muted()),
        ]));
    }
    if p.targets.is_empty() {
        body.push(Line::styled(
            "none yet · servers join once a replica on them is healthy",
            theme::muted(),
        ));
    }
    body.push(Line::raw(""));
    let mut cost = vec![Span::styled(format!("{:<9}", "cost"), theme::muted())];
    match p.monthly_cost {
        Some(c) => {
            cost.push(Span::styled(
                format!("≈ {}/month", money(c, &p.currency)),
                theme::text(),
            ));
            cost.push(Span::styled(format!("  {}", p.cost_note), theme::muted()));
        }
        None => cost.push(Span::styled(
            if p.cost_note.is_empty() {
                "nothing billed".to_string()
            } else {
                p.cost_note.clone()
            },
            theme::muted(),
        )),
    }
    body.push(Line::from(cost));
    for w in &p.warnings {
        body.push(Line::from(vec![
            Span::raw(" ".repeat(9)),
            Span::styled("! ", theme::fg(theme::ERR)),
            Span::styled(w.clone(), theme::text()),
        ]));
    }
    if !p.dns.is_empty() {
        body.push(Line::raw(""));
        body.push(Line::styled("add these records by hand", theme::muted()));
        for r in &p.dns {
            body.push(Line::from(vec![
                Span::styled(format!("{:<9}", r.kind), theme::text()),
                Span::styled(format!("{:<25}", r.name), theme::text()),
                Span::styled(r.value.clone(), theme::muted()),
            ]));
        }
    }
    if let Some(e) = &d.error {
        body.push(Line::raw(""));
        body.push(Line::from(vec![
            Span::styled("✕ ", theme::fg(theme::ERR)),
            Span::styled(e.clone(), theme::text()),
        ]));
    }
    let inner = dialog(
        frame,
        area,
        110,
        body.len() as u16 + 7,
        vec![
            Span::styled("Load balancer", theme::text()),
            Span::styled(format!(" · {} · plan", d.name), theme::muted()),
        ],
    );
    if inner.height < 3 {
        return;
    }
    let keys = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
    let rule = Rect {
        y: keys.y - 1,
        ..keys
    };
    frame.render_widget(
        Paragraph::new(body),
        Rect {
            height: rule.y.saturating_sub(inner.y + 1),
            ..inner
        },
    );
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    if d.busy {
        frame.render_widget(
            Paragraph::new("waiting for the daemon").style(theme::muted()),
            keys,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} enabling… ", util::spinner(tick)),
                theme::chip(),
            ))
            .right_aligned(),
            keys,
        );
        return;
    }
    let rest = action_chip(frame, keys, "y", "enable", theme::chip());
    mouse::hints(
        frame,
        rest,
        &[("b", "back to edit"), ("esc", "cancel")],
        Alignment::Left,
    );
}

fn resource_detail(r: &LbResource, config: &LbConfig) -> String {
    match r.kind {
        LbResourceKind::HetznerLoadBalancer => {
            let mut s = config.lb_type.clone().unwrap_or_else(|| "lb11".into());
            if let Some(l) = &config.location {
                s.push_str(&format!(" in {l}"));
            }
            format!("{s} · billed until deleted")
        }
        _ => format!("id {}", r.id),
    }
}

/// "SVC-38": turning the load balancer off.
pub(crate) fn render_disable(frame: &mut Frame, area: Rect, d: &DisableDialog, tick: usize) {
    let created: Vec<&LbResource> = d.lb.resources.iter().filter(|r| r.created).collect();
    let n = created.len();
    let mut body: Vec<Line<'static>> = Vec::new();
    if n > 0 {
        body.push(Line::styled(
            format!(
                "Delete the {n} resource{} {} created?",
                plural(n),
                brand::NAME
            ),
            theme::text(),
        ));
        body.push(Line::raw(""));
        for r in &created {
            body.push(Line::from(vec![
                Span::styled(format!("{:<23}", r.kind.label()), theme::text()),
                Span::styled(format!("{:<20}", truncate(&r.name, 19)), theme::text()),
                Span::styled(resource_detail(r, &d.lb.config), theme::muted()),
            ]));
        }
    } else {
        body.push(Line::styled(
            format!(
                "Turn off the load balancer of {}? {} created nothing, so nothing is deleted.",
                d.name,
                brand::NAME
            ),
            theme::text(),
        ));
    }
    body.push(Line::raw(""));
    body.push(Line::styled(
        format!(
            "Resources {} adopted instead of creating are always kept.",
            brand::NAME
        ),
        theme::muted(),
    ));
    let inner = dialog(
        frame,
        area,
        92,
        body.len() as u16 + 7,
        vec![
            Span::styled("Turn off load balancer", theme::text()),
            Span::styled(format!(" · {}", d.name), theme::muted()),
        ],
    );
    if inner.height < 3 {
        return;
    }
    let keys = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
    let rule = Rect {
        y: keys.y - 1,
        ..keys
    };
    frame.render_widget(
        Paragraph::new(body),
        Rect {
            height: rule.y.saturating_sub(inner.y + 1),
            ..inner
        },
    );
    frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
    let danger = Style::new().fg(theme::BG).bg(theme::ERR);
    if d.busy {
        frame.render_widget(
            Paragraph::new("waiting for the daemon").style(theme::muted()),
            keys,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} turning off… ", util::spinner(tick)),
                danger,
            ))
            .right_aligned(),
            keys,
        );
        return;
    }
    if n > 0 {
        let rest = action_chip(
            frame,
            keys,
            "y",
            &format!("delete {n} resource{}", plural(n)),
            danger,
        );
        mouse::hints(
            frame,
            rest,
            &[("esc", "cancel"), ("k", "keep them")],
            Alignment::Left,
        );
    } else {
        let rest = action_chip(frame, keys, "y", "turn off", theme::chip());
        mouse::hints(frame, rest, &[("esc", "cancel")], Alignment::Left);
    }
}

/// "SVC-37" and "SVC-39": the load balancer, its targets and what it created, in place of the
/// Settings tab's panels.
pub(crate) fn render_status(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    d: &ServiceDetail,
    row: usize,
) {
    let Some(lb) = &d.lb else {
        frame.render_widget(
            Paragraph::new("no load balancer · l to turn one on").style(theme::muted()),
            area,
        );
        return;
    };
    let c = &lb.config;
    let provider = app.providers.get(&c.provider_id);
    let server = |id: &str| {
        app.servers
            .iter()
            .find(|s| s.id == id)
            .map_or(id.to_string(), |s| s.name.clone())
    };
    const KEY_W: usize = 14;
    let kv = |k: &str, v: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("{k:<KEY_W$}"), theme::muted())];
        spans.extend(v);
        Line::from(spans)
    };
    let style = state_style(lb.state);
    let mut state = vec![
        Span::styled(format!("{} ", state_glyph(lb.state)), style),
        Span::styled(lb.state.as_str(), style),
    ];
    let synced = lb
        .last_sync_at
        .map(|at| format!("last sync {}", util::ago(at)));
    let after = match lb.state {
        LbSyncState::Drift => synced.map(|s| format!("{s}, waiting for you")),
        LbSyncState::Error => Some(synced.unwrap_or_default()),
        LbSyncState::Pending => Some("first sync running".into()),
        LbSyncState::InSync => synced,
    };
    if let Some(a) = after.filter(|a| !a.is_empty()) {
        state.push(Span::styled(format!("   {a}"), theme::muted()));
    }
    let domains = if c.domains.is_empty() {
        d.domains
            .iter()
            .map(|x| x.host.clone())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        c.domains.join(", ")
    };
    let records = match &c.dns_provider_id {
        Some(id) => format!("{domains} → A record via {}", app.providers.name(id)),
        None if !lb.dns.is_empty() => format!("{domains} → records to add by hand, below"),
        None => domains.clone(),
    };
    let address = match c.mode {
        LbMode::Hetzner => vec![
            Span::styled(
                if lb.addresses.is_empty() {
                    "not assigned yet".to_string()
                } else {
                    lb.addresses.join(", ")
                },
                if lb.addresses.is_empty() {
                    theme::muted()
                } else {
                    theme::text()
                },
            ),
            Span::styled(format!("   {records}"), theme::muted()),
        ],
        _ => vec![
            Span::styled(domains.clone(), theme::text()),
            Span::styled(
                if c.proxied() {
                    "   proxied through Cloudflare"
                } else {
                    "   DNS only"
                },
                theme::muted(),
            ),
        ],
    };
    let kind = match c.mode {
        LbMode::Hetzner => {
            let mut t = c.lb_type.clone().unwrap_or_else(|| "lb11".into());
            if let Some(l) = &c.location {
                t.push_str(&format!(" · {l}"));
            }
            let reach = match c.network_id {
                Some(id) => app
                    .providers
                    .discovered(&c.provider_id)
                    .and_then(|x| x.networks.iter().find(|n| n.id == id))
                    .map_or(format!("network {id}"), |n| n.name.clone()),
                None => "public addresses".into(),
            };
            vec![
                Span::styled(t, theme::text()),
                Span::styled(
                    format!("   {reach}, TCP 80 and 443, health check on the proxy"),
                    theme::muted(),
                ),
            ]
        }
        LbMode::CloudflareDns => vec![
            Span::styled("Cloudflare DNS", theme::text()),
            Span::styled(
                "   records list every server with a healthy replica",
                theme::muted(),
            ),
        ],
        LbMode::CloudflareLb => vec![
            Span::styled("Cloudflare Load Balancing", theme::text()),
            Span::styled("   a monitor, a pool and a load balancer", theme::muted()),
        ],
    };
    let mut lines = vec![kv("state", state), kv("address", address), kv("type", kind)];
    let mut force_at = None;
    for (i, drift) in lb.drift.iter().enumerate() {
        let mut spans = vec![
            Span::styled("! ", theme::fg(theme::ERR)),
            Span::styled(drift.clone(), theme::text()),
        ];
        if i == 0 {
            spans.push(Span::styled(" · ", theme::muted()));
            force_at = Some((
                lines.len(),
                KEY_W + spans.iter().map(Span::width).sum::<usize>(),
            ));
            spans.extend([
                Span::styled("f", theme::accent()),
                Span::styled(" force sync", theme::muted()),
            ]);
        }
        lines.push(kv("", spans));
    }
    if let Some(e) = &lb.last_error {
        lines.push(kv("", vec![Span::styled(e.clone(), theme::fg(theme::ERR))]));
    }
    for r in &lb.dns {
        lines.push(kv(
            "add record",
            vec![
                Span::styled(format!("{} {}", r.kind, r.name), theme::text()),
                Span::styled(format!(" → {}", r.value), theme::muted()),
            ],
        ));
    }
    let targets_h = lb.targets.len().max(1) as u16 + 3;
    let resources_h = lb.resources.len().max(1) as u16 + 2;
    let [top, targets, resources, _] = Layout::vertical([
        Constraint::Length(lines.len() as u16 + 2),
        Constraint::Length(targets_h),
        Constraint::Length(resources_h),
        Constraint::Fill(1),
    ])
    .spacing(1)
    .areas(area);
    let right =
        provider.map(|p| Line::styled(format!("{} · {}", p.name, p.kind.label()), theme::muted()));
    let block = panel(title("Load balancer", true), right, true);
    let inner = inside(&block, top);
    frame.render_widget(block, top);
    if let Some((i, x)) = force_at
        && (i as u16) < inner.height
    {
        mouse::span_target(
            Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
            x,
            "f force sync".len(),
            Hit::Key(KeyCode::Char('f'), KeyModifiers::NONE),
        );
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let health_title = format!(
        "{} HEALTH",
        match c.mode.provider() {
            ProviderKind::Hetzner => "HETZNER",
            ProviderKind::Cloudflare => "CLOUDFLARE",
        }
    );
    let block = panel(
        title("Targets", true),
        Some(Line::styled(
            format!("{} of {} healthy", healthy(lb), lb.targets.len()),
            theme::muted(),
        )),
        true,
    );
    let inner = inside(&block, targets);
    frame.render_widget(block, targets);
    mouse::hit(inner, Hit::List(crate::mouse::List::Targets));
    let cols = |r: Rect| {
        Layout::horizontal([
            Constraint::Length(2),
            Constraint::Length(24),
            Constraint::Length(19),
            Constraint::Length(19),
            Constraint::Fill(1),
        ])
        .areas::<5>(r)
    };
    let [_, a, b, cc, e] = cols(Rect { height: 1, ..inner });
    for (r, t) in [
        (a, "SERVER"),
        (b, "ADDRESS"),
        (cc, "REPLICAS HEALTHY"),
        (e, health_title.as_str()),
    ] {
        frame.render_widget(Paragraph::new(t).style(theme::muted()), r);
    }
    if lb.targets.is_empty() && inner.height > 1 {
        frame.render_widget(
            Paragraph::new("none yet · servers join once a replica on them is healthy")
                .style(theme::muted()),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    for (i, t) in lb.targets.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let line = Rect::new(inner.x, y, inner.width, 1);
        let selected = i == row;
        if selected {
            frame.render_widget(Paragraph::new("").style(theme::selected()), line);
        }
        mouse::hit(line, Hit::Row(i));
        let [g, a, b, cc, e] = cols(line);
        let health = t
            .provider_health
            .clone()
            .unwrap_or_else(|| "unknown".into());
        let (glyph, hs) = match health.as_str() {
            "healthy" => ("●", theme::fg(theme::OK)),
            "unhealthy" => ("●", theme::fg(theme::ERR)),
            _ => ("○", theme::muted()),
        };
        let total = d
            .replicas
            .iter()
            .filter(|r| r.server_id == t.server_id)
            .count()
            .max(usize::from(t.replicas));
        frame.render_widget(Paragraph::new(Span::styled(glyph, hs)), g);
        frame.render_widget(
            Paragraph::new(server(&t.server_id)).style(if selected {
                theme::accent()
            } else {
                theme::text()
            }),
            a,
        );
        frame.render_widget(Paragraph::new(t.address.clone()).style(theme::muted()), b);
        frame.render_widget(
            Paragraph::new(format!("{} of {total}", t.replicas)).style(theme::text()),
            cc,
        );
        frame.render_widget(
            Paragraph::new(health.clone()).style(match health.as_str() {
                "healthy" => theme::text(),
                "unhealthy" => theme::fg(theme::ERR),
                _ => theme::muted(),
            }),
            e,
        );
    }

    let created = lb.resources.iter().any(|r| r.created);
    let block = panel(
        title(&format!("Created by {}", brand::NAME.to_lowercase()), true),
        created.then(|| Line::styled("deleted when you disable it", theme::muted())),
        true,
    );
    let inner = inside(&block, resources);
    frame.render_widget(block, resources);
    if lb.resources.is_empty() {
        frame.render_widget(
            Paragraph::new("nothing yet · the first sync creates it").style(theme::muted()),
            inner,
        );
    }
    let lines: Vec<Line> = lb
        .resources
        .iter()
        .map(|r| {
            let mut detail = format!("id {}", r.id);
            if !r.created {
                detail.push_str(" · adopted, always kept");
            }
            Line::from(vec![
                Span::styled("● ", theme::fg(theme::OK)),
                Span::styled(format!("{:<24}", r.kind.label()), theme::text()),
                Span::styled(format!("{:<20}", truncate(&r.name, 19)), theme::text()),
                Span::styled(detail, theme::muted()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_like_the_provider_lists_them() {
        assert_eq!(compact(10_000), "10k");
        assert_eq!(compact(25), "25");
        assert_eq!(compact(1_500), "1.5k");
        assert_eq!(money(5.39, "EUR"), "€5.39");
        assert_eq!(money(5.0, "CHF"), "5.00 CHF");
    }
}
