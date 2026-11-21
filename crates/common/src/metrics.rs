//! Service metrics: what `GET /services/{id}/metrics` returns.
//!
//! The daemon samples every running container every [`SAMPLE_SECS`] seconds and keeps raw
//! samples for an hour, then 1-minute rollups for a day, 10-minute rollups for a week and
//! 1-hour rollups for 30 days. Each range reads the finest level that still covers it.

use crate::{DeploymentStatus, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Seconds between two samples of a container.
pub const SAMPLE_SECS: i64 = 10;

/// How far back a metrics request looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum MetricsRange {
    #[default]
    #[serde(rename = "1h")]
    Hour,
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
}

impl MetricsRange {
    pub const ALL: [Self; 4] = [Self::Hour, Self::Day, Self::Week, Self::Month];

    /// The query value, e.g. `24h`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }

    pub fn secs(self) -> i64 {
        match self {
            Self::Hour => 3600,
            Self::Day => 86_400,
            Self::Week => 7 * 86_400,
            Self::Month => 30 * 86_400,
        }
    }

    /// Seconds each point covers: raw samples for an hour, then the rollup that keeps the
    /// range to at most about 1,500 points.
    pub fn step_secs(self) -> i64 {
        match self {
            Self::Hour => SAMPLE_SECS,
            Self::Day => 60,
            Self::Week => 600,
            Self::Month => 3600,
        }
    }
}

impl fmt::Display for MetricsRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl std::str::FromStr for MetricsRange {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|r| r.id() == s)
            .ok_or_else(|| format!("unknown range '{s}', expected one of 1h, 24h, 7d, 30d"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceMetrics {
    pub range: MetricsRange,
    /// Seconds each point covers.
    pub step_secs: i64,
    pub from: Timestamp,
    pub to: Timestamp,
    /// The compose services of a compose group, by name; empty for a single container.
    pub containers: Vec<String>,
    /// The compose service the points are for; `None` sums every container of the service.
    pub container: Option<String>,
    /// Indexes of the replicas that reported within the range.
    #[serde(default)]
    pub replicas: Vec<u16>,
    /// The replica the points are for; `None` sums every replica.
    #[serde(default)]
    pub replica: Option<u16>,
    /// Oldest first; buckets without samples are left out.
    pub points: Vec<MetricPoint>,
    /// Deployments that went live within the range, oldest first.
    pub deploys: Vec<DeployMarker>,
    /// Whether a domain routes to the service (or to `container`), so requests are measured.
    pub http: bool,
}

/// One bucket of samples. Averages are over the samples in the bucket; rates are per second.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricPoint {
    /// Start of the bucket.
    pub at: Timestamp,
    /// CPU in percent of one core, so a busy two-core container reads 200.
    pub cpu: f64,
    pub cpu_max: f64,
    /// Memory in use in bytes, excluding reclaimable page cache.
    pub mem: u64,
    pub mem_max: u64,
    /// The container's memory limit; the host's memory when it has none.
    pub mem_limit: u64,
    pub net_rx: f64,
    pub net_tx: f64,
    pub disk_read: f64,
    pub disk_write: f64,
    /// Most processes seen at once.
    pub pids: u64,
    /// Docker's restart count at the end of the bucket.
    pub restarts: i64,
    /// Seconds the container had been running at the end of the bucket.
    pub uptime_secs: i64,
    /// Requests through the proxy to the service's domains; `None` when none were measured.
    pub http: Option<HttpPoint>,
    /// Replicas that reported in the bucket; 0 for compose services.
    #[serde(default)]
    pub replicas: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpPoint {
    pub rps: f64,
    pub s2xx: u64,
    pub s3xx: u64,
    pub s4xx: u64,
    pub s5xx: u64,
    /// Latency percentiles in milliseconds; `None` without requests.
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
}

impl HttpPoint {
    pub fn requests(&self) -> u64 {
        self.s2xx + self.s3xx + self.s4xx + self.s5xx
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployMarker {
    /// When the deployment went live.
    pub at: Timestamp,
    pub number: i64,
    pub status: DeploymentStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_parse_and_pick_their_resolution() {
        for r in MetricsRange::ALL {
            assert_eq!(r.id().parse::<MetricsRange>(), Ok(r));
            assert_eq!(
                serde_json::to_string(&r).unwrap(),
                format!("\"{}\"", r.id())
            );
            assert!(r.secs() / r.step_secs() <= 1500, "{r} has too many points");
        }
        assert!("5m".parse::<MetricsRange>().is_err());
        assert_eq!(MetricsRange::default(), MetricsRange::Hour);
    }
}
