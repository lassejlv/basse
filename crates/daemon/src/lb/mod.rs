//! Managed load balancers: the plan shown before one is turned on, turning it on and off, and
//! the reconciler that keeps the provider's targets on the servers running healthy replicas.
//!
//! Each sync compares three things: what the daemon applied last time, what the provider has
//! now, and what it should have. When the provider no longer has what was applied, someone
//! changed it by hand: the sync reports the drift once, sends `lb.drift_detected`, and leaves
//! the provider alone until the change is undone or a sync is forced. Otherwise it applies the
//! difference. Failures back off exponentially; a rate limit waits for the time the provider
//! names. Nothing is ever pointed at an empty set: with no healthy replica anywhere, the
//! previous targets stay.

pub mod cloudflare;
pub mod hetzner;

use crate::db::ServiceRecord;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::notify::Event;
use crate::replicas;
use crate::state::AppState;
use anyhow::{Context as _, Result, anyhow, bail};
use chrono::{TimeDelta, Utc};
use cloudflare::{Cloudflare, Origin, Record};
use common::lb::*;
use common::notify::NotifyEvent;
use common::*;
use hetzner::{Hetzner, NewLb, Target};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

/// A provider refused more requests until `until`.
#[derive(Debug, thiserror::Error)]
#[error("{provider} rate limit reached; trying again at {until}")]
pub struct RateLimited {
    pub provider: &'static str,
    pub until: Timestamp,
}

/// How often the reconciler looks at every load balancer without being asked.
const TICK: Duration = Duration::from_secs(30);
/// A load balancer in sync whose targets haven't changed is read back from the provider this
/// often, to notice changes made by hand.
const RECHECK_SECS: i64 = 300;
/// Failed syncs in a row before `lb.sync_failed` goes out.
const NOTIFY_AFTER: u32 = 3;
const FIRST_BACKOFF_SECS: i64 = 30;
const MAX_BACKOFF_SECS: i64 = 1800;
/// Hetzner's least load balancer type.
const DEFAULT_LB_TYPE: &str = "lb11";
/// Rough Cloudflare Load Balancing list price: the base covers two origins, each further
/// origin costs as much again.
const CLOUDFLARE_LB_BASE_USD: f64 = 5.0;
const CLOUDFLARE_LB_ORIGIN_USD: f64 = 5.0;

/// A service's load balancer as stored: its config and what the reconciler knows.
#[derive(Debug, Clone)]
pub struct LbRecord {
    pub config: LbConfig,
    pub stored: LbStored,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LbStored {
    #[serde(default)]
    pub resources: Vec<LbResource>,
    /// What the last sync left at the provider; `None` before the first.
    #[serde(default)]
    pub applied: Option<Applied>,
    #[serde(default)]
    pub state: LbSyncState,
    #[serde(default)]
    pub targets: Vec<LbTarget>,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub dns: Vec<DnsRecord>,
    #[serde(default)]
    pub drift: Vec<String>,
    #[serde(default)]
    pub last_sync_at: Option<Timestamp>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub next_attempt_at: Option<Timestamp>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub failure_notified: bool,
    #[serde(default)]
    pub drift_notified: bool,
}

/// What the daemon put at the provider, to tell its own changes from changes made by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// Hetzner target keys, or the addresses of a Cloudflare pool's origins.
    #[serde(default)]
    pub targets: BTreeSet<String>,
    /// DNS records the daemon manages, `zone/record` → `A 203.0.113.7`.
    #[serde(default)]
    pub records: BTreeMap<String, String>,
}

/// The reconciler's own state, part of `AppState`.
pub struct Reconciler {
    kick: tokio::sync::Notify,
    /// Syncs, enabling and disabling run one at a time.
    lock: tokio::sync::Mutex<()>,
    http: reqwest::Client,
    hetzner_url: String,
    cloudflare_url: String,
}

impl Default for Reconciler {
    fn default() -> Self {
        let url = |suffix: &str, default: &str| {
            std::env::var(brand::env_var(suffix))
                .ok()
                .filter(|u| !u.trim().is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        Self {
            kick: tokio::sync::Notify::new(),
            lock: tokio::sync::Mutex::new(()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            hetzner_url: url("HETZNER_API_URL", hetzner::DEFAULT_URL),
            cloudflare_url: url("CLOUDFLARE_API_URL", cloudflare::DEFAULT_URL),
        }
    }
}

impl Reconciler {
    /// Asks for a round now, e.g. after replicas moved.
    pub fn kick(&self) {
        self.kick.notify_one();
    }

    pub fn hetzner(&self, token: Zeroizing<String>) -> Hetzner {
        Hetzner::new(self.http.clone(), &self.hetzner_url, token)
    }

    pub fn cloudflare(&self, token: Zeroizing<String>) -> Cloudflare {
        Cloudflare::new(self.http.clone(), &self.cloudflare_url, token)
    }
}

/// Runs a round every [`TICK`], and whenever something kicks it.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = state.lb.kick.notified() => {}
                () = tokio::time::sleep(TICK) => {}
            }
            if let Err(e) = round(&state).await {
                tracing::warn!("load balancer round failed: {e:#}");
            }
        }
    });
}

async fn round(state: &Arc<AppState>) -> Result<()> {
    let now = Utc::now();
    for (service_id, lb) in state.db.service_lbs().await? {
        if lb.stored.next_attempt_at.is_some_and(|t| t > now) {
            continue;
        }
        let Some(rec) = state.db.service(&service_id).await? else {
            continue;
        };
        if lb.stored.state == LbSyncState::InSync
            && lb
                .stored
                .last_sync_at
                .is_some_and(|t| (now - t).num_seconds() < RECHECK_SECS)
        {
            let wanted = wanted(state, &rec).await;
            let same = wanted.len() == lb.stored.targets.len()
                && wanted.iter().all(|w| {
                    lb.stored
                        .targets
                        .iter()
                        .any(|t| t.server_id == w.server.id && t.replicas == w.replicas)
                });
            if same {
                continue;
            }
        }
        sync(state, &service_id, false).await?;
    }
    Ok(())
}

/// A server that should get traffic: it runs healthy replicas behind its proxy.
#[derive(Debug, Clone)]
struct Wanted {
    server: Server,
    ipv4: Option<String>,
    ipv6: Option<String>,
    replicas: u16,
}

