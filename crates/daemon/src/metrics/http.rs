//! Requests per domain from the proxy's Prometheus metrics.
//!
//! The proxy runs with Caddy's `metrics { per_host }` global option, so its request counters and
//! latency histogram carry a `host` label. The sampler reads them through the server's Docker
//! connection (an exec of `wget` against the admin endpoint, which only listens inside the
//! container), keeps the previous totals per host, and turns the difference into an
//! [`HttpSample`] for each container that domain routes to.

use super::HttpSample;
use crate::docker;
use anyhow::{Context as _, Result};
use bollard::Docker;
use common::brand;
use std::collections::HashMap;

/// Where Caddy's admin endpoint serves metrics inside the proxy container.
const METRICS_URL: &str = "http://127.0.0.1:2019/metrics";
const REQUESTS: &str = "caddy_http_request_duration_seconds_count";
const BUCKETS: &str = "caddy_http_request_duration_seconds_bucket";

/// Counters of one host since the proxy started, or the difference between two readings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostTotals {
    /// Responses by status class: 2xx, 3xx, 4xx, 5xx.
    pub classes: [u64; 4],
    /// The cumulative latency histogram: upper bound in seconds and count, by bound.
    pub buckets: Vec<(f64, u64)>,
}

impl HostTotals {
    fn add_bucket(&mut self, le: f64, n: u64) {
        match self.buckets.iter_mut().find(|(b, _)| *b == le) {
            Some((_, count)) => *count += n,
            None => self.buckets.push((le, n)),
        }
    }

    /// Adds another host's counters, e.g. a second domain of the same container.
    pub fn add(&mut self, other: &HostTotals) {
        for (sum, n) in self.classes.iter_mut().zip(other.classes) {
            *sum += n;
        }
        for &(le, n) in &other.buckets {
            self.add_bucket(le, n);
        }
        self.buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    /// What happened since `prev`; `None` when a counter went backwards because the
    /// proxy restarted, as the gap cannot be told apart from the new requests.
    pub fn since(&self, prev: &HostTotals) -> Option<HostTotals> {
        let mut classes = [0; 4];
        for (i, class) in classes.iter_mut().enumerate() {
            *class = self.classes[i].checked_sub(prev.classes[i])?;
        }
        let mut buckets = Vec::with_capacity(self.buckets.len());
        for &(le, n) in &self.buckets {
            let before = prev
                .buckets
                .iter()
                .find(|(b, _)| *b == le)
                .map_or(0, |(_, n)| *n);
            buckets.push((le, n.checked_sub(before)?));
        }
        Some(HostTotals { classes, buckets })
    }

    /// The `q`-quantile of the latency in milliseconds, interpolated within its histogram bucket
    /// the way Prometheus' `histogram_quantile` does.
    pub fn quantile_ms(&self, q: f64) -> Option<f64> {
        let &(_, total) = self.buckets.last()?;
        if total == 0 {
            return None;
        }
        let rank = q * total as f64;
        let i = self.buckets.iter().position(|&(_, n)| n as f64 >= rank)?;
        let (le, n) = self.buckets[i];
        let (lower, below) = if i == 0 {
            (0.0, 0)
        } else {
            self.buckets[i - 1]
        };
        if le.is_infinite() {
            return Some(lower * 1000.0);
        }
        let within = (n - below) as f64;
        let fraction = if within > 0.0 {
            (rank - below as f64) / within
        } else {
            1.0
        };
        Some((lower + (le - lower) * fraction) * 1000.0)
    }

    /// The sample for this many seconds of these counters.
    pub fn sample(&self, secs: f64) -> HttpSample {
        HttpSample {
            secs,
            classes: self.classes,
            p50_ms: self.quantile_ms(0.5),
            p95_ms: self.quantile_ms(0.95),
        }
    }
}

/// The label set and value of one sample line, e.g. `name{a="b"} 3`.
fn labels(line: &str) -> Option<(HashMap<&str, String>, f64)> {
    let open = line.find('{')?;
    let close = line.rfind('}')?;
    let value = line[close + 1..].split_whitespace().next()?.parse().ok()?;
    let mut map = HashMap::new();
    let mut rest = &line[open + 1..close];
    while let Some(eq) = rest.find("=\"") {
        let key = rest[..eq].trim_start_matches(',').trim();
        let mut val = String::new();
        let mut chars = rest[eq + 2..].char_indices();
        let mut end = None;
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some((_, 'n')) => val.push('\n'),
                    Some((_, c)) => val.push(c),
                    None => break,
                },
                '"' => {
                    end = Some(eq + 2 + i + 1);
                    break;
                }
                c => val.push(c),
            }
        }
        map.insert(key, val);
        rest = &rest[end?..];
    }
    Some((map, value))
}

