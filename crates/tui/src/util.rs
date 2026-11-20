//! Small formatting helpers shared by every view.

use crate::theme;
use chrono::{DateTime, Local, Utc};
use common::{ActivityLevel, DeploymentStatus, ServiceState, StepStatus};
use ratatui::style::Style;
use ratatui::text::Span;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn spinner(tick: usize) -> &'static str {
    SPINNER[tick % SPINNER.len()]
}

pub fn state_glyph(state: ServiceState, tick: usize) -> Span<'static> {
    match state {
        ServiceState::Running => Span::styled("●", theme::fg(theme::OK)),
        ServiceState::Building | ServiceState::Deploying => {
            Span::styled(spinner(tick), theme::fg(theme::INFO))
        }
        ServiceState::Failed => Span::styled("✕", theme::fg(theme::ERR)),
        ServiceState::Degraded => Span::styled("▲", theme::accent()),
        ServiceState::Stopped => Span::styled("○", theme::muted()),
        ServiceState::New => Span::styled("◌", theme::muted()),
    }
}

pub fn state_style(state: ServiceState) -> Style {
    match state {
        ServiceState::Running => theme::fg(theme::OK),
        ServiceState::Building | ServiceState::Deploying => theme::fg(theme::INFO),
        ServiceState::Failed => theme::fg(theme::ERR),
        ServiceState::Degraded => theme::accent(),
        ServiceState::Stopped | ServiceState::New => theme::muted(),
    }
}

pub fn deployment_glyph(status: DeploymentStatus, tick: usize) -> Span<'static> {
    match status {
        DeploymentStatus::Live => Span::styled("●", theme::fg(theme::OK)),
        DeploymentStatus::Queued | DeploymentStatus::Building | DeploymentStatus::Starting => {
            Span::styled(spinner(tick), theme::fg(theme::INFO))
        }
        DeploymentStatus::Failed => Span::styled("✕", theme::fg(theme::ERR)),
        DeploymentStatus::Superseded | DeploymentStatus::Cancelled => {
            Span::styled("○", theme::muted())
        }
    }
}

pub fn deployment_style(status: DeploymentStatus) -> Style {
    match status {
        DeploymentStatus::Live => theme::fg(theme::OK),
        DeploymentStatus::Queued | DeploymentStatus::Building | DeploymentStatus::Starting => {
            theme::fg(theme::INFO)
        }
        DeploymentStatus::Failed => theme::fg(theme::ERR),
        DeploymentStatus::Superseded | DeploymentStatus::Cancelled => theme::muted(),
    }
}

pub fn step_glyph(status: StepStatus, tick: usize) -> Span<'static> {
    match status {
        StepStatus::Done => Span::styled("✓", theme::fg(theme::OK)),
        StepStatus::Running => Span::styled(spinner(tick), theme::fg(theme::INFO)),
        StepStatus::Failed => Span::styled("✕", theme::fg(theme::ERR)),
        StepStatus::Skipped => Span::styled("–", theme::muted()),
        StepStatus::Pending => Span::styled("○", theme::muted()),
    }
}

pub fn level_style(level: ActivityLevel) -> Style {
    match level {
        ActivityLevel::Info => theme::text(),
        ActivityLevel::Success => theme::fg(theme::OK),
        ActivityLevel::Error => theme::fg(theme::ERR),
    }
}

/// Block sparkline of the last `width` values, scaled to `max` (or the series max).
pub fn sparkline(values: &[f64], width: usize, max: Option<f64>) -> String {
    let tail = &values[values.len().saturating_sub(width)..];
    let top = max
        .unwrap_or_else(|| tail.iter().copied().fold(0.0, f64::max))
        .max(f64::EPSILON);
    tail.iter()
        .map(|v| {
            let idx = ((v / top) * (BARS.len() - 1) as f64)
                .round()
                .clamp(0.0, (BARS.len() - 1) as f64);
            BARS[idx as usize]
        })
        .collect()
}

/// `2m ago`, `3h ago`, `9d ago`.
pub fn ago(at: DateTime<Utc>) -> String {
    let secs = (Utc::now() - at).num_seconds().max(0);
    match secs {
        0..5 => "now".into(),
        5..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// `41s`, `1m12s`, `2h03m`.
pub fn duration(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

pub fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

pub fn clock_ms(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M:%S%.3f").to_string()
}

/// `212M`, `3.1G`.
pub fn bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    let n = n as f64;
    if n >= K * K * K {
        format!("{:.1}G", n / (K * K * K))
    } else if n >= K * K {
        format!("{:.0}M", n / (K * K))
    } else if n >= K {
        format!("{:.0}K", n / K)
    } else {
        format!("{n:.0}B")
    }
}

/// Cut `s` to `width` characters, ending in `…` when it was longer.
pub fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_scales_to_max() {
        assert_eq!(sparkline(&[0.0, 50.0, 100.0], 10, Some(100.0)), "▁▅█");
        assert_eq!(sparkline(&[1.0, 2.0, 3.0, 4.0], 2, None), "▆█");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(41), "41s");
        assert_eq!(duration(72), "1m12s");
        assert_eq!(duration(7380), "2h03m");
    }

    #[test]
    fn truncate_marks_cut_text() {
        assert_eq!(truncate("ghcr.io/imgproxy", 8), "ghcr.io…");
        assert_eq!(truncate("web", 8), "web");
    }

    #[test]
    fn byte_units() {
        assert_eq!(bytes(212 * 1024 * 1024), "212M");
        assert_eq!(bytes(3_328_599_654), "3.1G");
    }
}