async fn wanted(state: &AppState, rec: &ServiceRecord) -> Vec<Wanted> {
    if rec.stopped || rec.service.live_deployment_id.is_none() {
        return Vec::new();
    }
    let mut counts: Vec<(Id, u16)> = Vec::new();
    for r in replicas::describe(state, rec).await {
        if r.health != ReplicaHealth::Healthy || !r.routed {
            continue;
        }
        match counts.iter_mut().find(|(id, _)| *id == r.server_id) {
            Some((_, n)) => *n += 1,
            None => counts.push((r.server_id.clone(), 1)),
        }
    }
    let mut out = Vec::new();
    for (server_id, replicas) in counts {
        let Ok(Some(server)) = state.db.server(&server_id).await else {
            continue;
        };
        let (v4, v6) = state.server_ips(&server).await;
        out.push(Wanted {
            server,
            ipv4: v4.map(|ip| ip.to_string()),
            ipv6: v6.map(|ip| ip.to_string()),
            replicas,
        });
    }
    out
}

fn wanted_targets(wanted: &[Wanted]) -> Vec<LbTarget> {
    wanted
        .iter()
        .filter_map(|w| {
            Some(LbTarget {
                server_id: w.server.id.clone(),
                address: w.ipv4.clone().or_else(|| w.ipv6.clone())?,
                replicas: w.replicas,
                provider_health: None,
            })
        })
        .collect()
}

/// The hosts a load balancer handles: the ones it names, or every domain of the service.
fn hosts(config: &LbConfig, domains: &[Domain]) -> Vec<String> {
    domains
        .iter()
        .map(|d| d.host.to_ascii_lowercase())
        .filter(|h| {
            config.domains.is_empty() || config.domains.iter().any(|d| d.eq_ignore_ascii_case(h))
        })
        .collect()
}

fn labels(svc: &Service) -> HashMap<String, String> {
    HashMap::from([
        (brand::label("managed"), "true".to_string()),
        (brand::label("service"), svc.id.clone()),
    ])
}

/// The comment or description everything created at Cloudflare carries.
fn marker(svc: &Service) -> String {
    format!("{} managed · service {}", brand::NAME, svc.id)
}

fn lb_name(svc: &Service) -> String {
    let id: String = svc.id.chars().take(6).collect();
    let mut name = format!("{}-{}-{id}", brand::PREFIX, svc.name);
    name.truncate(63);
    name
}

async fn provider(state: &AppState, id: &str) -> ApiResult<(Provider, Zeroizing<String>)> {
    let provider = state
        .db
        .provider(id)
        .await?
        .ok_or_else(|| AppError::not_found("provider", id))?;
    let token = token(state, &provider).await?;
    Ok((provider, token))
}

pub async fn token(state: &AppState, provider: &Provider) -> Result<Zeroizing<String>> {
    match state.secrets.value(&provider.token_secret_id).await? {
        SecretValue::Generic { value } => Ok(Zeroizing::new(value)),
        _ => bail!(
            "the token secret of {} is not a generic secret",
            provider.name
        ),
    }
}

/// Syncs one service's load balancer now and stores the outcome.
pub async fn sync(
    state: &Arc<AppState>,
    service_id: &str,
    force: bool,
) -> Result<Option<ServiceLb>> {
    let _guard = state.lb.lock.lock().await;
    let Some(mut lb) = state.db.service_lb(service_id).await? else {
        return Ok(None);
    };
    let Some(rec) = state.db.service(service_id).await? else {
        return Ok(None);
    };
    let was = lb.stored.state;
    let result = sync_once(state, &rec, &mut lb, force).await;
    let now = Utc::now();
    let svc = &rec.service;
    match result {
        Ok(()) => {
            lb.stored.last_sync_at = Some(now);
            lb.stored.failures = 0;
            lb.stored.next_attempt_at = None;
            if lb.stored.state == LbSyncState::Drift {
                if !lb.stored.drift_notified {
                    lb.stored.drift_notified = true;
                    state
                        .service_activity(
                            svc,
                            ActivityLevel::Error,
                            "Load balancer changed outside".into(),
                            Some(lb.stored.drift.join("; ")),
                        )
                        .await;
                    state
                        .notify_service(
                            svc,
                            Event::new(NotifyEvent::LbDriftDetected)
                                .detail(lb.stored.drift.join("; "))
                                .hint(format!(
                                    "{} service lb sync {} --force",
                                    brand::CLI_BIN,
                                    svc.name
                                )),
                        )
                        .await;
                }
            } else {
                lb.stored.last_error = None;
                lb.stored.drift.clear();
                lb.stored.drift_notified = false;
                if lb.stored.failure_notified || was == LbSyncState::Drift {
                    lb.stored.failure_notified = false;
                    state
                        .notify_service(
                            svc,
                            Event::new(NotifyEvent::LbRecovered)
                                .detail(format!("{} targets", lb.stored.targets.len())),
                        )
                        .await;
                }
            }
        }
        Err(e) => {
            lb.stored.state = LbSyncState::Error;
            lb.stored.last_error = Some(format!("{e:#}"));
            match e.downcast_ref::<RateLimited>() {
                Some(limited) => lb.stored.next_attempt_at = Some(limited.until),
                None => {
                    lb.stored.failures = lb.stored.failures.saturating_add(1);
                    lb.stored.next_attempt_at =
                        Some(now + TimeDelta::seconds(backoff_secs(lb.stored.failures)));
                }
            }
            if lb.stored.failures >= NOTIFY_AFTER && !lb.stored.failure_notified {
                lb.stored.failure_notified = true;
                state
                    .service_activity(
                        svc,
                        ActivityLevel::Error,
                        "Load balancer sync failed".into(),
                        Some(format!("{e:#}")),
                    )
                    .await;
                state
                    .notify_service(
                        svc,
                        Event::new(NotifyEvent::LbSyncFailed)
                            .detail(format!("{e:#}"))
                            .field("Attempts", lb.stored.failures.to_string())
                            .hint(format!("{} service lb status {}", brand::CLI_BIN, svc.name)),
                    )
                    .await;
            }
        }
    }
    state.db.save_lb_state(service_id, &lb.stored).await?;
    Ok(Some(to_api(&lb)))
}

