//! Background checks that turn state changes into events: server reachability, host disk and
//! memory, new releases, TLS certificates and storage connections, plus the events of the
//! daemon starting.

use super::Event;
use crate::api;
use crate::db::TlsRecord;
use crate::state::AppState;
use chrono::{DateTime, Utc};
use common::notify::NotifyEvent;
use common::{Domain, brand};
use std::sync::Arc;
use std::time::Duration;

const TICK: Duration = Duration::from_secs(60);
/// Servers are checked every tick; slower checks run every this many ticks.
const SLOW_EVERY: u64 = 360;
/// Domains without a certificate yet are probed this often, so issuance shows up quickly.
const NEW_DOMAIN_EVERY: u64 = 5;
/// Usage above this percentage raises an event; it is armed again below the lower one.
const HIGH_PERCENT: f64 = 90.0;
const CLEAR_PERCENT: f64 = 85.0;
/// Memory must stay high for this many ticks.
const MEMORY_TICKS: u32 = 3;
const LAST_VERSION: &str = "last_version";
const NOTIFIED_RELEASE: &str = "notified_release";

/// Emits the startup events. Call once the dispatcher runs.
pub async fn startup(state: &AppState) {
    let current = env!("CARGO_PKG_VERSION");
    let previous = state.db.meta(LAST_VERSION).await.ok().flatten();
    state.notify(
        Event::new(NotifyEvent::DaemonStarted)
            .about(crate::host::hostname(), "daemon")
            .detail(format!(
                "{} {current} listening on {}",
                brand::DAEMON_BIN,
                state.listen
            )),
    );
    if let Some(previous) = previous.filter(|p| p != current) {
        state.notify(
            Event::new(NotifyEvent::UpdateInstalled)
                .about(crate::host::hostname(), "daemon")
                .detail(format!("{previous} → {current}"))
                .field("Version", current),
        );
    }
    if let Err(e) = state.db.set_meta(LAST_VERSION, current).await {
        tracing::warn!("cannot record the running version: {e:#}");
    }
}

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        let mut host = HostWatch::default();
        let mut n: u64 = 0;
        loop {
            tick.tick().await;
            n += 1;
            if n > 1 {
                check_servers(&state).await;
            }
            host.check(&state).await;
            let slow = n % SLOW_EVERY == 2;
            if slow {
                check_release(&state).await;
                check_storage(&state).await;
            }
            if slow || n % NEW_DOMAIN_EVERY == 2 {
                check_certificates(&state, slow).await;
            }
        }
    });
}

/// Reachability: [`api::check_server`] emits the transitions.
async fn check_servers(state: &AppState) {
    for server in state.db.servers().await.unwrap_or_default() {
        let _ = api::check_server(state, server).await;
    }
}

#[derive(Default)]
struct HostWatch {
    disk_raised: bool,
    memory_raised: bool,
    memory_high_ticks: u32,
}

fn percent(used: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| used as f64 / total as f64 * 100.0)
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / f64::from(1 << 30))
}

impl HostWatch {
    async fn check(&mut self, state: &AppState) {
        let sample = state.stats.host();
        let Ok(Some(local)) = state.db.local_server().await else {
            return;
        };
        let event = |kind, used: u64, total: u64, pct: f64| {
            Event::new(kind)
                .about(local.name.clone(), local.id.clone())
                .detail(format!("{pct:.0}% used: {} of {}", gib(used), gib(total)))
        };
        if let Some(pct) = percent(sample.disk_used, sample.disk_total) {
            if pct >= HIGH_PERCENT && !self.disk_raised {
                self.disk_raised = true;
                state.notify(
                    event(
                        NotifyEvent::ServerDiskHigh,
                        sample.disk_used,
                        sample.disk_total,
                        pct,
                    )
                    .hint(format!("{} prune", brand::CLI_BIN)),
                );
            } else if pct < CLEAR_PERCENT {
                self.disk_raised = false;
            }
        }
        if let Some(pct) = percent(sample.mem_used, sample.mem_total) {
            if pct >= HIGH_PERCENT {
                self.memory_high_ticks += 1;
                if self.memory_high_ticks >= MEMORY_TICKS && !self.memory_raised {
                    self.memory_raised = true;
                    state.notify(event(
                        NotifyEvent::ServerMemoryHigh,
                        sample.mem_used,
                        sample.mem_total,
                        pct,
                    ));
                }
            } else {
                self.memory_high_ticks = 0;
                if pct < CLEAR_PERCENT {
                    self.memory_raised = false;
                }
            }
        }
    }
}

/// Announces each new release once.
async fn check_release(state: &AppState) {
    let check = api::latest_version(state).await;
    let Some(latest) = check.latest.filter(|_| check.update_available) else {
        return;
    };
    if state
        .db
        .meta(NOTIFIED_RELEASE)
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some(&latest)
    {
        return;
    }
    state.notify(
        Event::new(NotifyEvent::UpdateAvailable)
            .about(crate::host::hostname(), "release")
            .detail(format!(
                "{latest} is out; this daemon runs {}",
                check.current
            ))
            .hint(format!("{} update", brand::CLI_BIN)),
    );
    let _ = state.db.set_meta(NOTIFIED_RELEASE, &latest).await;
}

/// Probes every storage connection; a failing one emits [`NotifyEvent::StorageUnreachable`].
async fn check_storage(state: &AppState) {
    for conn in state.db.storages().await.unwrap_or_default() {
        api::run_storage_test(state, conn).await;
    }
}

