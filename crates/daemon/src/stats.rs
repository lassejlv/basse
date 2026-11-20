//! Background sampler: host CPU, memory and disk, per-service container stats and crash
//! detection, kept in fixed-size in-memory ring buffers.

use crate::docker::{self, RunState};
use crate::state::AppState;
use bollard::Docker;
use bollard::models::ContainerStatsResponse;
use bollard::query_parameters::StatsOptions;
use common::{ActivityLevel, Id, ServiceStats};
use futures_util::StreamExt as _;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use sysinfo::{Disks, System};

/// One sample every 15 seconds, 240 samples: an hour of history.
pub const INTERVAL: Duration = Duration::from_secs(15);
pub const HISTORY: usize = 240;

#[derive(Debug, Clone, Default)]
pub struct HostSample {
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub cpu_history: VecDeque<f32>,
}

#[derive(Debug, Default)]
struct Series {
    container: String,
    cpu: VecDeque<f32>,
    mem: VecDeque<u64>,
    mem_limit: u64,
    /// Container and system CPU counters of the previous sample.
    prev_cpu: Option<(u64, u64)>,
}

/// What the crash detector last saw of a container.
#[derive(Debug, Default, Clone, Copy)]
struct Seen {
    restart_count: i64,
    crash_reported: bool,
}

#[derive(Default)]
pub struct Stats {
    host: Mutex<HostSample>,
    services: Mutex<HashMap<Id, Series>>,
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

    pub fn service(&self, service_id: &str) -> Option<ServiceStats> {
        let services = lock(&self.services);
        let s = services.get(service_id)?;
        Some(ServiceStats {
            cpu_percent: s.cpu.back().copied().unwrap_or(0.0),
            mem_used: s.mem.back().copied().unwrap_or(0),
            mem_limit: s.mem_limit,
            cpu_history: s.cpu.iter().copied().collect(),
            mem_history: s.mem.iter().copied().collect(),
        })
    }

    pub fn forget_service(&self, service_id: &str) {
        lock(&self.services).remove(service_id);
    }

    fn record_service(&self, service_id: &str, container: &str, stats: &ContainerStatsResponse) {
        let mut services = lock(&self.services);
        let series = services.entry(service_id.to_string()).or_default();
        if series.container != container {
            *series = Series {
                container: container.to_string(),
                ..Default::default()
            };
        }
        let cpu = stats.cpu_stats.as_ref();
        let total = cpu
            .and_then(|c| c.cpu_usage.as_ref())
            .and_then(|u| u.total_usage)
            .unwrap_or(0);
        let system = cpu.and_then(|c| c.system_cpu_usage).unwrap_or(0);
        let cpus = cpu.and_then(|c| c.online_cpus).unwrap_or(1).max(1);
        let percent = match series.prev_cpu {
            Some((prev_total, prev_system)) if system > prev_system && total >= prev_total => {
                ((total - prev_total) as f64 / (system - prev_system) as f64
                    * f64::from(cpus)
                    * 100.0) as f32
            }
            _ => 0.0,
        };
        series.prev_cpu = Some((total, system));
        let (used, limit) = memory(stats);
        series.mem_limit = limit;
        push(&mut series.cpu, percent);
        push(&mut series.mem, used);
    }
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

/// One stats sample of a container, used when the sampler has not seen the service yet.
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
        loop {
            ticker.tick().await;
            sample_host(&state, &mut sys);
            sample_services(&state).await;
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
    let Ok(recs) = state.db.services() else {
        return;
    };
    for rec in recs {
        let (Some(container), None) = (&rec.live_container, rec.busy) else {
            continue;
        };
        let Ok((_, docker)) = state.docker_for(&rec.service.server_id).await else {
            continue;
        };
        let Ok(Some(inspected)) = docker::inspect(&docker, container).await else {
            continue;
        };
        let run = RunState::from_inspect(&inspected);
        if run == RunState::Running
            && let Some(stats) = one_shot(&docker, container).await
        {
            state
                .stats
                .record_service(&rec.service.id, container, &stats);
        }
        if rec.stopped {
            continue;
        }
        let restart_count = inspected.restart_count.unwrap_or(0);
        let exit_code = inspected
            .state
            .as_ref()
            .and_then(|s| s.exit_code)
            .unwrap_or(0);
        let previous = lock(&state.stats.seen).get(container).copied();
        let mut seen = previous.unwrap_or(Seen {
            restart_count,
            crash_reported: false,
        });
        if let RunState::Exited { code: Some(code) } = run
            && code != 0
            && !seen.crash_reported
        {
            state.service_activity(
                &rec.service,
                ActivityLevel::Error,
                "Container crashed".into(),
                Some(format!("exit code {code}")),
            );
            seen.crash_reported = true;
        } else if previous.is_some() && restart_count > seen.restart_count {
            state.service_activity(
                &rec.service,
                ActivityLevel::Error,
                "Container restarted after a crash".into(),
                Some(format!("exit code {exit_code}, restart #{restart_count}")),
            );
        }
        if run == RunState::Running {
            seen.crash_reported = false;
        }
        seen.restart_count = restart_count;
        lock(&state.stats.seen).insert(container.clone(), seen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats};

    fn sample(total: u64, system: u64, mem: u64) -> ContainerStatsResponse {
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
            ..Default::default()
        }
    }

    #[test]
    fn cpu_percent_comes_from_deltas_between_samples() {
        let stats = Stats::default();
        stats.record_service("s", "c", &sample(100, 1000, 110));
        stats.record_service("s", "c", &sample(200, 2000, 210));
        let s = stats.service("s").unwrap();
        assert_eq!(s.cpu_history.len(), 2);
        assert!((s.cpu_percent - 20.0).abs() < 0.01);
        assert_eq!(s.mem_used, 200);
        assert_eq!(s.mem_limit, 1000);
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