/// Seconds to wait after the `failures`-th failure in a row: 30s, 1m, 2m, … up to 30 minutes.
pub fn backoff_secs(failures: u32) -> i64 {
    let doublings = failures.saturating_sub(1).min(16);
    (FIRST_BACKOFF_SECS << doublings).min(MAX_BACKOFF_SECS)
}

pub fn to_api(lb: &LbRecord) -> ServiceLb {
    let s = &lb.stored;
    ServiceLb {
        config: lb.config.clone(),
        state: s.state,
        targets: s.targets.clone(),
        resources: s.resources.clone(),
        addresses: s.addresses.clone(),
        dns: s.dns.clone(),
        drift: s.drift.clone(),
        last_sync_at: s.last_sync_at,
        last_error: s.last_error.clone(),
        next_attempt_at: s.next_attempt_at,
    }
}

async fn sync_once(
    state: &AppState,
    rec: &ServiceRecord,
    lb: &mut LbRecord,
    force: bool,
) -> Result<()> {
    let (provider, token) = provider(state, &lb.config.provider_id)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let domains = state.db.domains(&rec.service.id).await?;
    let hosts = hosts(&lb.config, &domains);
    if hosts.is_empty() {
        bail!(
            "'{}' has none of the load balancer's domains",
            rec.service.name
        );
    }
    let wanted = wanted(state, rec).await;
    let ctx = Ctx {
        svc: &rec.service,
        hosts: &hosts,
        wanted: &wanted,
        force,
    };
    let mut drift = Vec::new();
    match lb.config.mode {
        LbMode::Hetzner => {
            let hz = state.lb.hetzner(token);
            let dns = match &lb.config.dns_provider_id {
                Some(id) => {
                    let (_, token) = self::provider(state, id)
                        .await
                        .map_err(|e| anyhow!("{e}"))?;
                    Some(state.lb.cloudflare(token))
                }
                None => None,
            };
            sync_hetzner(&ctx, &hz, dns.as_ref(), lb, &mut drift).await?;
        }
        LbMode::CloudflareDns => {
            let cf = state.lb.cloudflare(token);
            sync_cloudflare_dns(&ctx, &cf, lb, &mut drift).await?;
        }
        LbMode::CloudflareLb => {
            if !provider.load_balancing {
                bail!("{} has no Cloudflare Load Balancing", provider.name);
            }
            let cf = state.lb.cloudflare(token);
            sync_cloudflare_lb(&ctx, &cf, lb, &mut drift).await?;
        }
    }
    lb.stored.drift = drift;
    lb.stored.state = if lb.stored.drift.is_empty() {
        LbSyncState::InSync
    } else {
        LbSyncState::Drift
    };
    Ok(())
}

struct Ctx<'a> {
    svc: &'a Service,
    hosts: &'a [String],
    wanted: &'a [Wanted],
    force: bool,
}

/// Each wanted server as a Hetzner target: by id when it is a Hetzner Cloud server of the
/// project (on its private address when the load balancer is on a network), else by address.
fn hetzner_targets(
    wanted: &[Wanted],
    servers: &[HetznerServer],
    network: Option<i64>,
) -> Vec<(LbTarget, Target)> {
    wanted
        .iter()
        .filter_map(|w| {
            let found = servers.iter().find(|h| {
                h.public_ipv4.is_some() && h.public_ipv4 == w.ipv4
                    || h.private_networks
                        .iter()
                        .any(|p| Some(&p.ip) == w.ipv4.as_ref())
            });
            let (target, address) = match (found, network) {
                (Some(h), Some(net)) => {
                    let ip = h
                        .private_networks
                        .iter()
                        .find(|p| p.network_id == net)?
                        .ip
                        .clone();
                    (
                        Target::Server {
                            id: h.id,
                            private: true,
                        },
                        ip,
                    )
                }
                (Some(h), None) => (
                    Target::Server {
                        id: h.id,
                        private: false,
                    },
                    h.public_ipv4.clone().or_else(|| w.ipv4.clone())?,
                ),
                (None, _) => {
                    let ip = w.ipv4.clone()?;
                    (Target::Ip(ip.clone()), ip)
                }
            };
            Some((
                LbTarget {
                    server_id: w.server.id.clone(),
                    address,
                    replicas: w.replicas,
                    provider_health: None,
                },
                target,
            ))
        })
        .collect()
}

fn drift_lines(what: &str, applied: &BTreeSet<String>, observed: &BTreeSet<String>) -> Vec<String> {
    let mut lines: Vec<String> = observed
        .difference(applied)
        .map(|t| format!("{what} {t} added outside {}", brand::NAME))
        .collect();
    lines.extend(
        applied
            .difference(observed)
            .map(|t| format!("{what} {t} removed outside {}", brand::NAME)),
    );
    lines
}

