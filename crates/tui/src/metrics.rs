//! The Metrics tab of a service: a range switcher, a container switcher for compose groups,
//! and six braille line charts with deploy markers, following the "SVC-05 Service · Metrics"
//! artboards in Paper.

use crate::app::{App, ServiceView};
use crate::mouse::{self, Hit};
use crate::theme;
use crate::view::{inside, panel};
use chrono::TimeDelta;
use common::ServiceState;
use common::metrics::{MetricPoint, MetricsRange, ServiceMetrics};
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, Paragraph};

const COLLECTING: &str = "collecting · first samples in ~10s";
const NOT_RUNNING: &str = "not running · no samples in this range";
const NO_DOMAIN: &str = "no domain · requests appear once a domain routes here";
const NO_REQUESTS: &str = "no requests in this range";
const ALL_REPLICAS: &str = "requests are measured across all replicas";
/// Width of the y-axis labels plus their gutter.
const Y_LABELS: u16 = 6;

pub(crate) fn tab(frame: &mut Frame, area: Rect, app: &App, view: &ServiceView) {
    let [bar, _, grid] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let running = view
        .detail
        .as_ref()
        .map(|d| d.service.state)
        .or_else(|| app.service_path(&view.id).map(|(_, _, s)| s.state))
        == Some(ServiceState::Running);
    toolbar(frame, bar, view, running);

    // A new range or container keeps showing the last answer until its own arrives.
    let data = view.metrics.data.as_ref();
    let points = data.map_or(&[][..], |m| m.points.as_slice());
    let show_replicas = view.compose().is_empty()
        && view.metrics.replica.is_none()
        && (view.replicated() || points.iter().any(|p| p.replicas > 1));
    let (grid, replicas_area) = if show_replicas {
        let [grid, replicas] = Layout::vertical([Constraint::Fill(1), Constraint::Length(8)])
            .spacing(1)
            .areas(grid);
        (grid, Some(replicas))
    } else {
        (grid, None)
    };
    let rows = Layout::vertical([Constraint::Fill(1); 3])
        .spacing(1)
        .split(grid);
    let cells: Vec<Rect> = rows
        .iter()
        .flat_map(|row| {
            Layout::horizontal([Constraint::Fill(1); 2])
                .spacing(1)
                .split(*row)
                .to_vec()
        })
        .collect();
    let empty = if running { COLLECTING } else { NOT_RUNNING };
    if let Some(area) = replicas_area {
        let autoscale = view
            .detail
            .as_ref()
            .map(|d| &d.service.scaling.autoscale)
            .filter(|a| a.enabled);
        let message = points.is_empty().then_some(empty);
        chart_panel(frame, area, replica_count(points, autoscale), data, message);
    }
    for (i, area) in cells.into_iter().enumerate() {
        let spec = match i {
            0 => cpu(points),
            1 => memory(points),
            2 => network(points),
            3 => disk(points),
            4 => requests(points),
            _ => latency(points),
        };
        let http_panel = i >= 4;
        let message = match data {
            Some(_) if http_panel && view.metrics.replica.is_some() => Some(ALL_REPLICAS),
            Some(m) if http_panel && !m.http => Some(NO_DOMAIN),
            _ if points.is_empty() => Some(empty),
            _ if http_panel && spec.series.iter().all(|(_, s)| s.is_empty()) => Some(NO_REQUESTS),
            _ => None,
        };
        chart_panel(frame, area, spec, data, message);
    }
}

