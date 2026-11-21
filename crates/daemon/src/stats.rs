//! Background sampler, every 10 seconds: host CPU, memory and disk for the top bar, every running
//! container of every service into the metrics store (see [`crate::metrics`]), the proxy's
//! per-domain request counters, crash detection and the per-service memory event.

use crate::db::ServiceRecord;
use crate::docker::{self, RunState};
use crate::metrics::http::{self, HostTotals};
use crate::metrics::{self, HttpSample, Rates, Sample};
use crate::notify::Event;
use crate::replicas;
use crate::state::AppState;
use bollard::Docker;
use bollard::models::{ContainerInspectResponse, ContainerStatsResponse};
use bollard::query_parameters::StatsOptions;
use chrono::Utc;
use common::metrics::SAMPLE_SECS;
use common::notify::NotifyEvent;
use common::{ActivityLevel, Domain, Id, ServiceStats, brand};
use futures_util::StreamExt as _;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use sysinfo::{Disks, System};

pub const INTERVAL: Duration = Duration::from_secs(SAMPLE_SECS.unsigned_abs());
/// An hour of host CPU samples.
pub const HISTORY: usize = 360;
/// Servers sampled at once.
const PARALLEL_SERVERS: usize = 4;
/// Old metrics are dropped every this many ticks.
const EXPIRE_EVERY: u64 = 30;
/// A container that was not sampled for this long is forgotten.
const FORGET_AFTER: Duration = Duration::from_secs(300);

/// Memory above this share of a container's limit raises `service.memory_high`; it is armed
/// again below the lower one.
const MEMORY_HIGH_PERCENT: f64 = 90.0;
const MEMORY_CLEAR_PERCENT: f64 = 85.0;
/// Memory must stay high for this many samples: three minutes.
const MEMORY_HIGH_SAMPLES: u32 = 18;

#[derive(Debug, Clone, Default)]
pub struct HostSample {
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub cpu_history: VecDeque<f32>,
}

/// Cumulative counters of a container at one sample.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Counters {
    cpu_total: u64,
    cpu_system: u64,
    net_rx: u64,
    net_tx: u64,
    disk_read: u64,
    disk_write: u64,
}

/// The proxy's counters by host.
type HostMap = HashMap<String, HostTotals>;

/// What the sampler remembers of a container between samples.
#[derive(Debug, Clone)]
struct Tracked {
    counters: Counters,
    at: Instant,
    memory_high: u32,
    memory_raised: bool,
}

/// What the crash detector last saw of a container.
#[derive(Debug, Default, Clone)]
struct Seen {
    restart_count: i64,
    crash_reported: bool,
    /// When each restart within LOOP_WINDOW was seen.
    restarts: VecDeque<Instant>,
    loop_reported: bool,
}