async fn sync_hetzner(
    ctx: &Ctx<'_>,
    hz: &Hetzner,
    dns: Option<&Cloudflare>,
    lb: &mut LbRecord,
    drift: &mut Vec<String>,
) -> Result<()> {
    let config = lb.config.clone();
    let servers = hz.servers().await?;
    let targets = hetzner_targets(ctx.wanted, &servers, config.network_id);
    let wanted: BTreeSet<String> = targets.iter().map(|(_, t)| t.key()).collect();
    let health_path = crate::docker::lb_health_path();
    let resource = lb
        .stored
        .resources
        .iter()
        .find(|r| r.kind == LbResourceKind::HetznerLoadBalancer)
        .cloned();
    let existing = match &resource {
        Some(r) => {
            hz.load_balancer(r.id.parse().context("bad load balancer id")?)
                .await?
        }
        None => None,
    };
    let current = match (existing, resource) {
        (Some(current), _) => current,
        (None, Some(r)) if lb.stored.applied.is_some() && !ctx.force => {
            drift.push(format!(
                "load balancer {} was deleted outside {}",
                r.name,
                brand::NAME
            ));
            return Ok(());
        }
        (None, _) => {
            lb.stored
                .resources
                .retain(|r| r.kind != LbResourceKind::HetznerLoadBalancer);
            let (created, resource) = match config.adopt_lb_id {
                Some(id) => {
                    let found = hz
                        .load_balancer(id)
                        .await?
                        .with_context(|| format!("load balancer {id} does not exist"))?;
                    for port in [80, 443] {
                        if !found.listen_ports.contains(&port) {
                            hz.add_service(id, port, &ctx.hosts[0], &health_path)
                                .await?;
                        }
                    }
                    (false, found)
                }
                None => {
                    let location = match &config.location {
                        Some(l) => l.clone(),
                        None => default_location(&targets, &servers),
                    };
                    let created_targets: Vec<Target> =
                        targets.iter().map(|(_, t)| t.clone()).collect();
                    let created = hz
                        .create_lb(&NewLb {
                            name: &lb_name(ctx.svc),
                            lb_type: config.lb_type.as_deref().unwrap_or(DEFAULT_LB_TYPE),
                            location: &location,
                            network_id: config.network_id,
                            labels: labels(ctx.svc),
                            health_host: &ctx.hosts[0],
                            health_path: &health_path,
                            targets: &created_targets,
                        })
                        .await?;
                    (true, created)
                }
            };
            lb.stored.resources.push(LbResource {
                kind: LbResourceKind::HetznerLoadBalancer,
                id: resource.id.to_string(),
                name: resource.name.clone(),
                created,
            });
            let applied = lb.stored.applied.get_or_insert_with(Applied::default);
            applied.targets = resource.targets.iter().map(|(t, _)| t.key()).collect();
            resource
        }
    };
    let observed: BTreeSet<String> = current.targets.iter().map(|(t, _)| t.key()).collect();
    let applied = lb.stored.applied.clone().unwrap_or_default();
    let manual = drift_lines("target", &applied.targets, &observed);
    if !manual.is_empty() && !ctx.force {
        drift.extend(manual);
    } else if wanted.is_empty() {
        lb.stored.last_error =
            Some("no server runs a healthy replica; the load balancer keeps its targets".into());
    } else {
        for (_, target) in &targets {
            if !observed.contains(&target.key()) {
                hz.add_target(current.id, target).await?;
            }
        }
        for (target, _) in &current.targets {
            if !wanted.contains(&target.key()) {
                hz.remove_target(current.id, target).await?;
            }
        }
        lb.stored
            .applied
            .get_or_insert_with(Applied::default)
            .targets = wanted.clone();
    }
    let health: HashMap<String, Option<String>> = current
        .targets
        .iter()
        .map(|(t, h)| (t.key(), h.clone()))
        .collect();
    lb.stored.targets = targets
        .iter()
        .map(|(t, key)| LbTarget {
            provider_health: Some(
                health
                    .get(&key.key())
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| "unknown".into()),
            ),
            ..t.clone()
        })
        .collect();
    lb.stored.addresses = current
        .ipv4
        .iter()
        .chain(current.ipv6.iter())
        .cloned()
        .collect();
    let mut records = BTreeSet::new();
    if let Some(v4) = &current.ipv4 {
        records.insert(("A".to_string(), v4.clone()));
    }
    if let Some(v6) = &current.ipv6 {
        records.insert(("AAAA".to_string(), v6.trim_end_matches("/64").to_string()));
    }
    match dns {
        Some(cf) => {
            lb.stored.dns.clear();
            if drift.is_empty() || ctx.force {
                let zones = cf.zones().await?;
                for host in ctx.hosts {
                    sync_records(
                        ctx,
                        cf,
                        &zones,
                        host,
                        &records,
                        config.proxied(),
                        config.adopt_records,
                        lb,
                        drift,
                    )
                    .await?;
                }
            }
        }
        None => {
            lb.stored.dns = ctx
                .hosts
                .iter()
                .flat_map(|h| {
                    records.iter().map(move |(kind, value)| DnsRecord {
                        kind: kind.clone(),
                        name: h.clone(),
                        value: value.clone(),
                    })
                })
                .collect();
        }
    }
    Ok(())
}

/// The location of the first Hetzner server among the targets, or Falkenstein.
fn default_location(targets: &[(LbTarget, Target)], servers: &[HetznerServer]) -> String {
    targets
        .iter()
        .find_map(|(_, t)| match t {
            Target::Server { id, .. } => servers
                .iter()
                .find(|s| s.id == *id)
                .map(|s| s.location.clone()),
            Target::Ip(_) => None,
        })
        .unwrap_or_else(|| "fsn1".into())
}