fn toolbar(frame: &mut Frame, area: Rect, view: &ServiceView, running: bool) {
    let mut spans = vec![Span::raw(" ")];
    for (i, range) in MetricsRange::ALL.into_iter().enumerate() {
        let offset: usize = spans.iter().map(Span::width).sum();
        let digit = Span::styled((i + 1).to_string(), theme::accent());
        let label = Span::styled(
            range.id(),
            if range == view.metrics.range {
                theme::text().add_modifier(Modifier::UNDERLINED)
            } else {
                theme::muted()
            },
        );
        mouse::span_target(
            area,
            offset,
            digit.width() + 1 + label.width(),
            Hit::MetricsRange(range),
        );
        spans.extend([digit, Span::raw(" "), label, Span::raw("   ")]);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    let mut right: Vec<(Span, Option<Hit>)> = Vec::new();
    let compose = view.compose();
    if !compose.is_empty() {
        right.push((
            Span::styled("c", theme::accent()),
            Some(Hit::Key(KeyCode::Char('c'), KeyModifiers::NONE)),
        ));
        right.push((Span::styled(" container", theme::muted()), None));
        let names = std::iter::once("all").chain(compose.iter().map(|s| s.name.as_str()));
        for (i, name) in names.enumerate() {
            let picked = match &view.metrics.container {
                None => i == 0,
                Some(c) => i > 0 && c == name,
            };
            right.push((Span::raw("  "), None));
            right.push((
                Span::styled(
                    name.to_string(),
                    if picked {
                        theme::text().add_modifier(Modifier::UNDERLINED)
                    } else {
                        theme::muted()
                    },
                ),
                Some(Hit::MetricsContainer(i)),
            ));
        }
        right.push((Span::raw("    "), None));
    }
    let replicas = view.metric_replicas();
    if compose.is_empty() && !replicas.is_empty() {
        right.push((
            Span::styled("c", theme::accent()),
            Some(Hit::Key(KeyCode::Char('c'), KeyModifiers::NONE)),
        ));
        right.push((Span::styled(" replica", theme::muted()), None));
        let names = std::iter::once("all replicas".to_string())
            .chain(replicas.iter().map(|r| format!("#{r}")));
        for (i, name) in names.enumerate() {
            let picked = match view.metrics.replica {
                None => i == 0,
                Some(r) => i > 0 && replicas[i - 1] == r,
            };
            right.push((Span::raw("  "), None));
            right.push((
                Span::styled(
                    name,
                    if picked {
                        theme::text().add_modifier(Modifier::UNDERLINED)
                    } else {
                        theme::muted()
                    },
                ),
                Some(Hit::MetricsReplica(i)),
            ));
        }
        right.push((Span::raw("    "), None));
    }
    if running {
        right.push((Span::styled("●", theme::fg(theme::OK)), None));
        right.push((Span::styled(" live · every 10s", theme::muted()), None));
    } else {
        right.push((Span::styled("○ not running", theme::muted()), None));
    }
    right.push((Span::raw(" "), None));
    let width: usize = right.iter().map(|(s, _)| s.width()).sum();
    let left: usize = (area.width as usize).saturating_sub(width);
    let used: usize = MetricsRange::ALL
        .iter()
        .map(|r| r.id().len() + 5)
        .sum::<usize>()
        + 1;
    if left < used {
        return;
    }
    let mut offset = left;
    for (span, hit) in &right {
        if let Some(hit) = hit {
            mouse::span_target(area, offset, span.width(), *hit);
        }
        offset += span.width();
    }
    let rect = Rect {
        x: area.x + left as u16,
        width: width as u16,
        ..area
    };
    frame.render_widget(
        Paragraph::new(Line::from(
            right.into_iter().map(|(s, _)| s).collect::<Vec<_>>(),
        )),
        rect,
    );
}

/// One chart panel: its title, legend, series in draw order, y scale and limit line.
struct Spec {
    title: Vec<Span<'static>>,
    legend: Vec<(&'static str, Color)>,
    /// Muted text at the right of the title, instead of a legend.
    note: Option<&'static str>,
    /// Drawn in order, so the last series wins a shared cell.
    series: Vec<(Color, Vec<(TimeDelta, f64)>)>,
    y_max: f64,
    y_label: fn(f64) -> String,
    /// A dotted line at this value, labelled "limit".
    limit: Option<f64>,
}

fn title(name: &'static str, now: Option<String>, rest: String) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(name, theme::text())];
    if now.is_some() || !rest.is_empty() {
        spans.push(Span::raw("  "));
    }
    if let Some(now) = now {
        spans.push(Span::styled(now, theme::text()));
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest, theme::muted()));
    }
    spans
}

