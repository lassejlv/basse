//! Autoscaling: every [`INTERVAL`], each service with autoscaling on gets the replica count its
//! targets ask for, from the last minute of metrics samples.
//!
//! Like Kubernetes' horizontal autoscaler, a target asks for `ceil(replicas * observed /
//! target)` replicas and the busiest target wins. Flapping is held off three ways: a ratio
//! within [`TOLERANCE`] of the target asks for no change, scaling down follows the highest
//! recommendation of the stabilization window, and each direction waits out its cooldown after
//! any scaling. The result stays within `min..=max` and the server's memory headroom.

use crate::notify::Event;
use crate::replicas;
use crate::state::AppState;
use chrono::TimeDelta;
use common::notify::NotifyEvent;
use common::*;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const INTERVAL: Duration = Duration::from_secs(15);
/// Observed values within this share of their target ask for no change.
pub const TOLERANCE: f64 = 0.1;
/// How far back the observed averages look.
const WINDOW_SECS: i64 = 60;

/// Average load per replica over the window; `None` where nothing was measured.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Observed {
    /// Percent of one core.
    pub cpu: Option<f64>,
    /// Percent of the memory limit.
    pub memory: Option<f64>,
    pub rps: Option<f64>,
}

/// What the autoscaler remembers of a service between rounds.
#[derive(Debug, Clone, Default)]
pub struct Track {
    pub last_scaled: Option<Timestamp>,
    /// Bounded recommendations of the stabilization window, oldest first.
    pub recommendations: VecDeque<(Timestamp, u16)>,
    /// A limit was reported and has not cleared since.
    pub limit_reported: bool,
}

/// One round's verdict for a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The new replica count, when it should change now.
    pub scale_to: Option<u16>,
    /// What the targets asked for before `max` applied.
    pub wanted: u16,
    /// The target that decided, e.g. `cpu 140% of 70%`.
    pub reason: String,
}

/// Decides one round. Pure, so the rules can be tested with synthetic metrics.
pub fn decide(
    cfg: &Autoscale,
    current: u16,
    observed: Observed,
    now: Timestamp,
    track: &mut Track,
) -> Decision {
    let targets = [
        ("cpu", observed.cpu, cfg.cpu_percent.map(f64::from), "%"),
        (
            "memory",
            observed.memory,
            cfg.memory_percent.map(f64::from),
            "%",
        ),
        ("rps", observed.rps, cfg.rps.map(f64::from), ""),
    ];
    let current_f = f64::from(current.max(1));
    let mut wanted = current;
    let mut reason = String::from("no metrics yet");
    let mut measured = false;
    for (label, observed, target, unit) in targets {
        let (Some(observed), Some(target)) = (observed, target) else {
            continue;
        };
        let ratio = observed / target;
        let want = if (ratio - 1.0).abs() <= TOLERANCE {
            current
        } else {
            (current_f * ratio).ceil().clamp(0.0, f64::from(u16::MAX)) as u16
        };
        if !measured || want > wanted {
            wanted = want;
            reason = format!("{label} {observed:.0}{unit} of {target:.0}{unit}");
        }
        measured = true;
    }
    let bounded = wanted.clamp(cfg.min, cfg.max);
    let window = TimeDelta::seconds(i64::from(cfg.stabilization_secs));
    track.recommendations.push_back((now, bounded));
    while track
        .recommendations
        .front()
        .is_some_and(|(at, _)| *at < now - window)
    {
        track.recommendations.pop_front();
    }
    let cooled = |secs: u32| {
        track
            .last_scaled
            .is_none_or(|at| now - at >= TimeDelta::seconds(i64::from(secs)))
    };
    let scale_to = if current < cfg.min || current > cfg.max {
        reason = format!("outside {}–{}", cfg.min, cfg.max);
        Some(current.clamp(cfg.min, cfg.max))
    } else if !measured {
        None
    } else if bounded > current && cooled(cfg.scale_up_cooldown_secs) {
        Some(bounded)
    } else if bounded < current && cooled(cfg.scale_down_cooldown_secs) {
        let stable = track
            .recommendations
            .iter()
            .map(|(_, n)| *n)
            .max()
            .unwrap_or(bounded);
        (stable < current).then_some(stable)
    } else {
        None
    };
    if scale_to.is_some() {
        track.last_scaled = Some(now);
    }
    Decision {
        scale_to,
        wanted,
        reason,
    }
}

/// Averages per replica from metric points: the sums over replicas divided by how many replicas
/// reported.
pub fn observe(points: &[common::metrics::MetricPoint], current: u16) -> Observed {
    if points.is_empty() {
        return Observed::default();
    }
    let n = points.len() as f64;
    let per = |p: &common::metrics::MetricPoint| {
        f64::from(p.replicas.max(1).min(u32::from(current.max(1))).max(1))
    };
    let cpu = points.iter().map(|p| p.cpu / per(p)).sum::<f64>() / n;
    let memory = points
        .iter()
        .filter(|p| p.mem_limit > 0)
        .map(|p| p.mem as f64 / per(p) / p.mem_limit as f64 * 100.0)
        .collect::<Vec<_>>();
    let rps: Vec<f64> = points
        .iter()
        .filter_map(|p| p.http.as_ref().map(|h| h.rps / per(p)))
        .collect();
    let avg = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    Observed {
        cpu: Some(cpu),
        memory: avg(&memory),
        rps: avg(&rps),
    }
}