/// Makes the A/AAAA records of `host` exactly `wanted`, using only records the daemon created
/// or adopted. Records it doesn't own are adopted on the first sync when allowed, and refused
/// otherwise; ones that appear later are reported as drift.
#[allow(clippy::too_many_arguments)]
async fn sync_records(
    ctx: &Ctx<'_>,
    cf: &Cloudflare,
    zones: &[CloudflareZone],
    host: &str,
    wanted: &BTreeSet<(String, String)>,
    proxied: bool,
    adopt: bool,
    lb: &mut LbRecord,
    drift: &mut Vec<String>,
) -> Result<()> {
    let zone = cloudflare::zone_for(zones, host)
        .with_context(|| format!("no zone for {host} in the Cloudflare account"))?;
    let key = |id: &str| format!("{}/{id}", zone.id);
    let existing = cf.records(&zone.id, host).await?;
    let owned = |r: &Record, lb: &LbRecord| {
        lb.stored
            .resources
            .iter()
            .any(|res| res.kind == LbResourceKind::CloudflareDnsRecord && res.id == key(&r.id))
    };
    let first = !lb
        .stored
        .resources
        .iter()
        .any(|r| r.kind == LbResourceKind::CloudflareDnsRecord && r.name == host);
    let foreign: Vec<Record> = existing.iter().filter(|r| !owned(r, lb)).cloned().collect();
    if !foreign.is_empty() {
        let list: Vec<String> = foreign.iter().map(|r| r.value()).collect();
        if first && adopt {
            for r in &foreign {
                lb.stored.resources.push(LbResource {
                    kind: LbResourceKind::CloudflareDnsRecord,
                    id: key(&r.id),
                    name: host.to_string(),
                    created: false,
                });
                lb.stored
                    .applied
                    .get_or_insert_with(Applied::default)
                    .records
                    .insert(key(&r.id), r.value());
            }
        } else if first {
            bail!(
                "{host} already has records {} did not create ({}); enable with adopt_records to take them over",
                brand::NAME,
                list.join(", ")
            );
        } else if !ctx.force {
            drift.push(format!(
                "{host}: records {} added outside {}",
                list.join(", "),
                brand::NAME
            ));
            return Ok(());
        }
    }
    let applied = lb.stored.applied.clone().unwrap_or_default();
    let mut manual = Vec::new();
    for res in lb
        .stored
        .resources
        .iter()
        .filter(|r| r.kind == LbResourceKind::CloudflareDnsRecord && r.name == host)
    {
        let now = existing
            .iter()
            .find(|r| key(&r.id) == res.id)
            .map(Record::value);
        match (applied.records.get(&res.id), now) {
            (Some(was), Some(now)) if *was != now => manual.push(format!(
                "{host}: {was} changed to {now} outside {}",
                brand::NAME
            )),
            (Some(was), None) => {
                manual.push(format!("{host}: {was} deleted outside {}", brand::NAME))
            }
            _ => {}
        }
    }
    if !manual.is_empty() && !ctx.force {
        drift.extend(manual);
        return Ok(());
    }
    let host_keys: Vec<String> = lb
        .stored
        .resources
        .iter()
        .filter(|r| r.kind == LbResourceKind::CloudflareDnsRecord && r.name == host)
        .map(|r| r.id.clone())
        .collect();
    let ours: Vec<Record> = existing.iter().filter(|r| owned(r, lb)).cloned().collect();
    lb.stored.resources.retain(|r| {
        !(r.kind == LbResourceKind::CloudflareDnsRecord
            && r.name == host
            && !ours.iter().any(|o| key(&o.id) == r.id))
    });
    if wanted.is_empty() {
        lb.stored.last_error =
            Some("no server runs a healthy replica; the records stay as they are".into());
        return Ok(());
    }
    let comment = marker(ctx.svc);
    let mut keep: Vec<&Record> = Vec::new();
    let mut missing: Vec<&(String, String)> = Vec::new();
    for w in wanted {
        match ours
            .iter()
            .find(|r| r.kind == w.0 && r.content == w.1 && !keep.iter().any(|k| k.id == r.id))
        {
            Some(r) => keep.push(r),
            None => missing.push(w),
        }
    }
    let mut spare: Vec<&Record> = ours
        .iter()
        .filter(|r| !keep.iter().any(|k| k.id == r.id))
        .collect();
    let mut records: BTreeMap<String, String> = BTreeMap::new();
    for r in &keep {
        if r.proxied != proxied {
            cf.update_record(&zone.id, &r.id, &r.kind, &r.content, proxied, &comment)
                .await?;
        }
        records.insert(key(&r.id), r.value());
    }
    for (kind, value) in missing {
        let reuse = spare.iter().position(|r| r.kind == *kind);
        match reuse {
            Some(i) => {
                let r = spare.remove(i);
                cf.update_record(&zone.id, &r.id, kind, value, proxied, &comment)
                    .await?;
                records.insert(key(&r.id), format!("{kind} {value}"));
            }
            None => {
                let r = cf
                    .create_record(&zone.id, kind, host, value, proxied, &comment)
                    .await?;
                lb.stored.resources.push(LbResource {
                    kind: LbResourceKind::CloudflareDnsRecord,
                    id: key(&r.id),
                    name: host.to_string(),
                    created: true,
                });
                records.insert(key(&r.id), r.value());
            }
        }
    }
    for r in spare {
        cf.delete_record(&zone.id, &r.id).await?;
        lb.stored.resources.retain(|res| res.id != key(&r.id));
    }
    let applied = lb.stored.applied.get_or_insert_with(Applied::default);
    applied.records.retain(|k, _| !host_keys.contains(k));
    applied.records.extend(records);
    Ok(())
}

async fn sync_cloudflare_dns(
    ctx: &Ctx<'_>,
    cf: &Cloudflare,
    lb: &mut LbRecord,
    drift: &mut Vec<String>,
) -> Result<()> {
    let zones = cf.zones().await?;
    let mut wanted = BTreeSet::new();
    for w in ctx.wanted {
        if let Some(v4) = &w.ipv4 {
            wanted.insert(("A".to_string(), v4.clone()));
        }
        if let Some(v6) = &w.ipv6 {
            wanted.insert(("AAAA".to_string(), v6.clone()));
        }
    }
    let (proxied, adopt) = (lb.config.proxied(), lb.config.adopt_records);
    for host in ctx.hosts {
        sync_records(ctx, cf, &zones, host, &wanted, proxied, adopt, lb, drift).await?;
    }
    lb.stored.applied.get_or_insert_with(Applied::default);
    lb.stored.targets = wanted_targets(ctx.wanted);
    Ok(())
}