/// What a TLS probe found.
enum Probe {
    Valid(Cert),
    /// A certificate that does not verify: expired, self-signed or for another name.
    Invalid(Option<Cert>, String),
    /// No TLS answer at all (DNS elsewhere, nothing listening): not a certificate problem.
    Unreachable,
}

struct Cert {
    serial: String,
    not_before: DateTime<Utc>,
    not_after: DateTime<Utc>,
}

fn parse_cert(der: &[u8]) -> Option<Cert> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
    let validity = cert.validity();
    Some(Cert {
        serial: cert.raw_serial_as_string(),
        not_before: DateTime::from_timestamp(validity.not_before.timestamp(), 0)?,
        not_after: DateTime::from_timestamp(validity.not_after.timestamp(), 0)?,
    })
}

async fn fetch_cert(client: &reqwest::Client, host: &str) -> Result<Option<Cert>, String> {
    let res = client
        .head(format!("https://{host}/"))
        .send()
        .await
        .map_err(|e| format!("{:#}", anyhow::Error::from(e.without_url())))?;
    Ok(res
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|info| info.peer_certificate())
        .and_then(parse_cert))
}

async fn probe(host: &str) -> Probe {
    let client = |strict: bool| {
        reqwest::Client::builder()
            .tls_info(true)
            .danger_accept_invalid_certs(!strict)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
    };
    let (Ok(strict), Ok(lax)) = (client(true), client(false)) else {
        return Probe::Unreachable;
    };
    match fetch_cert(&strict, host).await {
        Ok(Some(cert)) => Probe::Valid(cert),
        Ok(None) => Probe::Unreachable,
        Err(strict_error) => match fetch_cert(&lax, host).await {
            Ok(cert) => Probe::Invalid(cert, strict_error),
            Err(_) => Probe::Unreachable,
        },
    }
}

/// Probes HTTPS domains: all of them on a slow tick, else only those without a certificate.
async fn check_certificates(state: &AppState, all: bool) {
    for domain in state.db.tls_domains().await.unwrap_or_default() {
        let record = state.db.tls_certificate(&domain.host).await.ok().flatten();
        if !all && record.is_some() {
            continue;
        }
        let probe = probe(&domain.host).await;
        if let Some(record) = certificate_events(state, &domain, record, probe).await
            && let Err(e) = state.db.save_tls_certificate(&record).await
        {
            tracing::warn!(host = %domain.host, "cannot record the certificate: {e:#}");
        }
    }
}

fn days(d: chrono::TimeDelta) -> i64 {
    d.num_hours().div_euclid(24)
}

async fn certificate_events(
    state: &AppState,
    domain: &Domain,
    record: Option<TlsRecord>,
    probe: Probe,
) -> Option<TlsRecord> {
    let now = Utc::now();
    let service = state.db.service(&domain.service_id).await.ok().flatten()?;
    let event = |kind| {
        let mut e = Event::new(kind);
        e.key = domain.host.clone();
        e.link = Some(format!("https://{}", domain.host));
        e.field("Domain", domain.host.clone())
    };
    match probe {
        Probe::Unreachable => None,
        Probe::Invalid(cert, error) => {
            let mut record = record?;
            if !record.invalid_reported {
                let detail = match &cert {
                    Some(c) if c.not_after < now => {
                        format!("expired {} days ago", days(now - c.not_after))
                    }
                    _ => error,
                };
                state
                    .notify_service(
                        &service.service,
                        event(NotifyEvent::TlsCertificateInvalid).detail(detail),
                    )
                    .await;
                record.invalid_reported = true;
            }
            Some(record)
        }
        Probe::Valid(cert) => {
            let renewed = record.as_ref().is_some_and(|r| r.serial != cert.serial);
            let first = record.is_none()
                && cert.not_before > domain.created_at - chrono::TimeDelta::hours(1)
                && now - cert.not_before < chrono::TimeDelta::days(7);
            if renewed || first {
                let verb = if renewed { "renewed" } else { "issued" };
                state
                    .notify_service(
                        &service.service,
                        event(NotifyEvent::TlsCertificateIssued).detail(format!(
                            "{verb}, valid until {}",
                            cert.not_after.format("%Y-%m-%d")
                        )),
                    )
                    .await;
            }
            let mut record = match record.filter(|r| r.serial == cert.serial) {
                Some(r) => r,
                None => TlsRecord {
                    host: domain.host.clone(),
                    serial: cert.serial.clone(),
                    not_before: cert.not_before,
                    not_after: cert.not_after,
                    expiry_warned: false,
                    invalid_reported: false,
                },
            };
            record.invalid_reported = false;
            let left = cert.not_after - now;
            let lifetime = cert.not_after - cert.not_before;
            // The proxy renews with a third of the lifetime left, so a quarter means renewal is
            // failing.
            let warn_at = (lifetime / 4).min(chrono::TimeDelta::days(14));
            if left < warn_at && !record.expiry_warned {
                state
                    .notify_service(
                        &service.service,
                        event(NotifyEvent::TlsExpiringSoon).detail(format!(
                            "expires in {} days ({}) and has not been renewed",
                            days(left),
                            cert.not_after.format("%Y-%m-%d")
                        )),
                    )
                    .await;
                record.expiry_warned = true;
            }
            Some(record)
        }
    }
}