fn series(
    points: &[MetricPoint],
    pick: impl Fn(&MetricPoint) -> Option<f64>,
) -> Vec<(TimeDelta, f64)> {
    let Some(first) = points.first() else {
        return Vec::new();
    };
    points
        .iter()
        .filter_map(|p| Some((p.at - first.at, pick(p)?)))
        .collect()
}

fn peak(values: impl Iterator<Item = f64>) -> f64 {
    values.fold(0.0, f64::max)
}

/// The next of 1, 2 and 5 times a power of ten at or above `v`, so axes read in round numbers.
pub(crate) fn nice_ceil(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let base = 10f64.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * base)
        .find(|n| *n >= v * 0.999_999)
        .unwrap_or(10.0 * base)
}

fn percent(v: f64) -> String {
    format!("{v:.0}%")
}

/// Like [`nice_ceil`] in powers of 1024, so a memory axis reads `8M` rather than `9.5M`.
fn nice_ceil_binary(v: f64) -> f64 {
    let mut scale = 1.0;
    while v / scale >= 1024.0 {
        scale *= 1024.0;
    }
    match nice_ceil(v / scale) {
        n if n > 512.0 => 1024.0 * scale,
        n => n * scale,
    }
}

/// `212 MiB`, binary units for memory.
pub(crate) fn mib(bytes: f64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes;
    let mut unit = 0;
    while v >= 1024.0 && unit < units.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 || v >= 10.0 {
        format!("{v:.0} {}", units[unit])
    } else {
        format!("{v:.1} {}", units[unit])
    }
}

/// `1.2 MB/s`, decimal units for throughput.
pub(crate) fn rate(bytes: f64) -> String {
    let units = ["B/s", "kB/s", "MB/s", "GB/s"];
    let mut v = bytes;
    let mut unit = 0;
    while v >= 1000.0 && unit < units.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 || v >= 10.0 {
        format!("{v:.0} {}", units[unit])
    } else {
        format!("{v:.1} {}", units[unit])
    }
}

/// Axis labels such as `512M` or `40k`.
fn short(v: f64, base: f64) -> String {
    let mut v = v;
    for unit in ["", "k", "M", "G"] {
        if v < base {
            return format!("{}{unit}", trim(v));
        }
        v /= base;
    }
    format!("{}T", trim(v))
}