async fn sync_cloudflare_lb(
    ctx: &Ctx<'_>,
    cf: &Cloudflare,
    lb: &mut LbRecord,
    drift: &mut Vec<String>,
) -> Result<()> {
    let zones = cf.zones().await?;
    let zone = cloudflare::zone_for(&zones, &ctx.hosts[0])
        .with_context(|| format!("no zone for {} in the Cloudflare account", ctx.hosts[0]))?
        .clone();
    let account = zone.account_id.clone();
    let description = marker(ctx.svc);
    let origins: Vec<Origin> = ctx
        .wanted
        .iter()
        .filter_map(|w| {
            Some(Origin {
                name: w.server.name.clone(),
                address: w.ipv4.clone().or_else(|| w.ipv6.clone())?,
            })
        })
        .collect();
    let wanted: BTreeSet<String> = origins.iter().map(|o| o.address.clone()).collect();
    let find = |lb: &LbRecord, kind| lb.stored.resources.iter().find(|r| r.kind == kind).cloned();
    let monitor = match find(lb, LbResourceKind::CloudflareMonitor) {
        Some(m) => m.id,
        None => {
            let id = cf
                .create_monitor(
                    &account,
                    &ctx.hosts[0],
                    &crate::docker::lb_health_path(),
                    &description,
                )
                .await?;
            lb.stored.resources.push(LbResource {
                kind: LbResourceKind::CloudflareMonitor,
                id: format!("{account}/{id}"),
                name: format!("{} health", ctx.hosts[0]),
                created: true,
            });
            format!("{account}/{id}")
        }
    };
    let monitor_id = monitor.rsplit('/').next().unwrap_or_default().to_string();
    let pool = find(lb, LbResourceKind::CloudflarePool);
    let observed = match &pool {
        Some(p) => {
            cf.pool_origins(&account, p.id.rsplit('/').next().unwrap_or_default())
                .await?
        }
        None => None,
    };
    let pool_id = match (pool, observed) {
        (Some(p), Some(observed)) => {
            let observed: BTreeSet<String> = observed.into_iter().map(|o| o.address).collect();
            let applied = lb.stored.applied.clone().unwrap_or_default();
            let manual = drift_lines("origin", &applied.targets, &observed);
            let id = p.id.rsplit('/').next().unwrap_or_default().to_string();
            if !manual.is_empty() && !ctx.force {
                drift.extend(manual);
                lb.stored.targets = wanted_targets(ctx.wanted);
                return Ok(());
            }
            if wanted.is_empty() {
                lb.stored.last_error =
                    Some("no server runs a healthy replica; the pool keeps its origins".into());
            } else if wanted != observed {
                cf.set_pool_origins(&account, &id, &origins).await?;
                lb.stored
                    .applied
                    .get_or_insert_with(Applied::default)
                    .targets = wanted.clone();
            }
            id
        }
        (Some(p), None) if !ctx.force => {
            drift.push(format!(
                "pool {} was deleted outside {}",
                p.name,
                brand::NAME
            ));
            return Ok(());
        }
        (_, _) => {
            lb.stored
                .resources
                .retain(|r| r.kind != LbResourceKind::CloudflarePool);
            let name = lb_name(ctx.svc);
            let id = cf
                .create_pool(&account, &name, &origins, &monitor_id, &description)
                .await?;
            lb.stored.resources.push(LbResource {
                kind: LbResourceKind::CloudflarePool,
                id: format!("{account}/{id}"),
                name,
                created: true,
            });
            lb.stored
                .applied
                .get_or_insert_with(Applied::default)
                .targets = wanted.clone();
            id
        }
    };
    for host in ctx.hosts {
        let zone = cloudflare::zone_for(&zones, host)
            .with_context(|| format!("no zone for {host} in the Cloudflare account"))?;
        let existing = lb
            .stored
            .resources
            .iter()
            .find(|r| r.kind == LbResourceKind::CloudflareLoadBalancer && r.name == *host)
            .cloned();
        if let Some(r) = &existing {
            let id = r.id.rsplit('/').next().unwrap_or_default();
            if cf.lb_exists(&zone.id, id).await? {
                continue;
            }
            if !ctx.force {
                drift.push(format!(
                    "load balancer {host} was deleted outside {}",
                    brand::NAME
                ));
                continue;
            }
            lb.stored.resources.retain(|x| x.id != r.id);
        }
        let id = cf
            .create_lb(&zone.id, host, &pool_id, lb.config.proxied(), &description)
            .await?;
        lb.stored.resources.push(LbResource {
            kind: LbResourceKind::CloudflareLoadBalancer,
            id: format!("{}/{id}", zone.id),
            name: host.clone(),
            created: true,
        });
    }
    lb.stored.targets = wanted_targets(ctx.wanted);
    Ok(())
}