/// Each service's [`Track`].
#[derive(Default)]
pub struct Tracker {
    tracks: Mutex<HashMap<Id, Track>>,
}

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(e) = round(&state).await {
                tracing::warn!("autoscaling round failed: {e:#}");
            }
        }
    });
}

async fn round(state: &Arc<AppState>) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let recs = state.db.services().await?;
    {
        let mut tracks = state
            .autoscaler
            .tracks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        tracks.retain(|id, _| {
            recs.iter()
                .any(|r| r.service.id == *id && r.service.scaling.autoscale.enabled)
        });
    }
    for rec in recs {
        let svc = &rec.service;
        let cfg = &svc.scaling.autoscale;
        if !cfg.enabled
            || rec.busy.is_some()
            || rec.stopped
            || svc.live_deployment_id.is_none()
            || !replicas::supports_replicas(&svc.source)
        {
            continue;
        }
        let from = now - TimeDelta::seconds(WINDOW_SECS);
        let buckets = state
            .db
            .metric_buckets(&svc.id, common::metrics::SAMPLE_SECS, from, None)
            .await?;
        let points = crate::metrics::combine(&buckets);
        let current = svc.replicas;
        let observed = observe(&points, current);
        let mut decision = {
            let mut tracks = state
                .autoscaler
                .tracks
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let track = tracks.entry(svc.id.clone()).or_default();
            decide(cfg, current, observed, now, track)
        };
        let mut limit = (decision.wanted > cfg.max && current >= cfg.max)
            .then(|| format!("wants {} replicas, max is {}", decision.wanted, cfg.max));
        if let Some(to) = decision.scale_to
            && to > current
        {
            let fits = match replicas::capacity(state, &rec).await {
                Ok(cap) => {
                    let room = u16::try_from(cap.fits_more).unwrap_or(u16::MAX);
                    Some((current.saturating_add(room), cap))
                }
                Err(e) => {
                    tracing::warn!("cannot check memory for {}: {e:#}", svc.name);
                    None
                }
            };
            if let Some((allowed, cap)) = fits
                && to > allowed
            {
                limit = Some(format!(
                    "wants {to} replicas, server memory fits {allowed} (~{} each, {} free)",
                    replicas::human_bytes(cap.per_replica),
                    replicas::human_bytes(cap.mem_free)
                ));
                decision.scale_to = (allowed > current).then_some(allowed);
            }
        }
        report_limit(state, svc, limit).await;
        let Some(to) = decision.scale_to else {
            continue;
        };
        state.db.set_replica_count(&svc.id, to).await?;
        replicas::spawn_reconcile(state, &svc.id, format!("autoscale: {}", decision.reason));
    }
    Ok(())
}

