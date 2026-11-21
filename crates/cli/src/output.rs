//! Terminal output: the TUI palette as ANSI colors, aligned tables, and `--json`.

use common::*;
use std::io::IsTerminal;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub enum Tone {
    Text,
    Muted,
    Accent,
    Ok,
    Info,
    Err,
}

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

pub fn paint(text: impl AsRef<str>, tone: Tone) -> String {
    let text = text.as_ref();
    if !enabled() {
        return text.to_string();
    }
    let (r, g, b) = match tone {
        Tone::Text => (0xD8, 0xDE, 0xE3),
        Tone::Muted => (0x7C, 0x87, 0x8F),
        Tone::Accent => (0xF5, 0xA5, 0x24),
        Tone::Ok => (0x4F, 0xBF, 0x7F),
        Tone::Info => (0x6C, 0xA6, 0xE0),
        Tone::Err => (0xE5, 0x53, 0x4B),
    };
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

pub fn muted(text: impl AsRef<str>) -> String {
    paint(text, Tone::Muted)
}

pub fn ok(message: impl AsRef<str>) {
    println!("{} {}", paint("✓", Tone::Ok), message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    println!("{} {}", paint("!", Tone::Accent), message.as_ref());
}

pub fn note(message: impl AsRef<str>) {
    println!("{}", muted(message));
}

/// A table cell: plain text plus the tone it is drawn in.
pub struct Cell(pub String, pub Tone);

impl From<String> for Cell {
    fn from(s: String) -> Self {
        Cell(s, Tone::Text)
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Self {
        Cell(s.to_string(), Tone::Text)
    }
}

pub fn table(headers: &[&str], rows: Vec<Vec<Cell>>) {
    if rows.is_empty() {
        return;
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.0.chars().count());
        }
    }
    let line = |cells: Vec<(String, Tone)>| {
        let last = cells.len() - 1;
        let out: Vec<String> = cells
            .into_iter()
            .enumerate()
            .map(|(i, (text, tone))| {
                let pad = if i == last {
                    0
                } else {
                    widths[i] - text.chars().count()
                };
                format!("{}{}", paint(&text, tone), " ".repeat(pad))
            })
            .collect();
        println!("{}", out.join("  ").trim_end());
    };
    line(
        headers
            .iter()
            .map(|h| (h.to_string(), Tone::Muted))
            .collect(),
    );
    for row in rows {
        line(row.into_iter().map(|c| (c.0, c.1)).collect());
    }
}

pub fn json<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn state(state: ServiceState) -> Cell {
    let (glyph, tone) = match state {
        ServiceState::Running => ("●", Tone::Ok),
        ServiceState::Building | ServiceState::Deploying => ("◐", Tone::Info),
        ServiceState::Failed => ("✕", Tone::Err),
        ServiceState::Degraded => ("▲", Tone::Accent),
        ServiceState::Stopped => ("○", Tone::Muted),
        ServiceState::New => ("◌", Tone::Muted),
    };
    Cell(format!("{glyph} {}", state.as_str()), tone)
}

pub fn deployment(status: DeploymentStatus) -> Cell {
    let tone = match status {
        DeploymentStatus::Live => Tone::Ok,
        DeploymentStatus::Failed => Tone::Err,
        DeploymentStatus::Queued | DeploymentStatus::Building | DeploymentStatus::Starting => {
            Tone::Info
        }
        DeploymentStatus::Superseded | DeploymentStatus::Cancelled => Tone::Muted,
    };
    Cell(status.as_str().to_string(), tone)
}

pub fn ago(at: Timestamp) -> String {
    let secs = (chrono::Utc::now() - at).num_seconds().max(0);
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// `45s`, `12m`, `3h`, `9d`.
pub fn duration(secs: i64) -> String {
    match secs.max(0) {
        s @ 0..60 => format!("{s}s"),
        s @ 60..3600 => format!("{}m", s / 60),
        s @ 3600..86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// A block sparkline of all of `values` in at most `width` columns, each the average of
/// the values it covers, scaled from zero to the highest column.
pub fn sparkline(values: &[f64], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let columns = values.len().min(width);
    let averages: Vec<f64> = (0..columns)
        .map(|i| {
            let chunk = &values[i * values.len() / columns..(i + 1) * values.len() / columns];
            chunk.iter().sum::<f64>() / chunk.len() as f64
        })
        .collect();
    let top = averages.iter().copied().fold(0.0, f64::max);
    averages
        .iter()
        .map(|v| {
            let level = if top > 0.0 { v / top * 7.0 } else { 0.0 };
            BARS[level.round().clamp(0.0, 7.0) as usize]
        })
        .collect()
}

/// A file size such as `512 B`, `3.4 KB` or `1.2 GB`.
pub fn size(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn bytes(n: u64) -> String {
    let n = n as f64;
    let g = 1024.0 * 1024.0 * 1024.0;
    if n >= g {
        format!("{:.1}G", n / g)
    } else {
        format!("{:.0}M", n / (1024.0 * 1024.0))
    }
}