/// Per-host totals from Caddy's metrics text, summed over handlers, methods and servers.
pub fn parse(text: &str) -> HashMap<String, HostTotals> {
    let mut hosts: HashMap<String, HostTotals> = HashMap::new();
    for line in text.lines() {
        let is_count = line.starts_with(REQUESTS) && line[REQUESTS.len()..].starts_with('{');
        let is_bucket = line.starts_with(BUCKETS) && line[BUCKETS.len()..].starts_with('{');
        if !is_count && !is_bucket {
            continue;
        }
        let Some((labels, value)) = labels(line) else {
            continue;
        };
        let Some(host) = labels.get("host") else {
            continue;
        };
        let totals = hosts.entry(host.to_ascii_lowercase()).or_default();
        let n = value.max(0.0) as u64;
        if is_count {
            let class = labels
                .get("code")
                .and_then(|c| c.chars().next())
                .and_then(|c| c.to_digit(10));
            if let Some(class @ 2..=5) = class {
                totals.classes[class as usize - 2] += n;
            }
        } else if let Some(le) = labels.get("le") {
            let le = if le == "+Inf" {
                f64::INFINITY
            } else {
                match le.parse() {
                    Ok(le) => le,
                    Err(_) => continue,
                }
            };
            totals.add_bucket(le, n);
        }
    }
    for totals in hosts.values_mut() {
        totals.buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    hosts
}

/// Reads the proxy's metrics on a server.
pub async fn scrape(docker: &Docker) -> Result<HashMap<String, HostTotals>> {
    let mut out: Vec<u8> = Vec::new();
    docker::exec_stream(
        docker,
        brand::PROXY_CONTAINER,
        vec!["wget".into(), "-qO-".into(), METRICS_URL.into()],
        Vec::new(),
        None,
        &mut out,
    )
    .await?
    .check("reading the proxy's metrics")
    .context("cannot read the proxy's metrics")?;
    Ok(parse(&String::from_utf8_lossy(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"# HELP caddy_http_request_duration_seconds Histogram of round-trip request durations.
# TYPE caddy_http_request_duration_seconds histogram
caddy_http_request_duration_seconds_bucket{code="200",handler="subroute",host="Shop.example.com",method="GET",server="srv0",le="0.01"} 50
caddy_http_request_duration_seconds_bucket{code="200",handler="subroute",host="Shop.example.com",method="GET",server="srv0",le="0.1"} 90
caddy_http_request_duration_seconds_bucket{code="200",handler="subroute",host="Shop.example.com",method="GET",server="srv0",le="+Inf"} 100
caddy_http_request_duration_seconds_sum{code="200",handler="subroute",host="Shop.example.com",method="GET",server="srv0"} 3.2
caddy_http_request_duration_seconds_count{code="200",handler="subroute",host="Shop.example.com",method="GET",server="srv0"} 100
caddy_http_request_duration_seconds_bucket{code="502",handler="subroute",host="shop.example.com",method="POST",server="srv0",le="0.01"} 0
caddy_http_request_duration_seconds_bucket{code="502",handler="subroute",host="shop.example.com",method="POST",server="srv0",le="0.1"} 0
caddy_http_request_duration_seconds_bucket{code="502",handler="subroute",host="shop.example.com",method="POST",server="srv0",le="+Inf"} 2
caddy_http_request_duration_seconds_count{code="502",handler="subroute",host="shop.example.com",method="POST",server="srv0"} 2
caddy_http_request_duration_seconds_count{code="308",handler="static_response",host="api.example.com",method="GET",server="srv1"} 1.5e+01
caddy_http_requests_total{handler="subroute",host="shop.example.com",server="srv0"} 102
"#;

    #[test]
    fn hosts_sum_their_series() {
        let hosts = parse(TEXT);
        assert_eq!(hosts.len(), 2);
        let shop = &hosts["shop.example.com"];
        assert_eq!(shop.classes, [100, 0, 0, 2]);
        assert_eq!(
            shop.buckets,
            vec![(0.01, 50), (0.1, 90), (f64::INFINITY, 102)]
        );
        assert_eq!(hosts["api.example.com"].classes, [0, 15, 0, 0]);
    }

    #[test]
    fn labels_handle_escapes() {
        let (l, v) = labels(r#"x{a="q\"uote",b="c,d"} 7"#).unwrap();
        assert_eq!(l["a"], "q\"uote");
        assert_eq!(l["b"], "c,d");
        assert_eq!(v, 7.0);
    }

    #[test]
    fn quantiles_interpolate_within_a_bucket() {
        let t = HostTotals {
            classes: [100, 0, 0, 0],
            buckets: vec![(0.01, 50), (0.1, 90), (f64::INFINITY, 100)],
        };
        // The 50th request is the last of the first bucket.
        assert!((t.quantile_ms(0.5).unwrap() - 10.0).abs() < 1e-9);
        // The 70th is halfway through 10..100 ms.
        assert!((t.quantile_ms(0.7).unwrap() - 55.0).abs() < 1e-9);
        // Past the last finite bound, the bound is the best answer.
        assert!((t.quantile_ms(0.95).unwrap() - 100.0).abs() < 1e-9);
        assert_eq!(HostTotals::default().quantile_ms(0.5), None);
    }

    #[test]
    fn differences_skip_a_restarted_proxy() {
        let before = HostTotals {
            classes: [10, 0, 1, 0],
            buckets: vec![(0.1, 10), (f64::INFINITY, 11)],
        };
        let now = HostTotals {
            classes: [25, 0, 1, 2],
            buckets: vec![(0.1, 22), (f64::INFINITY, 28)],
        };
        let d = now.since(&before).unwrap();
        assert_eq!(d.classes, [15, 0, 0, 2]);
        assert_eq!(d.buckets, vec![(0.1, 12), (f64::INFINITY, 17)]);
        assert_eq!(before.since(&now), None);

        let mut sum = d.clone();
        sum.add(&d);
        assert_eq!(sum.classes, [30, 0, 0, 4]);
        let s = sum.sample(10.0);
        assert_eq!(s.classes, [30, 0, 0, 4]);
        assert!(s.p50_ms.is_some());
    }
}