/// What turning on `config` would do and roughly cost. Refuses configs that can't work.
pub async fn plan(state: &AppState, rec: &ServiceRecord, config: &LbConfig) -> ApiResult<LbPlan> {
    let svc = &rec.service;
    if !replicas::has_route(&svc.source) {
        bad_request!(
            "compose services route themselves; a managed load balancer needs a single-container service"
        );
    }
    let (provider, token) = provider(state, &config.provider_id).await?;
    if provider.kind != config.mode.provider() {
        bad_request!(
            "{} is a {} connection; {} needs {}",
            provider.name,
            provider.kind.label(),
            config.mode.label(),
            config.mode.provider().label()
        );
    }
    if config.mode == LbMode::CloudflareLb && !provider.load_balancing {
        bad_request!(
            "{} has no Cloudflare Load Balancing; use cloudflare-dns, or add Load Balancing to the account",
            provider.name
        );
    }
    let domains = state.db.domains(&svc.id).await?;
    for d in &config.domains {
        if !domains.iter().any(|x| x.host.eq_ignore_ascii_case(d)) {
            bad_request!("'{}' has no domain {d}", svc.name);
        }
    }
    let hosts = hosts(config, &domains);
    if hosts.is_empty() {
        bad_request!("'{}' has no domains; add one first", svc.name);
    }
    let wanted = wanted(state, rec).await;
    let mut out = LbPlan {
        actions: Vec::new(),
        targets: wanted_targets(&wanted),
        monthly_cost: None,
        currency: String::new(),
        cost_note: String::new(),
        dns: Vec::new(),
        warnings: Vec::new(),
    };
    if wanted.is_empty() {
        out.warnings.push(format!(
            "no server runs a healthy replica of '{}' yet; targets are added as replicas start",
            svc.name
        ));
    }
    if svc.scaling.placement.servers.len() < 2 {
        out.warnings.push(format!(
            "'{}' runs on one server; set its placement to spread it over more",
            svc.name
        ));
    }
    let api = |e: anyhow::Error| AppError::BadRequest(format!("{e:#}"));
    match config.mode {
        LbMode::Hetzner => {
            let hz = state.lb.hetzner(token);
            let servers = hz.servers().await.map_err(api)?;
            let targets = hetzner_targets(&wanted, &servers, config.network_id);
            for w in &wanted {
                match targets.iter().find(|(t, _)| t.server_id == w.server.id) {
                    Some((_, Target::Ip(ip))) => out.warnings.push(format!("{} ({ip}) is not a Hetzner Cloud server of this project; Hetzner accepts address targets only for its dedicated servers", w.server.name)),
                    None if config.network_id.is_some() => bad_request!("{} is not in network {}; every target must be in the load balancer's network", w.server.name, config.network_id.unwrap_or_default()),
                    _ => {}
                }
            }
            out.targets = targets.iter().map(|(t, _)| t.clone()).collect();
            match config.adopt_lb_id {
                Some(id) => {
                    let found = hz.load_balancer(id).await.map_err(api)?.ok_or_else(|| {
                        AppError::BadRequest(format!("load balancer {id} does not exist"))
                    })?;
                    out.actions.push(LbAction {
                        verb: LbVerb::Adopt,
                        resource: LbResourceKind::HetznerLoadBalancer,
                        name: found.name.clone(),
                        detail: format!(
                            "{} in {}; targets and services 80/443 managed, never deleted",
                            found.lb_type, found.location
                        ),
                    });
                }
                None => {
                    let lb_type = config
                        .lb_type
                        .clone()
                        .unwrap_or_else(|| DEFAULT_LB_TYPE.into());
                    let location = config
                        .location
                        .clone()
                        .unwrap_or_else(|| default_location(&targets, &servers));
                    let types = hz.lb_types().await.map_err(api)?;
                    let t = types.iter().find(|t| t.name == lb_type).ok_or_else(|| {
                        AppError::BadRequest(format!("unknown load balancer type {lb_type}"))
                    })?;
                    if let Some(price) = t.prices.iter().find(|p| p.location == location) {
                        out.monthly_cost = Some(price.monthly);
                        out.currency = price.currency.clone();
                        out.cost_note =
                            format!("Hetzner list price for {lb_type} in {location}, VAT included");
                    } else {
                        bad_request!("{lb_type} is not offered in {location}");
                    }
                    if let Some(zone) = servers
                        .iter()
                        .find(|s| s.location == location)
                        .map(|s| s.network_zone.clone())
                    {
                        for (target, key) in &targets {
                            if let Target::Server { id, .. } = key
                                && let Some(s) = servers.iter().find(|s| s.id == *id)
                                && s.network_zone != zone
                            {
                                out.warnings.push(format!(
                                    "{} is in {}, not {zone} like {location}",
                                    target.address, s.network_zone
                                ));
                            }
                        }
                    }
                    out.actions.push(LbAction {
                        verb: LbVerb::Create,
                        resource: LbResourceKind::HetznerLoadBalancer,
                        name: lb_name(svc),
                        detail: format!(
                            "{lb_type} in {location} · TCP 80 and 443 to {} target{}",
                            targets.len(),
                            if targets.len() == 1 { "" } else { "s" }
                        ),
                    });
                }
            }
            match &config.dns_provider_id {
                Some(id) => {
                    let (dns, token) = self::provider(state, id).await?;
                    if dns.kind != ProviderKind::Cloudflare {
                        bad_request!("{} is not a Cloudflare connection", dns.name);
                    }
                    let cf = state.lb.cloudflare(token);
                    plan_records(
                        &cf,
                        &hosts,
                        &["the load balancer's IPv4", "the load balancer's IPv6"],
                        config.adopt_records,
                        &mut out,
                    )
                    .await?;
                }
                None => {
                    for h in &hosts {
                        out.dns.push(DnsRecord {
                            kind: "A".into(),
                            name: h.clone(),
                            value: "the load balancer's IPv4, shown once it exists".into(),
                        });
                    }
                    out.warnings.push("no DNS provider: point the domains at the load balancer's address yourself".into());
                }
            }
        }
        LbMode::CloudflareDns => {
            let cf = state.lb.cloudflare(token);
            let values: Vec<String> = out.targets.iter().map(|t| t.address.clone()).collect();
            let values: Vec<&str> = values.iter().map(String::as_str).collect();
            plan_records(&cf, &hosts, &values, config.adopt_records, &mut out).await?;
            out.cost_note = "DNS records are free".into();
        }
        LbMode::CloudflareLb => {
            let cf = state.lb.cloudflare(token);
            let zones = cf.zones().await.map_err(api)?;
            for h in &hosts {
                if cloudflare::zone_for(&zones, h).is_none() {
                    bad_request!("no zone for {h} in the Cloudflare account");
                }
            }
            out.actions.push(LbAction {
                verb: LbVerb::Create,
                resource: LbResourceKind::CloudflareMonitor,
                name: format!("{} health", hosts[0]),
                detail: format!(
                    "GET {} on port 80 every 60s",
                    crate::docker::lb_health_path()
                ),
            });
            out.actions.push(LbAction {
                verb: LbVerb::Create,
                resource: LbResourceKind::CloudflarePool,
                name: lb_name(svc),
                detail: format!(
                    "{} origin{}",
                    out.targets.len(),
                    if out.targets.len() == 1 { "" } else { "s" }
                ),
            });
            for h in &hosts {
                out.actions.push(LbAction {
                    verb: LbVerb::Create,
                    resource: LbResourceKind::CloudflareLoadBalancer,
                    name: h.clone(),
                    detail: if config.proxied() {
                        "proxied".into()
                    } else {
                        "DNS only".into()
                    },
                });
            }
            let extra = out.targets.len().saturating_sub(2) as f64;
            out.monthly_cost = Some(CLOUDFLARE_LB_BASE_USD + extra * CLOUDFLARE_LB_ORIGIN_USD);
            out.currency = "USD".into();
            out.cost_note = "rough Cloudflare list price: the base covers two origins, each further origin adds the same".into();
        }
    }
    Ok(out)
}

/// The record actions for `hosts`, refusing records the daemon doesn't own unless adopted.
async fn plan_records(
    cf: &Cloudflare,
    hosts: &[String],
    values: &[&str],
    adopt: bool,
    out: &mut LbPlan,
) -> ApiResult<()> {
    let zones = cf
        .zones()
        .await
        .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
    for h in hosts {
        let zone = cloudflare::zone_for(&zones, h).ok_or_else(|| {
            AppError::BadRequest(format!("no zone for {h} in the Cloudflare account"))
        })?;
        let existing = cf
            .records(&zone.id, h)
            .await
            .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
        if !existing.is_empty() {
            let list: Vec<String> = existing.iter().map(Record::value).collect();
            if !adopt {
                conflict!(
                    "{h} already has records {} did not create ({}); turn on adopt_records to take them over",
                    brand::NAME,
                    list.join(", ")
                );
            }
            out.actions.push(LbAction {
                verb: LbVerb::Adopt,
                resource: LbResourceKind::CloudflareDnsRecord,
                name: h.clone(),
                detail: format!(
                    "{}; kept when the load balancer is turned off",
                    list.join(", ")
                ),
            });
        }
        for v in values {
            out.actions.push(LbAction {
                verb: LbVerb::Create,
                resource: LbResourceKind::CloudflareDnsRecord,
                name: h.clone(),
                detail: format!("→ {v}"),
            });
        }
    }
    Ok(())
}