/// This many restarts within LOOP_WINDOW is a restart loop.
const LOOP_RESTARTS: usize = 3;
const LOOP_WINDOW: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct Stats {
    host: Mutex<HostSample>,
    /// By container id.
    containers: Mutex<HashMap<String, Tracked>>,
    /// The proxy's totals per host at the last scrape, by server.
    proxies: Mutex<HashMap<Id, (Instant, HostMap)>>,
    seen: Mutex<HashMap<String, Seen>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn push<T>(buf: &mut VecDeque<T>, value: T) {
    if buf.len() == HISTORY {
        buf.pop_front();
    }
    buf.push_back(value);
}

impl Stats {
    pub fn host(&self) -> HostSample {
        lock(&self.host).clone()
    }

    /// Requests per host since the previous scrape of this server's proxy, and the seconds
    /// between the two; `None` on the first scrape.
    fn proxy_since(&self, server_id: &str, now: HostMap) -> Option<(f64, HostMap)> {
        let at = Instant::now();
        let (prev_at, prev) =
            lock(&self.proxies).insert(server_id.to_string(), (at, now.clone()))?;
        let secs = at.duration_since(prev_at).as_secs_f64();
        let deltas = now
            .iter()
            .filter_map(|(host, totals)| {
                let before = prev.get(host).cloned().unwrap_or_default();
                Some((host.clone(), totals.since(&before)?))
            })
            .collect();
        Some((secs, deltas))
    }

    /// Turns a stats reading into a sample, using the container's previous reading for CPU and
    /// I/O rates, and updates its memory alarm. Returns the sample and whether memory just
    /// crossed into `service.memory_high`.
    fn record(&self, container_id: &str, stats: &ContainerStatsResponse) -> (Sample, bool) {
        let now = counters(stats);
        let at = Instant::now();
        let mut containers = lock(&self.containers);
        let prev = containers.get(container_id).cloned();
        let (cpu, rates) = match &prev {
            Some(p) => (
                cpu_percent(&p.counters, &now, online_cpus(stats)),
                rates(&p.counters, &now, at.duration_since(p.at).as_secs_f64()),
            ),
            None => (0.0, None),
        };
        let (mem, mem_limit) = memory(stats);
        let mut tracked = prev.unwrap_or(Tracked {
            counters: now,
            at,
            memory_high: 0,
            memory_raised: false,
        });
        tracked.counters = now;
        tracked.at = at;
        let alarm = memory_alarm(&mut tracked, mem, mem_limit);
        containers.insert(container_id.to_string(), tracked);
        let sample = Sample {
            cpu,
            mem,
            mem_limit,
            rates,
            pids: stats
                .pids_stats
                .as_ref()
                .and_then(|p| p.current)
                .unwrap_or(0),
            ..Sample::default()
        };
        (sample, alarm)
    }

    fn forget_stale(&self) {
        lock(&self.containers).retain(|_, t| t.at.elapsed() < FORGET_AFTER);
    }
}

fn counters(stats: &ContainerStatsResponse) -> Counters {
    let cpu = stats.cpu_stats.as_ref();
    let (net_rx, net_tx) = stats
        .networks
        .iter()
        .flat_map(|n| n.values())
        .fold((0, 0), |(rx, tx), n| {
            (rx + n.rx_bytes.unwrap_or(0), tx + n.tx_bytes.unwrap_or(0))
        });
    let (disk_read, disk_write) = stats
        .blkio_stats
        .as_ref()
        .and_then(|b| b.io_service_bytes_recursive.as_ref())
        .into_iter()
        .flatten()
        .fold((0, 0), |(r, w), e| {
            let v = e.value.unwrap_or(0);
            match e.op.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("read") => (r + v, w),
                Some("write") => (r, w + v),
                _ => (r, w),
            }
        });
    Counters {
        cpu_total: cpu
            .and_then(|c| c.cpu_usage.as_ref())
            .and_then(|u| u.total_usage)
            .unwrap_or(0),
        cpu_system: cpu.and_then(|c| c.system_cpu_usage).unwrap_or(0),
        net_rx,
        net_tx,
        disk_read,
        disk_write,
    }
}

fn online_cpus(stats: &ContainerStatsResponse) -> u32 {
    stats
        .cpu_stats
        .as_ref()
        .and_then(|c| c.online_cpus)
        .unwrap_or(1)
        .max(1)
}

/// CPU between two readings in percent of one core, as `docker stats` reports it.
fn cpu_percent(prev: &Counters, now: &Counters, cpus: u32) -> f64 {
    if now.cpu_system <= prev.cpu_system || now.cpu_total < prev.cpu_total {
        return 0.0;
    }
    (now.cpu_total - prev.cpu_total) as f64 / (now.cpu_system - prev.cpu_system) as f64
        * f64::from(cpus)
        * 100.0
}

/// Network and disk bytes per second between two readings; `None` when a counter went
/// backwards (a restarted container) or no time passed.
fn rates(prev: &Counters, now: &Counters, secs: f64) -> Option<Rates> {
    if secs <= 0.0 {
        return None;
    }
    let rate = |a: u64, b: u64| Some(b.checked_sub(a)? as f64 / secs);
    Some(Rates {
        net_rx: rate(prev.net_rx, now.net_rx)?,
        net_tx: rate(prev.net_tx, now.net_tx)?,
        disk_read: rate(prev.disk_read, now.disk_read)?,
        disk_write: rate(prev.disk_write, now.disk_write)?,
    })
}

