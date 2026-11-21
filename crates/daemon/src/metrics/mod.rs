//! Service metrics: the samples the stats sampler takes, how they roll up, and how stored
//! buckets become the points of `GET /services/{id}/metrics`.
//!
//! Every sample is written once per resolution in [`LEVELS`]: as a raw row and folded into
//! the 1-minute, 10-minute and 1-hour bucket it falls in. Rows hold sums and maxima, so a bucket
//! is exact however many samples reached it, and [`expire`] only has to drop old rows.

pub mod http;

use crate::state::AppState;
use chrono::{DateTime, TimeDelta};
use common::metrics::{
    DeployMarker, HttpPoint, MetricPoint, MetricsRange, SAMPLE_SECS, ServiceMetrics,
};
use common::{BuildMethod, Source, Timestamp};
use std::collections::BTreeMap;

/// Each stored resolution in seconds and how long its rows are kept.
pub const LEVELS: [(i64, i64); 4] = [
    (SAMPLE_SECS, 3600),
    (60, 86_400),
    (600, 7 * 86_400),
    (3600, 30 * 86_400),
];

/// One sample of one container.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub cpu: f64,
    pub mem: u64,
    pub mem_limit: u64,
    /// Bytes per second since the previous sample of the same container.
    pub rates: Option<Rates>,
    pub pids: u64,
    pub restarts: i64,
    pub uptime_secs: i64,
    pub http: Option<HttpSample>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub net_rx: f64,
    pub net_tx: f64,
    pub disk_read: f64,
    pub disk_write: f64,
}

/// Requests to a container's domains between two scrapes of the proxy.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HttpSample {
    /// Seconds between the two scrapes.
    pub secs: f64,
    /// Responses by status class: 2xx, 3xx, 4xx, 5xx.
    pub classes: [u64; 4],
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
}

/// A stored bucket, as summed in the database.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bucket {
    pub container: String,
    pub at: Timestamp,
    pub samples: i64,
    pub cpu_sum: f64,
    pub cpu_max: f64,
    pub mem_sum: f64,
    pub mem_max: i64,
    pub mem_limit: i64,
    pub rate_samples: i64,
    pub net_rx_sum: f64,
    pub net_tx_sum: f64,
    pub disk_read_sum: f64,
    pub disk_write_sum: f64,
    pub pids_max: i64,
    pub restarts: i64,
    pub uptime: i64,
    pub http_secs: f64,
    pub classes: [i64; 4],
    pub latency_requests: i64,
    pub p50_wsum: f64,
    pub p95_wsum: f64,
}

/// Start of the bucket of `resolution` seconds that `at` falls in.
pub fn bucket_start(at: Timestamp, resolution: i64) -> Timestamp {
    let secs = at.timestamp().div_euclid(resolution) * resolution;
    DateTime::from_timestamp(secs, 0).unwrap_or(at)
}

/// The buckets a sample taken at `at` is folded into, one per level.
pub fn buckets_of(at: Timestamp) -> [(i64, Timestamp); 4] {
    LEVELS.map(|(resolution, _)| (resolution, bucket_start(at, resolution)))
}

/// For each level, the oldest bucket start that is still kept at `now`.
pub fn cutoffs(now: Timestamp) -> [(i64, Timestamp); 4] {
    LEVELS.map(|(resolution, keep)| (resolution, now - TimeDelta::seconds(keep)))
}

fn ratio(sum: f64, n: i64) -> f64 {
    if n > 0 { sum / n as f64 } else { 0.0 }
}