/// Turns the load balancer on, or changes it, after the client confirmed the plan.
pub async fn enable(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    body: EnableLb,
) -> ApiResult<ServiceLb> {
    if !body.confirm {
        bad_request!(
            "confirm the plan from POST /services/{{id}}/lb/plan by sending confirm: true"
        );
    }
    plan(state, rec, &body.config).await?;
    let _guard = state.lb.lock.lock().await;
    let svc = &rec.service;
    let stored = match state.db.service_lb(&svc.id).await? {
        Some(old)
            if old.config.provider_id != body.config.provider_id
                || old.config.mode != body.config.mode =>
        {
            conflict!(
                "'{}' already has a {} load balancer; turn it off first",
                svc.name,
                old.config.mode.label()
            );
        }
        Some(old) => LbStored {
            next_attempt_at: None,
            ..old.stored
        },
        None => LbStored::default(),
    };
    let lb = LbRecord {
        config: body.config,
        stored,
    };
    state.db.save_service_lb(&svc.id, &lb).await?;
    state
        .service_activity(
            svc,
            ActivityLevel::Info,
            format!("Load balancer on: {}", lb.config.mode.label()),
            None,
        )
        .await;
    reroute(state, &svc.id);
    state.lb_kick();
    Ok(to_api(&lb))
}

/// Turns the load balancer off. With `delete_resources`, deletes what the daemon created;
/// adopted resources always stay. A failed deletion keeps the load balancer on, holding what
/// is left, so nothing is forgotten at the provider.
pub async fn disable(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    delete_resources: bool,
) -> ApiResult<LbTeardown> {
    let _guard = state.lb.lock.lock().await;
    let svc = &rec.service;
    let Some(mut lb) = state.db.service_lb(&svc.id).await? else {
        return Err(AppError::NotFound(format!(
            "'{}' has no load balancer",
            svc.name
        )));
    };
    let mut out = LbTeardown::default();
    if delete_resources {
        let order = [
            LbResourceKind::CloudflareLoadBalancer,
            LbResourceKind::CloudflarePool,
            LbResourceKind::CloudflareMonitor,
            LbResourceKind::CloudflareDnsRecord,
            LbResourceKind::HetznerLoadBalancer,
        ];
        let mut resources = lb.stored.resources.clone();
        resources.sort_by_key(|r| order.iter().position(|k| *k == r.kind));
        for r in resources {
            if !r.created {
                out.kept.push(r);
                continue;
            }
            match delete_resource(state, &lb.config, &r).await {
                Ok(()) => {
                    lb.stored.resources.retain(|x| x != &r);
                    out.deleted.push(r);
                }
                Err(e) => {
                    state.db.save_lb_state(&svc.id, &lb.stored).await?;
                    return Err(AppError::Internal(e.context(format!(
                        "cannot delete {} {}; the load balancer stays on with what is left",
                        r.kind.label(),
                        r.name
                    ))));
                }
            }
        }
    } else {
        out.kept = lb.stored.resources.clone();
    }
    state.db.delete_service_lb(&svc.id).await?;
    let detail = format!("{} deleted, {} kept", out.deleted.len(), out.kept.len());
    state
        .service_activity(
            svc,
            ActivityLevel::Info,
            "Load balancer off".into(),
            Some(detail),
        )
        .await;
    reroute(state, &svc.id);
    Ok(out)
}

async fn delete_resource(state: &AppState, config: &LbConfig, r: &LbResource) -> Result<()> {
    let provider_id = match (r.kind, config.mode, &config.dns_provider_id) {
        (LbResourceKind::CloudflareDnsRecord, LbMode::Hetzner, Some(dns)) => dns.clone(),
        _ => config.provider_id.clone(),
    };
    let (_, token) = provider(state, &provider_id)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let (scope, id) = r.id.rsplit_once('/').unwrap_or(("", &r.id));
    match r.kind {
        LbResourceKind::HetznerLoadBalancer => state.lb.hetzner(token).delete_lb(id.parse()?).await,
        LbResourceKind::CloudflareDnsRecord => {
            state.lb.cloudflare(token).delete_record(scope, id).await
        }
        LbResourceKind::CloudflareLoadBalancer => {
            state.lb.cloudflare(token).delete_lb(scope, id).await
        }
        LbResourceKind::CloudflarePool => state.lb.cloudflare(token).delete_pool(scope, id).await,
        LbResourceKind::CloudflareMonitor => {
            state.lb.cloudflare(token).delete_monitor(scope, id).await
        }
    }
}

/// Rewrites the service's routes on every server it runs on, in the background: the health
/// endpoint and challenge relay come and go with the load balancer and the placement.
pub fn reroute(state: &Arc<AppState>, service_id: &str) {
    let state = state.clone();
    let id = service_id.to_string();
    tokio::spawn(async move {
        let Ok(Some(rec)) = state.db.service(&id).await else {
            return;
        };
        if rec.service.live_deployment_id.is_none() {
            return;
        }
        let servers = rec.replica_servers();
        let fleet = replicas::Fleet::connect(&state, servers.iter().map(String::as_str)).await;
        for server_id in servers {
            if !rec.replicas.iter().any(|r| r.server_id == server_id) {
                continue;
            }
            if let Some(docker) = fleet.docker(&server_id)
                && let Err(e) =
                    replicas::apply_route(&state, docker, &rec.service, &server_id).await
            {
                tracing::warn!(
                    "cannot update the route of {} on {server_id}: {e:#}",
                    rec.service.name
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_half_an_hour() {
        assert_eq!(backoff_secs(1), 30);
        assert_eq!(backoff_secs(2), 60);
        assert_eq!(backoff_secs(4), 240);
        assert_eq!(backoff_secs(10), MAX_BACKOFF_SECS);
        assert_eq!(backoff_secs(u32::MAX), MAX_BACKOFF_SECS);
    }

    #[test]
    fn drift_names_what_changed_by_hand() {
        let applied: BTreeSet<String> = ["server:1", "server:2"].map(String::from).into();
        let observed: BTreeSet<String> = ["server:1", "ip:10.0.0.9"].map(String::from).into();
        let lines = drift_lines("target", &applied, &observed);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("ip:10.0.0.9 added"));
        assert!(lines[1].contains("server:2 removed"));
        assert!(drift_lines("target", &applied, &applied).is_empty());
    }
}