/// Counts samples above [`MEMORY_HIGH_PERCENT`]; true once, when enough came in a row.
fn memory_alarm(t: &mut Tracked, used: u64, limit: u64) -> bool {
    if limit == 0 {
        return false;
    }
    let pct = used as f64 / limit as f64 * 100.0;
    if pct >= MEMORY_HIGH_PERCENT {
        t.memory_high += 1;
        if t.memory_high >= MEMORY_HIGH_SAMPLES && !t.memory_raised {
            t.memory_raised = true;
            return true;
        }
    } else {
        t.memory_high = 0;
        if pct < MEMORY_CLEAR_PERCENT {
            t.memory_raised = false;
        }
    }
    false
}

/// Memory in use excluding reclaimable page cache, as `docker stats` reports it.
fn memory(stats: &ContainerStatsResponse) -> (u64, u64) {
    let Some(mem) = &stats.memory_stats else {
        return (0, 0);
    };
    let usage = mem.usage.unwrap_or(0);
    let cache = mem
        .stats
        .as_ref()
        .and_then(|s| {
            s.get("inactive_file")
                .or_else(|| s.get("total_inactive_file"))
                .copied()
        })
        .unwrap_or(0);
    (usage.saturating_sub(cache), mem.limit.unwrap_or(0))
}

/// One stats sample of a container, used when the store has no samples of the service yet.
pub async fn sample_now(docker: &Docker, container: &str) -> Option<ServiceStats> {
    let stats = one_shot(docker, container).await?;
    let (mem_used, mem_limit) = memory(&stats);
    Some(ServiceStats {
        cpu_percent: 0.0,
        mem_used,
        mem_limit,
        cpu_history: Vec::new(),
        mem_history: Vec::new(),
    })
}

async fn one_shot(docker: &Docker, container: &str) -> Option<ContainerStatsResponse> {
    let options = StatsOptions {
        stream: false,
        one_shot: true,
    };
    docker.stats(container, Some(options)).next().await?.ok()
}

pub fn spawn_sampler(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut n: u64 = 0;
        loop {
            ticker.tick().await;
            sample_host(&state, &mut sys);
            sample_services(&state).await;
            state.stats.forget_stale();
            n += 1;
            if n % EXPIRE_EVERY == 1
                && let Err(e) = state.db.expire_metrics(Utc::now()).await
            {
                tracing::warn!("cannot drop old metrics: {e:#}");
            }
        }
    });
}

fn sample_host(state: &AppState, sys: &mut System) {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let (disk_used, disk_total) = disk_usage(state.dirs.root());
    let mut host = lock(&state.stats.host);
    host.cpu_percent = sys.global_cpu_usage();
    host.mem_used = sys.used_memory();
    host.mem_total = sys.total_memory();
    host.disk_used = disk_used;
    host.disk_total = disk_total;
    let cpu = host.cpu_percent;
    push(&mut host.cpu_history, cpu);
}

/// Usage of the filesystem holding `path`: the disk with the longest matching mount point.
fn disk_usage(path: &Path) -> (u64, u64) {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let disks = Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map_or((0, 0), |d| {
            (
                d.total_space().saturating_sub(d.available_space()),
                d.total_space(),
            )
        })
}

async fn sample_services(state: &AppState) {
    let Ok(recs) = state.db.services().await else {
        return;
    };
    let mut by_server: HashMap<Id, Vec<ServiceRecord>> = HashMap::new();
    for rec in recs {
        by_server
            .entry(rec.service.server_id.clone())
            .or_default()
            .push(rec);
    }
    futures_util::stream::iter(by_server)
        .for_each_concurrent(PARALLEL_SERVERS, |(server_id, recs)| async move {
            sample_server(state, &server_id, recs).await;
        })
        .await;
}