fn unsigned(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

impl Bucket {
    pub fn point(&self) -> MetricPoint {
        let requests: i64 = self.classes.iter().sum();
        let http = (self.http_secs > 0.0).then(|| HttpPoint {
            rps: requests as f64 / self.http_secs,
            s2xx: unsigned(self.classes[0]),
            s3xx: unsigned(self.classes[1]),
            s4xx: unsigned(self.classes[2]),
            s5xx: unsigned(self.classes[3]),
            p50_ms: (self.latency_requests > 0)
                .then(|| ratio(self.p50_wsum, self.latency_requests)),
            p95_ms: (self.latency_requests > 0)
                .then(|| ratio(self.p95_wsum, self.latency_requests)),
        });
        MetricPoint {
            at: self.at,
            cpu: ratio(self.cpu_sum, self.samples),
            cpu_max: self.cpu_max,
            mem: ratio(self.mem_sum, self.samples).round() as u64,
            mem_max: unsigned(self.mem_max),
            mem_limit: unsigned(self.mem_limit),
            net_rx: ratio(self.net_rx_sum, self.rate_samples),
            net_tx: ratio(self.net_tx_sum, self.rate_samples),
            disk_read: ratio(self.disk_read_sum, self.rate_samples),
            disk_write: ratio(self.disk_write_sum, self.rate_samples),
            pids: unsigned(self.pids_max),
            restarts: self.restarts,
            uptime_secs: self.uptime,
            http,
        }
    }
}

/// The whole service at each bucket: usage adds up across its containers, the limit is the
/// largest one, and uptime is the youngest container's.
pub fn combine(buckets: &[Bucket]) -> Vec<MetricPoint> {
    let mut by_time: BTreeMap<Timestamp, MetricPoint> = BTreeMap::new();
    for b in buckets {
        let p = b.point();
        let Some(sum) = by_time.get_mut(&p.at) else {
            by_time.insert(p.at, p);
            continue;
        };
        sum.cpu += p.cpu;
        sum.cpu_max += p.cpu_max;
        sum.mem += p.mem;
        sum.mem_max += p.mem_max;
        sum.mem_limit = sum.mem_limit.max(p.mem_limit);
        sum.net_rx += p.net_rx;
        sum.net_tx += p.net_tx;
        sum.disk_read += p.disk_read;
        sum.disk_write += p.disk_write;
        sum.pids += p.pids;
        sum.restarts += p.restarts;
        sum.uptime_secs = sum.uptime_secs.min(p.uptime_secs);
        sum.http = match (sum.http.take(), p.http) {
            (Some(a), Some(b)) => Some(merge_http(a, b)),
            (a, b) => a.or(b),
        };
    }
    by_time.into_values().collect()
}

fn merge_http(a: HttpPoint, b: HttpPoint) -> HttpPoint {
    let (wa, wb) = (a.requests() as f64, b.requests() as f64);
    let weighted = |x: Option<f64>, y: Option<f64>| match (x, y) {
        (Some(x), Some(y)) if wa + wb > 0.0 => Some((x * wa + y * wb) / (wa + wb)),
        (x, y) => x.or(y),
    };
    HttpPoint {
        rps: a.rps + b.rps,
        s2xx: a.s2xx + b.s2xx,
        s3xx: a.s3xx + b.s3xx,
        s4xx: a.s4xx + b.s4xx,
        s5xx: a.s5xx + b.s5xx,
        p50_ms: weighted(a.p50_ms, b.p50_ms),
        p95_ms: weighted(a.p95_ms, b.p95_ms),
    }
}

/// Whether a service deploys as a compose group, so its metrics are kept per container.
pub fn is_compose(source: &Source) -> bool {
    matches!(source, Source::Git { build, .. } if build.method == BuildMethod::Compose)
}

/// Everything `GET /services/{id}/metrics` returns, as of `now`.
pub async fn service_metrics(
    state: &AppState,
    service_id: &str,
    range: MetricsRange,
    container: Option<&str>,
    now: Timestamp,
) -> anyhow::Result<ServiceMetrics> {
    let step = range.step_secs();
    let from = bucket_start(now - TimeDelta::seconds(range.secs()), step);
    let buckets = state
        .db
        .metric_buckets(service_id, step, from, container)
        .await?;
    let mut containers = state.db.metric_containers(service_id, step, from).await?;
    containers.retain(|c| !c.is_empty());
    let domains = state.db.domains(service_id).await?;
    let http = domains
        .iter()
        .any(|d| container.is_none() || d.compose_service.as_deref() == container);
    let deploys: Vec<DeployMarker> = state.db.deploy_markers(service_id, from).await?;
    Ok(ServiceMetrics {
        range,
        step_secs: step,
        from,
        to: now,
        containers,
        container: container.map(str::to_string),
        points: combine(&buckets),
        deploys,
        http,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Timestamp {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn samples_fold_into_one_bucket_per_level() {
        // 2023-11-14 22:00:00 UTC.
        let hour = 1_699_999_200;
        let t = at(hour + 17 * 60 + 43);
        let b = buckets_of(t);
        assert_eq!(b[0], (10, at(hour + 17 * 60 + 40)));
        assert_eq!(b[1], (60, at(hour + 17 * 60)));
        assert_eq!(b[2], (600, at(hour + 10 * 60)));
        assert_eq!(b[3], (3600, at(hour)));
        for (res, start) in b {
            assert_eq!(start.timestamp() % res, 0);
            assert!(start <= t && t < start + TimeDelta::seconds(res));
        }
    }

    #[test]
    fn retention_keeps_each_level_for_its_window() {
        let now = at(10_000_000);
        let c = cutoffs(now);
        assert_eq!(c[0], (10, now - TimeDelta::hours(1)));
        assert_eq!(c[1], (60, now - TimeDelta::days(1)));
        assert_eq!(c[2], (600, now - TimeDelta::days(7)));
        assert_eq!(c[3], (3600, now - TimeDelta::days(30)));
        // Each range reads a level that still holds all of it.
        for r in MetricsRange::ALL {
            let (_, keep) = LEVELS
                .iter()
                .find(|(res, _)| *res == r.step_secs())
                .unwrap();
            assert!(*keep >= r.secs(), "{r}");
        }
    }

    fn bucket(container: &str, secs: i64) -> Bucket {
        Bucket {
            container: container.into(),
            at: at(secs),
            samples: 6,
            cpu_sum: 60.0,
            cpu_max: 30.0,
            mem_sum: 600.0,
            mem_max: 150,
            mem_limit: 1000,
            rate_samples: 5,
            net_rx_sum: 50.0,
            net_tx_sum: 25.0,
            disk_read_sum: 0.0,
            disk_write_sum: 10.0,
            pids_max: 4,
            restarts: 1,
            uptime: 300,
            ..Default::default()
        }
    }

    #[test]
    fn a_bucket_averages_over_its_samples() {
        let mut b = bucket("", 60);
        b.http_secs = 60.0;
        b.classes = [100, 10, 8, 2];
        b.latency_requests = 120;
        b.p50_wsum = 120.0 * 20.0;
        b.p95_wsum = 120.0 * 90.0;
        let p = b.point();
        assert_eq!(p.cpu, 10.0);
        assert_eq!(p.cpu_max, 30.0);
        assert_eq!(p.mem, 100);
        assert_eq!(p.mem_max, 150);
        assert_eq!(p.net_rx, 10.0);
        assert_eq!(p.disk_write, 2.0);
        let http = p.http.unwrap();
        assert_eq!(http.rps, 2.0);
        assert_eq!(http.requests(), 120);
        assert_eq!(http.p50_ms, Some(20.0));
        assert_eq!(http.p95_ms, Some(90.0));

        let quiet = bucket("", 60).point();
        assert_eq!(quiet.http, None);
    }

    #[test]
    fn containers_add_up_per_bucket() {
        let mut worker = bucket("worker", 60);
        worker.mem_limit = 4000;
        worker.uptime = 20;
        let points = combine(&[bucket("web", 60), worker, bucket("web", 120)]);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].at, at(60));
        assert_eq!(points[0].cpu, 20.0);
        assert_eq!(points[0].mem, 200);
        assert_eq!(points[0].mem_limit, 4000);
        assert_eq!(points[0].uptime_secs, 20);
        assert_eq!(points[0].restarts, 2);
        assert_eq!(points[1].cpu, 10.0);
    }

    #[test]
    fn latency_merges_weighted_by_requests() {
        let a = HttpPoint {
            rps: 1.0,
            s2xx: 30,
            s3xx: 0,
            s4xx: 0,
            s5xx: 0,
            p50_ms: Some(10.0),
            p95_ms: Some(100.0),
        };
        let b = HttpPoint {
            rps: 3.0,
            s2xx: 90,
            s3xx: 0,
            s4xx: 0,
            s5xx: 0,
            p50_ms: Some(30.0),
            p95_ms: None,
        };
        let m = merge_http(a, b);
        assert_eq!(m.rps, 4.0);
        assert_eq!(m.s2xx, 120);
        assert_eq!(m.p50_ms, Some(25.0));
        assert_eq!(m.p95_ms, Some(100.0));
    }
}
