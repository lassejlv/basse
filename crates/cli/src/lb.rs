//! `provider` connections, `service placement` and `service lb`: replicas across servers and the
//! managed load balancers in front of them.

use crate::output::{self, Cell, Tone, muted, paint};
use crate::sources::read_secret_file;
use crate::{confirm, find, find_server, hint};
use anyhow::{Result, bail};
use clap::Subcommand;
use client::Client;
use common::lb::*;
use common::*;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ProviderCmd {
    List,
    /// Connect Hetzner Cloud or Cloudflare with an API token, read from --token-file or stdin
    /// and stored as a secret. Hetzner: a read & write token of the project. Cloudflare: Zone
    /// DNS edit, plus Load Balancing edit for the paid load balancers.
    Add {
        /// hetzner or cloudflare.
        kind: ProviderKind,
        /// Defaults to the kind.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Use a generic secret that already holds the token.
        #[arg(long, conflicts_with = "token_file")]
        secret: Option<String>,
    },
    /// Ask the provider about the token again.
    Check {
        name: String,
    },
    /// What the token sees: Hetzner servers matched to yours, locations, prices; Cloudflare zones.
    Show {
        name: String,
    },
    Rm {
        name: String,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum LbCmd {
    /// State, targets and what was created.
    Status { service: String },
    /// Turn a managed load balancer on (or change it). Shows what will be created and roughly
    /// what it costs, then asks.
    Enable {
        service: String,
        /// Provider connection to use.
        #[arg(long)]
        provider: String,
        /// hetzner, cloudflare-dns or cloudflare-lb; defaults to hetzner for a Hetzner
        /// connection and cloudflare-dns for Cloudflare.
        #[arg(long)]
        mode: Option<LbMode>,
        /// Only these domains; default all of the service's.
        #[arg(long = "domain")]
        domains: Vec<String>,
        /// Hetzner load balancer type, default lb11.
        #[arg(long = "type")]
        lb_type: Option<String>,
        /// Hetzner location, e.g. fsn1; default where the first server is.
        #[arg(long)]
        location: Option<String>,
        /// Hetzner private network id to reach the servers on.
        #[arg(long)]
        network: Option<i64>,
        /// Manage this existing Hetzner load balancer (by id) instead of creating one.
        #[arg(long)]
        adopt: Option<i64>,
        /// Cloudflare connection that gets the domains' records (Hetzner mode).
        #[arg(long)]
        dns: Option<String>,
        /// Proxy the records through Cloudflare: true or false.
        #[arg(long)]
        proxied: Option<bool>,
        /// Take over DNS records for the domains that already exist.
        #[arg(long)]
        adopt_records: bool,
        #[arg(long, short)]
        yes: bool,
    },
    /// Sync the targets now.
    Sync {
        service: String,
        /// Overwrite changes made by hand at the provider.
        #[arg(long)]
        force: bool,
    },
    /// Turn it off. Asks whether to delete what was created; adopted resources always stay.
    Disable {
        service: String,
        /// Delete the resources the daemon created.
        #[arg(long, conflicts_with = "keep")]
        delete: bool,
        /// Keep them.
        #[arg(long)]
        keep: bool,
        #[arg(long, short)]
        yes: bool,
    },
}

async fn find_provider(c: &Client, name: &str) -> Result<Provider> {
    let list = c.providers().await?;
    match list.into_iter().find(|p| p.name == name || p.id == name) {
        Some(p) => Ok(p),
        None => bail!("no provider named '{name}'; see {}", hint("provider list")),
    }
}

fn status(p: &Provider) -> Cell {
    match p.status {
        ProviderStatus::Connected => Cell("connected".into(), Tone::Ok),
        ProviderStatus::Error => Cell(
            format!("error: {}", p.problem.as_deref().unwrap_or_default()),
            Tone::Err,
        ),
        ProviderStatus::Unknown => Cell("unknown".into(), Tone::Muted),
    }
}

fn reaches(p: &Provider) -> String {
    let mut s = p.account.clone().unwrap_or_default();
    if p.kind == ProviderKind::Cloudflare && p.load_balancing {
        s.push_str(" · load balancing");
    }
    s
}

pub async fn provider(c: &Client, cmd: ProviderCmd, json: bool) -> Result<()> {
    match cmd {
        ProviderCmd::List => {
            let list = c.providers().await?;
            if json {
                return output::json(&list);
            }
            if list.is_empty() {
                output::note(format!(
                    "no providers yet; connect one with {}",
                    hint("provider add hetzner --token-file token.txt")
                ));
                return Ok(());
            }
            output::table(
                &["PROVIDER", "KIND", "STATUS", "REACHES", "CHECKED"],
                list.iter()
                    .map(|p| {
                        vec![
                            p.name.clone().into(),
                            Cell(p.kind.label().into(), Tone::Muted),
                            status(p),
                            Cell(reaches(p), Tone::Muted),
                            Cell(
                                p.last_checked_at.map(output::ago).unwrap_or_default(),
                                Tone::Muted,
                            ),
                        ]
                    })
                    .collect(),
            );
        }
        ProviderCmd::Add {
            kind,
            name,
            token_file,
            secret,
        } => {
            let token_secret_id = match &secret {
                Some(name) => {
                    let secrets = c.secrets().await?;
                    let found = secrets.iter().find(|s| s.name == *name || s.id == *name);
                    match found {
                        Some(s) => Some(s.id.clone()),
                        None => bail!("no secret named '{name}'"),
                    }
                }
                None => None,
            };
            let token = match token_secret_id {
                Some(_) => None,
                None => Some(read_secret_file(token_file.as_ref(), "API token")?),
            };
            let p = c
                .create_provider(&CreateProvider {
                    name: name.unwrap_or_else(|| kind.as_str().to_string()),
                    kind,
                    token,
                    token_secret_id,
                })
                .await?;
            if json {
                return output::json(&p);
            }
            output::ok(format!("{} connected: {}", p.name, reaches(&p)));
        }
        ProviderCmd::Check { name } => {
            let p = c.check_provider(&find_provider(c, &name).await?.id).await?;
            if json {
                return output::json(&p);
            }
            match p.status {
                ProviderStatus::Connected => output::ok(format!("{}: {}", p.name, reaches(&p))),
                _ => bail!("{}: {}", p.name, p.problem.unwrap_or_default()),
            }
        }
        ProviderCmd::Show { name } => {
            let p = find_provider(c, &name).await?;
            let d = c.discover_provider(&p.id).await?;
            if json {
                return output::json(&d);
            }
            show_discovery(c, &p, &d).await?;
        }
        ProviderCmd::Rm { name, yes } => {
            let p = find_provider(c, &name).await?;
            confirm(&format!("Remove provider {}?", p.name), yes)?;
            c.delete_provider(&p.id).await?;
            output::ok(format!("removed {}; its token stays in Secrets", p.name));
        }
    }
    Ok(())
}

async fn show_discovery(c: &Client, p: &Provider, d: &ProviderDiscovery) -> Result<()> {
    match p.kind {
        ProviderKind::Hetzner => {
            let servers = c.servers().await?;
            let name = |id: &str| {
                servers
                    .iter()
                    .find(|s| s.id == id)
                    .map_or(id.to_string(), |s| s.name.clone())
            };
            output::table(
                &["SERVER", "ADDRESS", "HETZNER", "LOCATION", "PRIVATE"],
                d.servers
                    .iter()
                    .map(|m| {
                        let (h, loc, private) = match &m.hetzner {
                            Some(h) => (
                                Cell(h.name.clone(), Tone::Ok),
                                format!("{} · {}", h.location, h.network_zone),
                                h.private_networks
                                    .iter()
                                    .map(|n| format!("{} ({})", n.ip, n.network_id))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            ),
                            None => (
                                Cell("not in this project".into(), Tone::Muted),
                                String::new(),
                                String::new(),
                            ),
                        };
                        vec![
                            name(&m.server_id).into(),
                            Cell(m.address.clone().unwrap_or_default(), Tone::Muted),
                            h,
                            Cell(loc, Tone::Muted),
                            Cell(private, Tone::Muted),
                        ]
                    })
                    .collect(),
            );
            if !d.load_balancers.is_empty() {
                println!();
                output::table(
                    &[
                        "LOAD BALANCER",
                        "ID",
                        "TYPE",
                        "LOCATION",
                        "ADDRESS",
                        "OWNER",
                    ],
                    d.load_balancers
                        .iter()
                        .map(|l| {
                            vec![
                                l.name.clone().into(),
                                Cell(l.id.to_string(), Tone::Muted),
                                Cell(l.lb_type.clone(), Tone::Muted),
                                Cell(l.location.clone(), Tone::Muted),
                                Cell(l.public_ipv4.clone().unwrap_or_default(), Tone::Muted),
                                if l.managed {
                                    Cell(brand::NAME.into(), Tone::Info)
                                } else {
                                    Cell("—".into(), Tone::Muted)
                                },
                            ]
                        })
                        .collect(),
                );
            }
            let prices: Vec<String> = d
                .lb_types
                .iter()
                .filter_map(|t| {
                    let p = t.prices.first()?;
                    Some(format!("{} €{:.2}", t.name, p.monthly))
                })
                .collect();
            if !prices.is_empty() {
                output::note(format!("load balancer types: {}", prices.join(" · ")));
            }
        }
        ProviderKind::Cloudflare => {
            output::table(
                &["ZONE", "STATUS", "ACCOUNT"],
                d.zones
                    .iter()
                    .map(|z| {
                        vec![
                            z.name.clone().into(),
                            Cell(z.status.clone(), Tone::Muted),
                            Cell(z.account_name.clone(), Tone::Muted),
                        ]
                    })
                    .collect(),
            );
            if p.load_balancing {
                output::note("Load Balancing is on this account: cloudflare-lb is available");
            }
        }
    }
    Ok(())
}

/// `service placement`: shows or sets the servers a service's replicas run on.
pub async fn placement(
    c: &Client,
    service: &str,
    servers: Vec<String>,
    strategy: Option<PlacementStrategy>,
    volumes_per_server: Option<bool>,
    json: bool,
) -> Result<()> {
    let (_, _, s) = find(c, service).await?;
    let all = c.servers().await?;
    let name = |id: &str| {
        all.iter()
            .find(|x| x.id == id)
            .map_or(id.to_string(), |x| x.name.clone())
    };
    let changed = !servers.is_empty() || strategy.is_some() || volumes_per_server.is_some();
    let detail = if changed {
        let mut p = s.scaling.placement.clone();
        if !servers.is_empty() {
            let mut ids = Vec::new();
            for name in &servers {
                ids.push(find_server(c, name).await?.id);
            }
            p.servers = ids;
        }
        if let Some(st) = strategy {
            p.strategy = st;
        }
        if let Some(v) = volumes_per_server {
            p.volumes_per_server = v;
        }
        Some(c.set_placement(&s.id, &p).await?)
    } else {
        None
    };
    let svc = detail.as_ref().map_or(&s, |d| &d.service);
    if json {
        return output::json(&svc.scaling.placement);
    }
    let p = &svc.scaling.placement;
    let names: Vec<String> = p
        .servers_or(&svc.server_id)
        .iter()
        .map(|id| name(id))
        .collect();
    let line = format!(
        "{}: {} · {}{}",
        svc.name,
        names.join(", "),
        p.strategy,
        if p.volumes_per_server {
            " · volumes per server"
        } else {
            ""
        }
    );
    if changed {
        output::ok(line);
        output::note(format!(
            "replicas move in the background; follow with {}",
            hint(&format!("service ps {}", svc.name))
        ));
    } else {
        println!("{line}");
        let cap = c.scale_capacity(&s.id).await?;
        for sc in &cap.servers {
            let room = match &sc.problem {
                Some(p) => paint(p, Tone::Err),
                None => muted(format!(
                    "{} replica{} · {} free · fits {} more",
                    sc.replicas,
                    if sc.replicas == 1 { "" } else { "s" },
                    output::bytes(sc.mem_free),
                    sc.fits_more
                )),
            };
            println!("  {:<16} {room}", name(&sc.server_id));
        }
    }
    Ok(())
}

pub async fn lb(c: &Client, cmd: LbCmd, json: bool) -> Result<()> {
    match cmd {
        LbCmd::Status { service } => {
            let (_, _, s) = find(c, &service).await?;
            let Some(lb) = c.service_lb(&s.id).await? else {
                if json {
                    return output::json(&Option::<ServiceLb>::None);
                }
                output::note(format!(
                    "{} has no load balancer; turn one on with {}",
                    s.name,
                    hint(&format!("service lb enable {} --provider <name>", s.name))
                ));
                return Ok(());
            };
            if json {
                return output::json(&lb);
            }
            print_lb(c, &s, &lb).await?;
        }
        LbCmd::Enable {
            service,
            provider,
            mode,
            domains,
            lb_type,
            location,
            network,
            adopt,
            dns,
            proxied,
            adopt_records,
            yes,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let p = find_provider(c, &provider).await?;
            let mode = mode.unwrap_or(match p.kind {
                ProviderKind::Hetzner => LbMode::Hetzner,
                ProviderKind::Cloudflare => LbMode::CloudflareDns,
            });
            let dns_provider_id = match &dns {
                Some(name) => Some(find_provider(c, name).await?.id),
                None => None,
            };
            let config = LbConfig {
                provider_id: p.id.clone(),
                mode,
                domains,
                lb_type,
                location,
                network_id: network,
                adopt_lb_id: adopt,
                dns_provider_id,
                proxied,
                adopt_records,
            };
            let plan = c.plan_lb(&s.id, &config).await?;
            if !json {
                print_plan(&plan);
            }
            confirm("Turn it on?", yes)?;
            let lb = c
                .enable_lb(
                    &s.id,
                    &EnableLb {
                        config,
                        confirm: true,
                    },
                )
                .await?;
            if json {
                return output::json(&lb);
            }
            output::ok(format!(
                "{} load balancer on for {}; it syncs in the background",
                lb.config.mode.label(),
                s.name
            ));
            output::note(format!(
                "follow with {}",
                hint(&format!("service lb status {}", s.name))
            ));
        }
        LbCmd::Sync { service, force } => {
            let (_, _, s) = find(c, &service).await?;
            let lb = c.sync_lb(&s.id, force).await?;
            if json {
                return output::json(&lb);
            }
            print_lb(c, &s, &lb).await?;
        }
        LbCmd::Disable {
            service,
            delete,
            keep,
            yes,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let Some(lb) = c.service_lb(&s.id).await? else {
                bail!("{} has no load balancer", s.name);
            };
            let created: Vec<&LbResource> = lb.resources.iter().filter(|r| r.created).collect();
            let delete = if delete || keep || created.is_empty() {
                delete
            } else {
                println!("{} created:", brand::NAME);
                for r in &created {
                    println!("  {} {}", r.kind.label(), muted(&r.name));
                }
                confirm_bool(&format!("Delete these {}?", created.len()), yes)?
            };
            confirm(&format!("Turn off the load balancer of {}?", s.name), yes)?;
            let t = c.disable_lb(&s.id, delete).await?;
            if json {
                return output::json(&t);
            }
            output::ok(format!(
                "load balancer off: {} deleted, {} kept",
                t.deleted.len(),
                t.kept.len()
            ));
            for r in &t.kept {
                output::note(format!(
                    "  kept {} {}{}",
                    r.kind.label(),
                    r.name,
                    if r.created { "" } else { " (adopted)" }
                ));
            }
        }
    }
    Ok(())
}

/// A yes/no question that defaults to no; `--yes` alone keeps resources.
fn confirm_bool(prompt: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(false);
    }
    Ok(confirm(prompt, false).is_ok())
}

fn print_plan(plan: &LbPlan) {
    for a in &plan.actions {
        println!(
            "  {:<7} {:<26} {} {}",
            paint(a.verb.as_str(), Tone::Accent),
            a.resource.label(),
            a.name,
            muted(&a.detail)
        );
    }
    if !plan.targets.is_empty() {
        let t: Vec<&str> = plan.targets.iter().map(|t| t.address.as_str()).collect();
        println!("  {:<7} {}", muted("targets"), t.join(", "));
    }
    match plan.monthly_cost {
        Some(cost) => println!(
            "  {:<7} ≈ {cost:.2} {}/month · {}",
            muted("cost"),
            plan.currency,
            muted(&plan.cost_note)
        ),
        None if !plan.cost_note.is_empty() => {
            println!("  {:<7} {}", muted("cost"), muted(&plan.cost_note));
        }
        None => {}
    }
    for r in &plan.dns {
        println!("  {:<7} {} {} → {}", muted("dns"), r.kind, r.name, r.value);
    }
    for w in &plan.warnings {
        output::warn(w);
    }
}

async fn print_lb(c: &Client, s: &Service, lb: &ServiceLb) -> Result<()> {
    let servers = c.servers().await?;
    let name = |id: &str| {
        servers
            .iter()
            .find(|x| x.id == id)
            .map_or(id.to_string(), |x| x.name.clone())
    };
    let tone = match lb.state {
        LbSyncState::InSync => Tone::Ok,
        LbSyncState::Pending => Tone::Muted,
        LbSyncState::Drift => Tone::Accent,
        LbSyncState::Error => Tone::Err,
    };
    println!(
        "{} · {} · {}",
        s.name,
        lb.config.mode.label(),
        paint(lb.state.as_str(), tone)
    );
    let mut facts = Vec::new();
    if let Some(at) = lb.last_sync_at {
        facts.push(format!("synced {}", output::ago(at)));
    }
    if !lb.addresses.is_empty() {
        facts.push(lb.addresses.join(", "));
    }
    if !facts.is_empty() {
        output::note(facts.join(" · "));
    }
    output::table(
        &["SERVER", "ADDRESS", "REPLICAS", "PROVIDER"],
        lb.targets
            .iter()
            .map(|t| {
                let health = t.provider_health.clone().unwrap_or_else(|| "—".into());
                let tone = match health.as_str() {
                    "healthy" => Tone::Ok,
                    "unhealthy" => Tone::Err,
                    _ => Tone::Muted,
                };
                vec![
                    name(&t.server_id).into(),
                    Cell(t.address.clone(), Tone::Muted),
                    Cell(format!("{} healthy", t.replicas), Tone::Muted),
                    Cell(health, tone),
                ]
            })
            .collect(),
    );
    for d in &lb.drift {
        output::warn(d);
    }
    if !lb.drift.is_empty() {
        output::note(format!(
            "left alone; overwrite with {}",
            hint(&format!("service lb sync {} --force", s.name))
        ));
    }
    if let Some(e) = &lb.last_error {
        println!("{}", paint(e, Tone::Err));
    }
    for r in &lb.dns {
        output::note(format!("add {} {} → {}", r.kind, r.name, r.value));
    }
    for r in &lb.resources {
        output::note(format!(
            "{} {} {}",
            if r.created { "created" } else { "adopted" },
            r.kind.label(),
            r.name
        ));
    }
    Ok(())
}