fn trim(v: f64) -> String {
    if v.fract() == 0.0 || v >= 10.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

fn cpu(points: &[MetricPoint]) -> Spec {
    let last = points.last();
    let avg = points.iter().map(|p| p.cpu).sum::<f64>() / points.len().max(1) as f64;
    let top = peak(points.iter().map(|p| p.cpu_max));
    Spec {
        title: title(
            "CPU",
            last.map(|p| percent(p.cpu)),
            last.map_or(String::new(), |_| {
                format!(" now · peak {} · avg {}", percent(top), percent(avg))
            }),
        ),
        legend: Vec::new(),
        note: None,
        series: vec![(theme::INFO, series(points, |p| Some(p.cpu)))],
        y_max: nice_ceil(peak(points.iter().map(|p| p.cpu))).max(100.0),
        y_label: percent,
        limit: None,
    }
}

fn memory(points: &[MetricPoint]) -> Spec {
    let last = points.last();
    let top = peak(points.iter().map(|p| p.mem_max as f64));
    let limit = last.map_or(0.0, |p| p.mem_limit as f64);
    // A limit far above what the service uses (often the host's memory) would flatten the
    // line, so the scale follows the usage and the limit stays in the title.
    let near = limit > 0.0 && limit <= top * 4.0;
    Spec {
        title: title(
            "Memory",
            last.map(|p| mib(p.mem as f64)),
            last.map_or(String::new(), |_| {
                let mut rest = format!(" now · peak {}", mib(top));
                if limit > 0.0 {
                    rest.push_str(&format!(" · limit {}", mib(limit)));
                }
                rest
            }),
        ),
        legend: Vec::new(),
        note: None,
        series: vec![(theme::INFO, series(points, |p| Some(p.mem as f64)))],
        y_max: if near {
            limit
        } else {
            nice_ceil_binary(top * 1.2)
        },
        y_label: |v| short(v, 1024.0),
        limit: near.then_some(limit),
    }
}

fn pair(
    name: &'static str,
    labels: [&'static str; 2],
    points: &[MetricPoint],
    pick: [fn(&MetricPoint) -> f64; 2],
) -> Spec {
    let last = points.last();
    let top = peak(points.iter().map(|p| pick[0](p).max(pick[1](p))));
    Spec {
        title: title(
            name,
            None,
            last.map_or(String::new(), |p| {
                format!(
                    "{} {} · {} {}",
                    labels[0],
                    rate(pick[0](p)),
                    labels[1],
                    rate(pick[1](p))
                )
            }),
        ),
        legend: vec![(labels[0], theme::INFO), (labels[1], theme::OK)],
        note: None,
        series: vec![
            (theme::OK, series(points, |p| Some(pick[1](p)))),
            (theme::INFO, series(points, |p| Some(pick[0](p)))),
        ],
        y_max: nice_ceil(top),
        y_label: |v| short(v, 1000.0),
        limit: None,
    }
}

fn network(points: &[MetricPoint]) -> Spec {
    pair(
        "Network",
        ["rx", "tx"],
        points,
        [|p| p.net_rx, |p| p.net_tx],
    )
}

fn disk(points: &[MetricPoint]) -> Spec {
    let mut spec = pair(
        "Disk I/O",
        ["read", "write"],
        points,
        [|p| p.disk_read, |p| p.disk_write],
    );
    // Writes are drawn last: they are what usually moves.
    spec.series.reverse();
    spec
}

fn requests(points: &[MetricPoint]) -> Spec {
    let http: Vec<_> = points.iter().filter_map(|p| p.http.as_ref()).collect();
    let total: u64 = http.iter().map(|h| h.requests()).sum();
    let share = |n: u64| n as f64 / total.max(1) as f64 * 100.0;
    let sum = |f: fn(&common::metrics::HttpPoint) -> u64| http.iter().map(|h| f(h)).sum::<u64>();
    let class = |f: fn(&common::metrics::HttpPoint) -> u64| {
        move |p: &MetricPoint| {
            p.http
                .as_ref()
                .map(|h| h.rps * f(h) as f64 / h.requests().max(1) as f64)
        }
    };
    let now = points.iter().rev().find_map(|p| p.http.as_ref());
    Spec {
        title: title(
            "Requests",
            now.map(|h| format!("{} req/s", trim(h.rps))),
            if total > 0 {
                format!(
                    " · 2xx {:.0}% · 4xx {:.0}% · 5xx {:.0}%",
                    share(sum(|h| h.s2xx)),
                    share(sum(|h| h.s4xx)),
                    share(sum(|h| h.s5xx))
                )
            } else {
                String::new()
            },
        ),
        legend: vec![
            ("2xx", theme::OK),
            ("4xx", theme::TEXT),
            ("5xx", theme::ERR),
        ],
        note: None,
        series: vec![
            (theme::ERR, series(points, class(|h| h.s5xx))),
            (theme::TEXT, series(points, class(|h| h.s4xx))),
            (theme::OK, series(points, class(|h| h.s2xx))),
        ],
        y_max: nice_ceil(peak(http.iter().map(|h| h.rps))),
        y_label: |v| trim(v),
        limit: None,
    }
}

/// Replicas over the range as a step line, the bounds of autoscaling in the title.
fn replica_count(points: &[MetricPoint], autoscale: Option<&common::Autoscale>) -> Spec {
    let mut steps: Vec<(TimeDelta, f64)> = Vec::new();
    if let Some(first) = points.first() {
        for p in points {
            let v = f64::from(p.replicas);
            if let Some(&(_, prev)) = steps.last()
                && prev != v
            {
                steps.push((p.at - first.at, prev));
            }
            steps.push((p.at - first.at, v));
        }
    }
    let now = points.last().map(|p| format!("{} now", p.replicas));
    let top = peak(steps.iter().map(|(_, v)| *v)).max(autoscale.map_or(0.0, |a| f64::from(a.max)));
    Spec {
        title: title(
            "Replicas",
            now,
            autoscale.map_or_else(String::new, |a| format!(" · min {} · max {}", a.min, a.max)),
        ),
        legend: Vec::new(),
        note: Some(if autoscale.is_some() {
            "autoscale on"
        } else {
            "autoscale off"
        }),
        series: vec![(theme::OK, steps)],
        y_max: top.max(2.0).ceil(),
        y_label: |v| format!("{v:.0}"),
        limit: None,
    }
}

fn latency(points: &[MetricPoint]) -> Spec {
    let now = points
        .iter()
        .rev()
        .find_map(|p| p.http.as_ref().filter(|h| h.p50_ms.is_some()));
    let ms = |v: Option<f64>| v.map_or("—".into(), |v| format!("{v:.0} ms"));
    let top = peak(
        points
            .iter()
            .filter_map(|p| p.http.as_ref()?.p95_ms.or(p.http.as_ref()?.p50_ms)),
    );
    Spec {
        title: title(
            "Latency",
            None,
            now.map_or(String::new(), |h| {
                format!("p50 {} · p95 {}", ms(h.p50_ms), ms(h.p95_ms))
            }),
        ),
        legend: vec![("p50", theme::INFO), ("p95", theme::TEXT)],
        note: None,
        series: vec![
            (theme::TEXT, series(points, |p| p.http.as_ref()?.p95_ms)),
            (theme::INFO, series(points, |p| p.http.as_ref()?.p50_ms)),
        ],
        y_max: nice_ceil(top),
        y_label: |v| format!("{}ms", trim(v)),
        limit: None,
    }
}

/// Labels for the start and middle of a range, e.g. `-1h` and `-30m`.
fn x_labels(range: MetricsRange) -> [&'static str; 2] {
    match range {
        MetricsRange::Hour => ["-1h", "-30m"],
        MetricsRange::Day => ["-24h", "-12h"],
        MetricsRange::Week => ["-7d", "-84h"],
        MetricsRange::Month => ["-30d", "-15d"],
    }
}

fn chart_panel(
    frame: &mut Frame,
    area: Rect,
    spec: Spec,
    data: Option<&ServiceMetrics>,
    message: Option<&str>,
) {
    let note = spec
        .note
        .map(|n| Line::from(Span::styled(n, theme::muted())));
    let legend = (message.is_none() && !spec.legend.is_empty()).then(|| {
        let mut spans = Vec::new();
        for (i, (label, color)) in spec.legend.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled("─", theme::fg(*color)));
            spans.push(Span::styled(format!(" {label}"), theme::muted()));
        }
        Line::from(spans)
    });
    let title = if message.is_some() {
        spec.title.into_iter().take(1).collect()
    } else {
        spec.title
    };
    let block = panel(title, legend.or(note), false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let (Some(m), None) = (data, message) else {
        let message = message.unwrap_or(COLLECTING);
        let row = Rect {
            y: inner.y + inner.height.saturating_sub(1) / 2,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(message).style(theme::muted()).centered(),
            row,
        );
        return;
    };
    let axes = inner.height >= 3 && inner.width > Y_LABELS + 10;
    let plot = if axes {
        Rect {
            x: inner.x + Y_LABELS,
            width: inner.width - Y_LABELS,
            height: inner.height - 1,
            ..inner
        }
    } else {
        inner
    };
    let span = (m.to - m.from).num_milliseconds().max(1) as f64 / 1000.0;
    let column = |at: chrono::DateTime<chrono::Utc>| {
        let x = (at - m.from).num_milliseconds() as f64 / 1000.0 / span;
        (x.clamp(0.0, 1.0) * f64::from(plot.width.saturating_sub(1))).round() as u16
    };
    let buf = frame.buffer_mut();
    let deploys: Vec<(u16, i64)> = m.deploys.iter().map(|d| (column(d.at), d.number)).collect();
    for &(col, _) in &deploys {
        for y in plot.top()..plot.bottom() {
            buf[(plot.x + col, y)]
                .set_symbol("┊")
                .set_style(theme::fg(theme::LINE));
        }
    }
    if let Some(limit) = spec.limit
        && limit > 0.0
        && plot.width > 8
    {
        let dots = plot.width as usize - 6;
        buf.set_string(plot.x, plot.y, "┄".repeat(dots), theme::muted());
        buf.set_string(plot.x + dots as u16 + 1, plot.y, "limit", theme::muted());
    }

    // Series are placed relative to the range start, so a point sits where its bucket began.
    let offset = data
        .and_then(|m| m.points.first())
        .map_or(0.0, |p| (p.at - m.from).num_milliseconds() as f64 / 1000.0);
    let owned: Vec<(Color, Vec<(f64, f64)>)> = spec
        .series
        .iter()
        .map(|(color, s)| {
            (
                *color,
                s.iter()
                    .map(|(t, v)| (offset + t.num_milliseconds() as f64 / 1000.0, *v))
                    .collect(),
            )
        })
        .collect();
    let datasets: Vec<Dataset> = owned
        .iter()
        .map(|(color, points)| {
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(theme::fg(*color))
                .data(points)
        })
        .collect();
    let y_max = spec.y_max.max(f64::EPSILON);
    frame.render_widget(
        Chart::new(datasets)
            .x_axis(Axis::default().bounds([0.0, span]))
            .y_axis(Axis::default().bounds([0.0, y_max])),
        plot,
    );
    if !axes {
        return;
    }
    let buf = frame.buffer_mut();
    let last = plot.height - 1;
    for (row, value) in [(0, y_max), (last / 2, y_max / 2.0), (last, 0.0)] {
        let label = (spec.y_label)(value);
        buf.set_string(
            inner.x,
            plot.y + row,
            format!("{label:>width$}", width = Y_LABELS as usize - 1),
            theme::muted(),
        );
    }
    let y = plot.bottom();
    let [start, mid] = x_labels(m.range);
    let mut taken: Vec<(u16, u16)> = Vec::new();
    let mut put = |buf: &mut ratatui::buffer::Buffer, col: u16, text: &str| {
        let width = text.chars().count() as u16;
        if col + width > plot.width
            || taken
                .iter()
                .any(|&(a, b)| col < b + 1 && a < col + width + 1)
        {
            return;
        }
        taken.push((col, col + width));
        buf.set_string(plot.x + col, y, text, theme::muted());
    };
    put(buf, 0, start);
    put(buf, plot.width.saturating_sub(3), "now");
    put(
        buf,
        (plot.width / 2).saturating_sub(mid.len() as u16 / 2),
        mid,
    );
    for (col, number) in deploys.iter().rev() {
        put(buf, col.saturating_sub(1), &format!("#{number}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_round_up_to_one_two_or_five() {
        assert_eq!(nice_ceil(0.0), 1.0);
        assert_eq!(nice_ceil(1.2), 2.0);
        assert_eq!(nice_ceil(48.0), 50.0);
        assert_eq!(nice_ceil(50.0), 50.0);
        assert_eq!(nice_ceil(230.0), 500.0);
        assert_eq!(nice_ceil(1_200_000.0), 2_000_000.0);
        let mib = 1024.0 * 1024.0;
        assert_eq!(nice_ceil_binary(8.5 * mib), 10.0 * mib);
        assert_eq!(nice_ceil_binary(300.0 * mib), 500.0 * mib);
        assert_eq!(nice_ceil_binary(514.0 * 1024.0), mib);
    }

    #[test]
    fn sizes_read_like_the_design() {
        assert_eq!(mib(212.0 * 1024.0 * 1024.0), "212 MiB");
        assert_eq!(rate(1_234_000.0), "1.2 MB/s");
        assert_eq!(rate(340_000.0), "340 kB/s");
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(short(512.0 * 1024.0 * 1024.0, 1024.0), "512M");
        assert_eq!(short(40_000.0, 1000.0), "40k");
        assert_eq!(short(0.0, 1000.0), "0");
    }
}