async fn sample_server(state: &AppState, server_id: &str, recs: Vec<ServiceRecord>) {
    let Ok((_, docker)) = state.docker_for(server_id).await else {
        return;
    };
    let mut domains: HashMap<Id, Vec<Domain>> = HashMap::new();
    for rec in &recs {
        if rec.live_container.is_some() {
            let list = state.db.domains(&rec.service.id).await.unwrap_or_default();
            domains.insert(rec.service.id.clone(), list);
        }
    }
    let requests = if domains.values().any(|d| !d.is_empty()) {
        match http::scrape(&docker).await {
            Ok(totals) => state.stats.proxy_since(server_id, totals),
            Err(e) => {
                tracing::debug!("no request metrics from the proxy on {server_id}: {e:#}");
                None
            }
        }
    } else {
        None
    };
    for rec in recs {
        let Some(live) = rec.live_container.clone() else {
            continue;
        };
        let service_domains = domains.remove(&rec.service.id).unwrap_or_default();
        sample_service(
            state,
            &docker,
            &rec,
            &live,
            &service_domains,
            requests.as_ref(),
        )
        .await;
    }
}

/// The containers of a service by the name metrics keep them under: the compose service, or
/// the replica (`#0`, `#1`, …) of any other service.
async fn targets(docker: &Docker, rec: &ServiceRecord) -> Vec<(String, String)> {
    if !metrics::is_compose(&rec.service.source) {
        return rec
            .replica_containers()
            .into_iter()
            .map(|(index, id)| (replicas::metrics_key(index), id))
            .collect();
    }
    let label = format!(
        "com.docker.compose.project={}",
        crate::build::compose_project(&rec.service.id)
    );
    let Ok(list) = docker::list_by_label(docker, &label).await else {
        return Vec::new();
    };
    list.into_iter()
        .filter_map(|c| {
            let name = c
                .labels
                .as_ref()?
                .get("com.docker.compose.service")?
                .clone();
            Some((name, c.id?))
        })
        .collect()
}