async fn report_limit(state: &AppState, svc: &Service, limit: Option<String>) {
    let first = {
        let mut tracks = state
            .autoscaler
            .tracks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let track = tracks.entry(svc.id.clone()).or_default();
        let first = limit.is_some() && !track.limit_reported;
        track.limit_reported = limit.is_some();
        first
    };
    let Some(limit) = limit.filter(|_| first) else {
        return;
    };
    state
        .service_activity(
            svc,
            ActivityLevel::Error,
            "Autoscaling hit its limit".into(),
            Some(limit.clone()),
        )
        .await;
    state
        .notify_service(
            svc,
            Event::new(NotifyEvent::ServiceScaleLimitReached)
                .detail(limit)
                .hint(format!(
                    "{} service autoscale {} --max <n>",
                    brand::CLI_BIN,
                    svc.name
                )),
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn at(secs: i64) -> Timestamp {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    fn cfg() -> Autoscale {
        Autoscale {
            enabled: true,
            min: 2,
            max: 6,
            cpu_percent: Some(70),
            memory_percent: None,
            rps: Some(50),
            scale_up_cooldown_secs: 60,
            scale_down_cooldown_secs: 300,
            stabilization_secs: 300,
        }
    }

    fn cpu(v: f64) -> Observed {
        Observed {
            cpu: Some(v),
            ..Default::default()
        }
    }

    #[test]
    fn scales_up_in_proportion_to_the_busiest_target() {
        let mut t = Track::default();
        let d = decide(&cfg(), 2, cpu(140.0), at(0), &mut t);
        assert_eq!(d.scale_to, Some(4));
        assert_eq!(d.reason, "cpu 140% of 70%");
        let mut t = Track::default();
        let both = Observed {
            cpu: Some(80.0),
            memory: None,
            rps: Some(120.0),
        };
        let d = decide(&cfg(), 2, both, at(0), &mut t);
        assert_eq!(d.scale_to, Some(5), "rps asks for ceil(2 * 2.4) = 5");
        assert!(d.reason.starts_with("rps"), "{}", d.reason);
    }

    #[test]
    fn small_deviations_change_nothing() {
        let mut t = Track::default();
        for (i, v) in [74.0, 66.0, 76.0, 64.0].into_iter().enumerate() {
            let d = decide(&cfg(), 3, cpu(v), at(i as i64 * 15), &mut t);
            assert_eq!(d.scale_to, None, "{v}% is within the tolerance of 70%");
        }
    }

    #[test]
    fn bounds_cap_the_count_and_report_what_was_wanted() {
        let mut t = Track::default();
        let d = decide(&cfg(), 4, cpu(700.0), at(0), &mut t);
        assert_eq!(d.scale_to, Some(6));
        assert_eq!(d.wanted, 40);
        let mut t = Track::default();
        let d = decide(&cfg(), 3, cpu(1.0), at(0), &mut t);
        assert_eq!(d.scale_to, Some(2), "never below min");
        let mut t = Track::default();
        let d = decide(&cfg(), 1, Observed::default(), at(0), &mut t);
        assert_eq!(
            d.scale_to,
            Some(2),
            "a count below min is fixed without metrics"
        );
        let d = decide(&cfg(), 9, Observed::default(), at(0), &mut Track::default());
        assert_eq!(d.scale_to, Some(6));
    }

    #[test]
    fn no_metrics_means_no_change() {
        let mut t = Track::default();
        let d = decide(&cfg(), 3, Observed::default(), at(0), &mut t);
        assert_eq!(d.scale_to, None);
        assert_eq!(d.reason, "no metrics yet");
    }

    #[test]
    fn cooldowns_space_out_scaling() {
        let mut t = Track::default();
        assert_eq!(
            decide(&cfg(), 2, cpu(140.0), at(0), &mut t).scale_to,
            Some(4)
        );
        // Still hot 30 s later: the scale-up cooldown holds.
        assert_eq!(decide(&cfg(), 4, cpu(140.0), at(30), &mut t).scale_to, None);
        assert_eq!(
            decide(&cfg(), 4, cpu(140.0), at(60), &mut t).scale_to,
            Some(6)
        );
        // Load drops right away: scaling down waits out its own, longer cooldown.
        assert_eq!(decide(&cfg(), 6, cpu(10.0), at(120), &mut t).scale_to, None);
    }

    #[test]
    fn scaling_down_follows_the_stabilization_window() {
        let mut t = Track {
            last_scaled: Some(at(-1000)),
            ..Default::default()
        };
        // Busy enough for 5 replicas, then a short dip.
        assert_eq!(decide(&cfg(), 5, cpu(70.0), at(0), &mut t).scale_to, None);
        assert_eq!(decide(&cfg(), 5, cpu(20.0), at(15), &mut t).scale_to, None);
        assert_eq!(decide(&cfg(), 5, cpu(20.0), at(200), &mut t).scale_to, None);
        // Once the busy recommendation leaves the window, the count drops to the window's max.
        let d = decide(&cfg(), 5, cpu(20.0), at(320), &mut t);
        assert_eq!(d.scale_to, Some(2), "ceil(5 * 20/70) = 2");
    }

    #[test]
    fn no_flapping_when_the_load_swings() {
        // The service's total CPU swings between 160% and 120% every round; each replica gets
        // its share.
        let mut t = Track::default();
        let mut replicas = 2;
        let mut changes = Vec::new();
        for i in 0..200 {
            let total = if i % 2 == 0 { 160.0 } else { 120.0 };
            let d = decide(
                &cfg(),
                replicas,
                cpu(total / f64::from(replicas)),
                at(i * 15),
                &mut t,
            );
            if let Some(to) = d.scale_to {
                replicas = to;
                changes.push(to);
            }
        }
        assert_eq!(changes, [3], "one step up, then steady");
    }

    #[test]
    fn observed_values_are_per_replica() {
        let point = |cpu: f64, replicas: u32, rps: Option<f64>| common::metrics::MetricPoint {
            at: at(0),
            cpu,
            cpu_max: cpu,
            mem: 300,
            mem_max: 300,
            mem_limit: 1000,
            net_rx: 0.0,
            net_tx: 0.0,
            disk_read: 0.0,
            disk_write: 0.0,
            pids: 1,
            restarts: 0,
            uptime_secs: 10,
            http: rps.map(|rps| common::metrics::HttpPoint {
                rps,
                s2xx: 1,
                s3xx: 0,
                s4xx: 0,
                s5xx: 0,
                p50_ms: None,
                p95_ms: None,
            }),
            replicas,
        };
        let o = observe(&[point(120.0, 3, Some(30.0)), point(60.0, 3, None)], 3);
        assert_eq!(o.cpu, Some(30.0));
        assert_eq!(o.memory, Some(10.0));
        assert_eq!(o.rps, Some(10.0));
        assert_eq!(observe(&[], 3), Observed::default());
    }
}