/// Requests to the domains that route to one container of a service.
fn requests_for(
    domains: &[Domain],
    container: &str,
    compose: bool,
    requests: Option<&(f64, HostMap)>,
) -> Option<HttpSample> {
    let (secs, by_host) = requests?;
    let mut sum: Option<HostTotals> = None;
    for d in domains {
        if compose && d.compose_service.as_deref() != Some(container) {
            continue;
        }
        let totals = by_host
            .get(&d.host.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default();
        sum.get_or_insert_with(HostTotals::default).add(&totals);
    }
    Some(sum?.sample(*secs))
}

async fn sample_service(
    state: &AppState,
    docker: &Docker,
    rec: &ServiceRecord,
    live: &str,
    domains: &[Domain],
    requests: Option<&(f64, HostMap)>,
) {
    let compose = metrics::is_compose(&rec.service.source);
    let now = Utc::now();
    let mut watched = Vec::new();
    let mut requests_left = requests;
    for (name, id) in targets(docker, rec).await {
        let Ok(Some(inspected)) = docker::inspect(docker, &id).await else {
            continue;
        };
        if RunState::from_inspect(&inspected) == RunState::Running
            && let Some(stats) = one_shot(docker, &id).await
        {
            let (mut sample, alarm) = state.stats.record(&id, &stats);
            let info = docker::container_info(&inspected);
            sample.restarts = info.restart_count;
            sample.uptime_secs = info
                .started_at
                .map_or(0, |t| (now - t).num_seconds().max(0));
            // A replica's requests can't be told apart, so the first one carries the service's.
            sample.http = if compose {
                requests_for(domains, &name, true, requests)
            } else {
                requests_for(domains, &name, false, requests_left.take())
            };
            if let Err(e) = state
                .db
                .insert_metric_sample(&rec.service.id, &name, now, &sample)
                .await
            {
                tracing::warn!("cannot store metrics of {}: {e:#}", rec.service.name);
            }
            if alarm {
                memory_event(state, rec, &name, &sample).await;
            }
        }
        if !compose || id == live || inspected.id.as_deref() == Some(live) {
            watched.push((id, inspected));
        }
    }
    if rec.busy.is_some() {
        return;
    }
    if watched.is_empty()
        && let Ok(Some(i)) = docker::inspect(docker, live).await
    {
        watched.push((live.to_string(), i));
    }
    for (id, inspected) in watched {
        detect_crash(state, rec, &id, &inspected).await;
    }
}

async fn memory_event(state: &AppState, rec: &ServiceRecord, container: &str, s: &Sample) {
    let pct = s.mem as f64 / s.mem_limit as f64 * 100.0;
    let mib = |b: u64| b as f64 / f64::from(1 << 20);
    let what = if container.is_empty() {
        String::new()
    } else {
        format!("{container}: ")
    };
    let mut event = Event::new(NotifyEvent::ServiceMemoryHigh)
        .detail(format!(
            "{what}{pct:.0}% of its memory limit: {:.0} of {:.0} MiB",
            mib(s.mem),
            mib(s.mem_limit)
        ))
        .hint(format!(
            "{} service metrics {}",
            brand::CLI_BIN,
            rec.service.name
        ));
    event.key = format!("{}/{container}", rec.service.id);
    state.notify_service(&rec.service, event).await;
}

async fn detect_crash(
    state: &AppState,
    rec: &ServiceRecord,
    container: &str,
    inspected: &ContainerInspectResponse,
) {
    if rec.stopped {
        return;
    }
    let run = RunState::from_inspect(inspected);
    let restart_count = inspected.restart_count.unwrap_or(0);
    let exit_code = inspected
        .state
        .as_ref()
        .and_then(|s| s.exit_code)
        .unwrap_or(0);
    let oom = inspected
        .state
        .as_ref()
        .and_then(|s| s.oom_killed)
        .unwrap_or(false);
    let previous = lock(&state.stats.seen).get(container).cloned();
    let known = previous.is_some();
    let mut seen = previous.unwrap_or(Seen {
        restart_count,
        ..Seen::default()
    });
    let crash_event = |detail: String| {
        let kind = if oom {
            NotifyEvent::ServiceOomKilled
        } else {
            NotifyEvent::ServiceCrashed
        };
        Event::new(kind).detail(detail).hint(format!(
            "{} logs {}",
            brand::CLI_BIN,
            rec.service.name
        ))
    };
    if let RunState::Exited { code: Some(code) } = run
        && code != 0
        && !seen.crash_reported
    {
        state
            .service_activity(
                &rec.service,
                ActivityLevel::Error,
                "Container crashed".into(),
                Some(format!("exit code {code}")),
            )
            .await;
        state
            .notify_service(&rec.service, crash_event(format!("exit code {code}")))
            .await;
        seen.crash_reported = true;
    } else if known && restart_count > seen.restart_count {
        state
            .service_activity(
                &rec.service,
                ActivityLevel::Error,
                "Container restarted after a crash".into(),
                Some(format!("exit code {exit_code}, restart #{restart_count}")),
            )
            .await;
        let now = Instant::now();
        seen.restarts.push_back(now);
        while seen
            .restarts
            .front()
            .is_some_and(|t| now.duration_since(*t) > LOOP_WINDOW)
        {
            seen.restarts.pop_front();
        }
        let event = if seen.restarts.len() >= LOOP_RESTARTS {
            (!seen.loop_reported).then(|| {
                Event::new(NotifyEvent::ServiceRestartLoop)
                    .detail(format!(
                        "{} restarts in {} minutes",
                        seen.restarts.len(),
                        LOOP_WINDOW.as_secs() / 60
                    ))
                    .hint(format!("{} logs {}", brand::CLI_BIN, rec.service.name))
            })
        } else {
            Some(crash_event(format!(
                "the container exited and Docker restarted it (restart #{restart_count})"
            )))
        };
        if let Some(event) = event {
            seen.loop_reported |= event.kind == NotifyEvent::ServiceRestartLoop;
            state.notify_service(&rec.service, event).await;
        }
    }
    if seen
        .restarts
        .back()
        .is_some_and(|t| t.elapsed() > LOOP_WINDOW)
    {
        seen.restarts.clear();
        seen.loop_reported = false;
    }
    if run == RunState::Running {
        seen.crash_reported = false;
    }
    seen.restart_count = restart_count;
    lock(&state.stats.seen).insert(container.to_string(), seen);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{
        ContainerBlkioStatEntry, ContainerBlkioStats, ContainerCpuStats, ContainerCpuUsage,
        ContainerMemoryStats, ContainerNetworkStats, ContainerPidsStats,
    };

    fn sample(total: u64, system: u64, mem: u64, io: u64) -> ContainerStatsResponse {
        ContainerStatsResponse {
            cpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(total),
                    ..Default::default()
                }),
                system_cpu_usage: Some(system),
                online_cpus: Some(2),
                ..Default::default()
            }),
            memory_stats: Some(ContainerMemoryStats {
                usage: Some(mem),
                limit: Some(1000),
                stats: Some(HashMap::from([("inactive_file".to_string(), 10)])),
                ..Default::default()
            }),
            networks: Some(HashMap::from([
                (
                    "eth0".to_string(),
                    ContainerNetworkStats {
                        rx_bytes: Some(io),
                        tx_bytes: Some(io / 2),
                        ..Default::default()
                    },
                ),
                (
                    "eth1".to_string(),
                    ContainerNetworkStats {
                        rx_bytes: Some(io),
                        ..Default::default()
                    },
                ),
            ])),
            blkio_stats: Some(ContainerBlkioStats {
                io_service_bytes_recursive: Some(vec![
                    ContainerBlkioStatEntry {
                        op: Some("read".into()),
                        value: Some(io),
                        ..Default::default()
                    },
                    ContainerBlkioStatEntry {
                        op: Some("Write".into()),
                        value: Some(3 * io),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
            pids_stats: Some(ContainerPidsStats {
                current: Some(7),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn cpu_and_rates_come_from_deltas_between_samples() {
        let stats = Stats::default();
        let (first, _) = stats.record("c", &sample(100, 1000, 110, 1000));
        assert_eq!(first.cpu, 0.0);
        assert_eq!(first.rates, None);
        assert_eq!(first.pids, 7);
        let (second, _) = stats.record("c", &sample(200, 2000, 210, 2000));
        assert!((second.cpu - 20.0).abs() < 0.01);
        assert_eq!(second.mem, 200);
        assert_eq!(second.mem_limit, 1000);
        assert!(second.rates.unwrap().net_rx > 0.0);
    }

    #[test]
    fn counters_sum_networks_and_split_disk_io() {
        let c = counters(&sample(1, 2, 3, 100));
        assert_eq!((c.net_rx, c.net_tx), (200, 50));
        assert_eq!((c.disk_read, c.disk_write), (100, 300));
        let later = counters(&sample(1, 2, 3, 300));
        let r = rates(&c, &later, 2.0).unwrap();
        assert_eq!(r.net_rx, 200.0);
        assert_eq!(r.net_tx, 50.0);
        assert_eq!(r.disk_read, 100.0);
        assert_eq!(r.disk_write, 300.0);
        // A restarted container starts its counters over: no rate for that interval.
        assert_eq!(rates(&later, &c, 2.0), None);
    }

    #[test]
    fn memory_alarm_needs_a_sustained_high_and_rearms_below_the_clear_mark() {
        let mut t = Tracked {
            counters: Counters::default(),
            at: Instant::now(),
            memory_high: 0,
            memory_raised: false,
        };
        let fired =
            |t: &mut Tracked, used, n| (0..n).filter(|_| memory_alarm(t, used, 100)).count();
        assert_eq!(fired(&mut t, 95, MEMORY_HIGH_SAMPLES - 1), 0);
        assert_eq!(fired(&mut t, 95, 5), 1);
        // A dip that stays above the clear mark does not re-arm it.
        assert_eq!(fired(&mut t, 88, 1), 0);
        assert_eq!(fired(&mut t, 95, MEMORY_HIGH_SAMPLES), 0);
        assert_eq!(fired(&mut t, 50, 1), 0);
        assert_eq!(fired(&mut t, 95, MEMORY_HIGH_SAMPLES), 1);
        assert!(!memory_alarm(&mut t, 5, 0));
    }

    #[test]
    fn requests_follow_the_domains_of_a_container() {
        let domain = |host: &str, service: Option<&str>| Domain {
            id: "d".into(),
            service_id: "s".into(),
            host: host.into(),
            port: None,
            tls: true,
            created_at: Utc::now(),
            compose_service: service.map(str::to_string),
        };
        let totals = |n| HostTotals {
            classes: [n, 0, 0, 0],
            buckets: vec![(0.1, n), (f64::INFINITY, n)],
        };
        let by_host = (
            10.0,
            HashMap::from([
                ("shop.example.com".to_string(), totals(30)),
                ("api.example.com".to_string(), totals(5)),
            ]),
        );
        let domains = [
            domain("Shop.example.com", Some("web")),
            domain("api.example.com", Some("api")),
            domain("quiet.example.com", Some("web")),
        ];
        let web = requests_for(&domains, "web", true, Some(&by_host)).unwrap();
        assert_eq!(web.classes, [30, 0, 0, 0]);
        assert_eq!(web.secs, 10.0);
        let all = requests_for(&domains, "", false, Some(&by_host)).unwrap();
        assert_eq!(all.classes, [35, 0, 0, 0]);
        assert_eq!(requests_for(&domains, "worker", true, Some(&by_host)), None);
        assert_eq!(requests_for(&domains, "web", true, None), None);
    }

    /// Samples a real container through the Docker API: two readings give CPU, I/O rates and
    /// processes. Skips without Docker unless `<ENV_PREFIX>_TEST_REQUIRE_DOCKER` is set.
    #[tokio::test]
    async fn samples_a_real_container() {
        let docker = match Docker::connect_with_defaults() {
            Ok(d) if d.ping().await.is_ok() => d,
            _ => {
                let require = brand::env_var("TEST_REQUIRE_DOCKER");
                assert!(
                    std::env::var_os(&require).is_none(),
                    "{require} is set but Docker is unavailable"
                );
                eprintln!("skipping: Docker is unavailable");
                return;
            }
        };
        let name = format!("{}-test-stats-{}", brand::PREFIX, std::process::id());
        let out = std::process::Command::new("docker")
            .args(["run", "-d", "--rm", "--name", &name, "busybox:1.36"])
            .args(["sh", "-c", "while true; do echo x > /tmp/x; done"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stats = Stats::default();
        let first = one_shot(&docker, &name).await.expect("first reading");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let second = one_shot(&docker, &name).await.expect("second reading");
        let (_, _) = stats.record(&name, &first);
        let (sample, alarm) = stats.record(&name, &second);
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &name])
            .output();
        assert!(sample.cpu > 1.0, "a busy loop uses CPU: {sample:?}");
        assert!(
            sample.mem > 0 && sample.mem_limit >= sample.mem,
            "{sample:?}"
        );
        assert!(sample.pids >= 1, "{sample:?}");
        let rates = sample.rates.expect("rates from the second reading");
        assert!(rates.net_rx >= 0.0 && rates.disk_write >= 0.0);
        assert!(!alarm);
    }

    #[test]
    fn history_is_bounded() {
        let mut buf = VecDeque::new();
        for i in 0..(HISTORY + 10) {
            push(&mut buf, i);
        }
        assert_eq!(buf.len(), HISTORY);
        assert_eq!(buf.front(), Some(&10));
    }
}
