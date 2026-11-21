//! Mouse support: click targets registered while drawing, wheel scrolling, and
//! drag-to-select with copy to the system clipboard through OSC 52.

use crate::app::*;
use crate::keys;
use crate::theme;
use base64::Engine as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::cell::{Cell, RefCell};
use std::io::Write as _;
use std::time::{Duration, Instant};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const WHEEL_LINES: isize = 3;

/// What a click on a region of the last frame means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Behaves like pressing the key, e.g. a hint in the key bar.
    Key(KeyCode, KeyModifiers),
    /// Swallows clicks, e.g. the backdrop behind a dialog.
    Block,
    Pane(Pane),
    OpenSettings,
    OpenServers,
    SidebarServer(usize),
    Filter,
    Search,
    Side(usize),
    Fold(usize),
    Service(usize),
    Activity(usize),
    Tab(Tab),
    /// A row in the list of the current service tab.
    Row(usize),
    DeployRow(usize),
    Pipeline(common::StepKind),
    SetupProxy(usize),
    Server(usize),
    /// A saved context on the Settings screen.
    Context(usize),
    ContextUrl(usize),
    Domain(usize),
    /// A scrollable log view.
    Logs,
    FormField(usize),
    FormChoice(usize, usize),
    PaletteRow(usize),
    PaletteInput,
    List(List),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    Activity(usize),
    Deployments,
    Pipeline(usize),
    Service,
    Servers,
    Contexts,
    Palette,
    Form,
}

thread_local! {
    static HITS: RefCell<Vec<(Rect, Hit)>> = const { RefCell::new(Vec::new()) };
    static MODAL: Cell<Option<Rect>> = const { Cell::new(None) };
}

/// Forget the targets of the previous frame; called at the start of every draw.
pub fn reset() {
    HITS.with_borrow_mut(Vec::clear);
    MODAL.set(None);
}

pub fn modal(rect: Rect) {
    MODAL.set(Some(rect));
}

pub fn finish_frame(mouse: &MouseState) {
    HITS.with_borrow_mut(|hits| std::mem::swap(hits, &mut mouse.targets.borrow_mut()));
    mouse.modal.set(MODAL.get());
}

/// Register a click target. Later registrations win, so overlays register after the screen below.
pub fn hit(rect: Rect, hit: Hit) {
    if rect.width > 0 && rect.height > 0 {
        HITS.with_borrow_mut(|hits| hits.push((rect, hit)));
    }
}

fn target_at(mouse: &MouseState, pos: Position) -> Option<(Rect, Hit)> {
    mouse
        .targets
        .borrow()
        .iter()
        .rev()
        .find(|(r, _)| r.contains(pos))
        .copied()
}

fn scroll_at(mouse: &MouseState, pos: Position) -> Option<Hit> {
    mouse.targets.borrow().iter().rev().find_map(|(r, h)| {
        (r.contains(pos) && matches!(h, Hit::Block | Hit::List(_) | Hit::Pane(_) | Hit::Logs))
            .then_some(*h)
    })
}

#[cfg(test)]
pub fn hit_at(mouse: &MouseState, col: u16, row: u16) -> Option<Hit> {
    target_at(mouse, Position::new(col, row)).map(|(_, hit)| hit)
}

#[cfg(test)]
pub fn targets(mouse: &MouseState) -> Vec<(Rect, Hit)> {
    mouse.targets.borrow().clone()
}

#[cfg(test)]
pub fn modal_bounds(mouse: &MouseState) -> Option<Rect> {
    mouse.modal.get()
}

pub fn span_target(area: Rect, offset: usize, width: usize, target: Hit) {
    if offset < area.width as usize {
        hit(
            Rect::new(
                area.x + offset as u16,
                area.y,
                width.min(area.width as usize - offset) as u16,
                area.height.min(1),
            ),
            target,
        );
    }
}

pub fn key_target(rect: Rect, label: &str) {
    if label == "↑↓" {
        span_target(rect, 0, 1, Hit::Key(KeyCode::Up, KeyModifiers::NONE));
        span_target(
            rect,
            1,
            rect.width.saturating_sub(1) as usize,
            Hit::Key(KeyCode::Down, KeyModifiers::NONE),
        );
    } else if let Some((code, mods)) = key_for_label(label) {
        hit(rect, Hit::Key(code, mods));
    }
}

pub fn hints_width(hints: &[(&str, &str)]) -> usize {
    hints
        .iter()
        .map(|(key, label)| Span::raw(*key).width() + 1 + Span::raw(*label).width())
        .sum::<usize>()
        + hints.len().saturating_sub(1) * 3
}

pub fn hints(frame: &mut Frame, area: Rect, hints: &[(&str, &str)], align: Alignment) {
    let area = area.intersection(frame.area());
    if area.is_empty() {
        return;
    }
    let mut offset = match align {
        Alignment::Right => (area.width as usize).saturating_sub(hints_width(hints)),
        Alignment::Center => (area.width as usize).saturating_sub(hints_width(hints)) / 2,
        Alignment::Left => 0,
    };
    for (key, label) in hints {
        let line = Line::from(vec![
            Span::styled(
                *key,
                if key_for_label(key).is_some() || *key == "↑↓" {
                    theme::accent()
                } else {
                    theme::muted()
                },
            ),
            Span::styled(format!(" {label}"), theme::muted()),
        ]);
        let width = line.width();
        if offset + width > area.width as usize {
            break;
        }
        let rect = Rect::new(area.x + offset as u16, area.y, width as u16, 1);
        frame.render_widget(Paragraph::new(line), rect);
        key_target(rect, key);
        offset += width + 3;
    }
}

/// Maps the label of a key hint ("d", "⏎", "esc", "ctrl+s") to the key it stands for.
pub fn key_for_label(label: &str) -> Option<(KeyCode, KeyModifiers)> {
    let none = KeyModifiers::NONE;
    Some(match label {
        "⏎" => (KeyCode::Enter, none),
        "tab" => (KeyCode::Tab, none),
        "⇧tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
        "esc" => (KeyCode::Esc, none),
        "↑" => (KeyCode::Up, none),
        "↓" => (KeyCode::Down, none),
        "←" => (KeyCode::Left, none),
        "→" => (KeyCode::Right, none),
        "ctrl+s" => (KeyCode::Char('s'), KeyModifiers::CONTROL),
        "ctrl+r" => (KeyCode::Char('r'), KeyModifiers::CONTROL),
        "ctrl+k" => (KeyCode::Char('k'), KeyModifiers::CONTROL),
        "y" => (KeyCode::Char('y'), none),
        _ => {
            let mut chars = label.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_graphic() => (KeyCode::Char(c), none),
                _ => return None,
            }
        }
    })
}

/// A drag selection in screen cells. It is a rectangle between anchor and cursor, so dragging
/// down a panel copies that panel's lines without the panes beside it.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub anchor: (u16, u16),
    pub cursor: (u16, u16),
    pub dragging: bool,
    /// Set on release; the next frame copies the text and clears this.
    pub copy_pending: bool,
}

impl Selection {
    /// Top-left and bottom-right corners, inclusive.
    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let (ax, ay) = self.anchor;
        let (cx, cy) = self.cursor;
        ((ax.min(cx), ay.min(cy)), (ax.max(cx), ay.max(cy)))
    }

    fn contains(&self, x: u16, y: u16) -> bool {
        let ((sx, sy), (ex, ey)) = self.ordered();
        (sx..=ex).contains(&x) && (sy..=ey).contains(&y)
    }
}

pub struct MouseState {
    pub selection: Option<Selection>,
    targets: RefCell<Vec<(Rect, Hit)>>,
    modal: Cell<Option<Rect>>,
    pointer: Option<Position>,
    pressed: Option<(Hit, Option<Rect>)>,
    last_click: Option<(Instant, Hit)>,
}

impl MouseState {
    pub fn new() -> Self {
        Self {
            selection: None,
            targets: RefCell::default(),
            modal: Cell::default(),
            pointer: None,
            pressed: None,
            last_click: None,
        }
    }

    pub fn cancel_gesture(&mut self) {
        self.selection = None;
        self.pressed = None;
        self.last_click = None;
    }
}

fn hover_at(mouse: &MouseState, pos: Position) -> Option<(Rect, Hit)> {
    target_at(mouse, pos).filter(|(rect, target)| {
        rect.height == 1
            && !matches!(
                target,
                Hit::Block | Hit::Logs | Hit::Pane(_) | Hit::List(_) | Hit::PaletteInput
            )
    })
}

pub fn paint_hover(buf: &mut Buffer, mouse: &MouseState) {
    let Some((rect, target)) = mouse.pointer.and_then(|pos| hover_at(mouse, pos)) else {
        return;
    };
    let row = matches!(
        target,
        Hit::Side(_)
            | Hit::Service(_)
            | Hit::Activity(_)
            | Hit::Row(_)
            | Hit::DeployRow(_)
            | Hit::Pipeline(_)
            | Hit::Server(_)
            | Hit::SidebarServer(_)
            | Hit::Context(_)
            | Hit::PaletteRow(_)
    );
    let rect = rect.intersection(buf.area);
    for y in rect.top()..rect.bottom() {
        let selected = row && (rect.left()..rect.right()).any(|x| buf[(x, y)].bg == theme::SURFACE);
        for x in rect.left()..rect.right() {
            let cell = &mut buf[(x, y)];
            if row && !selected {
                cell.set_bg(theme::SURFACE);
            } else if cell.symbol() != " " {
                cell.modifier.insert(Modifier::UNDERLINED);
            }
        }
    }
}

/// Highlight the selected cells; drawn last so it covers overlays too.
pub fn paint_selection(buf: &mut Buffer, selection: Option<&Selection>) {
    let Some(sel) = selection.filter(|s| s.dragging || s.copy_pending) else {
        return;
    };
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if sel.contains(x, y) {
                let cell = &mut buf[(x, y)];
                cell.set_bg(theme::ACCENT);
                cell.set_fg(theme::BG);
            }
        }
    }
}

/// The selected text, one line per row with trailing blanks trimmed.
pub fn selected_text(buf: &Buffer, sel: &Selection) -> String {
    let area = buf.area;
    let ((_, sy), (_, ey)) = sel.ordered();
    let mut lines = Vec::new();
    for y in sy..=ey.min(area.bottom().saturating_sub(1)) {
        let mut line = String::new();
        for x in area.left()..area.right() {
            if sel.contains(x, y) {
                line.push_str(buf[(x, y)].symbol());
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

/// Put text on the clipboard of the terminal the user sits at, also over SSH.
pub fn copy_to_clipboard(text: &str) {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

pub fn handle(app: &mut App, ev: MouseEvent) -> bool {
    let (col, row) = (ev.column, ev.row);
    let pos = Position::new(col, row);
    let target = target_at(&app.mouse, pos).map(|(_, target)| target);
    match ev.kind {
        MouseEventKind::Moved => {
            let old = app
                .mouse
                .pointer
                .replace(pos)
                .and_then(|pos| hover_at(&app.mouse, pos));
            return old != hover_at(&app.mouse, pos);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            app.mouse.pointer = Some(pos);
            let modal = app.mouse.modal.get();
            app.mouse.pressed = Some((target.unwrap_or(Hit::Block), modal));
            app.mouse.selection =
                modal
                    .is_none_or(|rect| rect.contains(pos))
                    .then_some(Selection {
                        anchor: (col, row),
                        cursor: (col, row),
                        dragging: false,
                        copy_pending: false,
                    });
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(sel) = app.mouse.selection.as_mut() {
                sel.cursor = app
                    .mouse
                    .modal
                    .get()
                    .filter(|rect| !rect.is_empty())
                    .map_or((col, row), |rect| {
                        (
                            col.clamp(rect.left(), rect.right() - 1),
                            row.clamp(rect.top(), rect.bottom() - 1),
                        )
                    });
                sel.dragging |= sel.cursor != sel.anchor;
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.mouse.pointer = Some(pos);
            let Some((pressed, modal)) = app.mouse.pressed.take() else {
                return false;
            };
            if modal != app.mouse.modal.get() {
                app.mouse.cancel_gesture();
                return true;
            }
            match app.mouse.selection.as_mut() {
                Some(sel) if sel.dragging => {
                    sel.dragging = false;
                    sel.copy_pending = true;
                    app.mouse.last_click = None;
                }
                _ => {
                    app.mouse.selection = None;
                    let previous = app.mouse.last_click.take();
                    if let Some(hit) = target.filter(|&hit| hit == pressed) {
                        let double = previous
                            .is_some_and(|(at, prev)| prev == hit && at.elapsed() < DOUBLE_CLICK);
                        app.mouse.last_click = (!double).then(|| (Instant::now(), hit));
                        click(app, hit, double);
                    }
                }
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            app.mouse.pointer = Some(pos);
            app.mouse.last_click = None;
            let dir = if ev.kind == MouseEventKind::ScrollUp {
                -1
            } else {
                1
            };
            scroll(app, scroll_at(&app.mouse, pos), dir);
        }
        _ => return false,
    }
    true
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    keys::handle(app, KeyEvent::new(code, modifiers));
}

fn click(app: &mut App, hit: Hit, double: bool) {
    match hit {
        Hit::Key(code, mods) => {
            if matches!(app.overlay, Some(Overlay::Help)) {
                app.overlay = None;
                if matches!(code, KeyCode::Esc | KeyCode::Char('?')) {
                    return;
                }
            }
            press(app, code, mods);
        }
        Hit::Block | Hit::Logs | Hit::List(_) | Hit::PaletteInput => {}
        Hit::OpenSettings => {
            keys::finish_text_input(app);
            app.open_settings();
        }
        Hit::OpenServers | Hit::SidebarServer(_) => {
            keys::finish_text_input(app);
            if let Hit::SidebarServer(i) = hit {
                app.server_row = i;
            }
            app.open_servers();
        }
        Hit::Filter => {
            app.overview.pane = Pane::Services;
            app.overview.filtering = true;
        }
        Hit::Search => {
            if let Some(view) = app.service.as_mut() {
                view.logs.searching = true;
            }
        }
        Hit::Pane(pane) => {
            if pane == Pane::Sidebar {
                keys::finish_text_input(app);
            }
            app.overview.pane = pane;
        }
        Hit::Side(i) | Hit::Fold(i) => {
            keys::finish_text_input(app);
            app.overview.pane = Pane::Sidebar;
            let delta = i as isize - app.overview.side as isize;
            if delta != 0 {
                app.move_side(delta);
            }
            if double || matches!(hit, Hit::Fold(_)) {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::Service(i) => {
            keys::finish_text_input(app);
            app.overview.pane = Pane::Services;
            let delta = i as isize - app.overview.row as isize;
            if delta != 0 {
                app.move_row(delta);
            }
            if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::Activity(i) => {
            if let Some(id) = app.activity.get(i).and_then(|a| a.service_id.clone()) {
                keys::finish_text_input(app);
                app.open_service(&id, Tab::Overview);
            }
        }
        Hit::Tab(tab) => {
            keys::finish_text_input(app);
            if app.service.as_ref().is_some_and(|v| v.tab != tab) {
                app.set_tab(tab);
            }
        }
        Hit::Row(i) => {
            if let Some(view) = app.service.as_mut() {
                view.row = i;
            }
            if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::DeployRow(i) => {
            if let Some(view) = app.service.as_mut() {
                view.deployment_row = i;
                if double && let Some(deployment) = view.deployments.get(i).cloned() {
                    keys::finish_text_input(app);
                    app.open_deploy(deployment);
                }
            }
        }
        Hit::Pipeline(kind) => {
            if let Some(view) = app.deploy.as_mut()
                && !view.logs.jump_to_step(kind)
            {
                app.toast(format!("no build output for {}", kind.as_str()));
            }
        }
        Hit::Server(i) | Hit::SetupProxy(i) => {
            app.server_row = i;
            if matches!(hit, Hit::SetupProxy(_)) {
                press(app, KeyCode::Char('p'), KeyModifiers::NONE);
            } else if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::Context(i) => {
            app.settings.row = i;
            if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::ContextUrl(i) => {
            if let Some((_, context)) = app.settings.contexts.contexts.iter().nth(i) {
                app.settings.row = i;
                app.copy(&context.url.clone(), "context URL");
            }
        }
        Hit::Domain(i) => {
            let detail = if app.screen == Screen::Overview {
                app.selected_service().and_then(|s| app.details.get(&s.id))
            } else {
                app.service.as_ref().and_then(|v| v.detail.as_ref())
            };
            if let Some(detail) = detail
                && let Some(domain) = detail.domains.get(i)
            {
                app.copy(&keys::domain_url(domain), "domain URL");
            }
        }
        Hit::FormField(i) => {
            if let Some(Overlay::Form(form)) = app.overlay.as_mut() {
                form.click(i, None);
            }
        }
        Hit::FormChoice(i, choice) => {
            if let Some(Overlay::Form(form)) = app.overlay.as_mut() {
                form.click(i, Some(choice));
            }
        }
        Hit::PaletteRow(i) => {
            if let Some(Overlay::Palette(p)) = app.overlay.as_mut() {
                p.row = i;
            }
            if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
    }
}

fn scroll(app: &mut App, hit: Option<Hit>, dir: isize) {
    let viewport = app.log_viewport.get();
    match hit {
        Some(Hit::Logs) => {
            let logs = match app.screen {
                Screen::Deploy => app.deploy.as_mut().map(|v| &mut v.logs),
                _ => app.service.as_mut().map(|v| &mut v.logs),
            };
            if let Some(logs) = logs {
                logs.scroll(-dir * WHEEL_LINES, viewport);
            }
        }
        Some(Hit::Pane(Pane::Sidebar)) => {
            app.overview.pane = Pane::Sidebar;
            app.move_side(dir);
        }
        Some(Hit::Pane(Pane::Services)) => {
            app.overview.pane = Pane::Services;
            app.move_row(dir);
        }
        Some(Hit::List(List::Activity(height))) => {
            app.overview.activity_offset = app
                .overview
                .activity_offset
                .saturating_add_signed(dir)
                .min(app.activity.len().saturating_sub(height));
        }
        Some(Hit::List(List::Pipeline(height))) => {
            if let Some(view) = app.deploy.as_mut() {
                view.pipeline_offset = view
                    .pipeline_offset
                    .saturating_add_signed(dir)
                    .min(view.deployment.steps.len().saturating_sub(height));
            }
        }
        Some(Hit::List(List::Deployments)) => {
            if let Some(view) = app.service.as_mut() {
                view.deployment_row = view
                    .deployment_row
                    .saturating_add_signed(dir)
                    .min(view.deployments.len().saturating_sub(1));
            }
        }
        Some(Hit::List(_)) => {
            let code = if dir < 0 { KeyCode::Up } else { KeyCode::Down };
            press(app, code, KeyModifiers::NONE);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(anchor: (u16, u16), cursor: (u16, u16)) -> Selection {
        Selection {
            anchor,
            cursor,
            dragging: false,
            copy_pending: true,
        }
    }

    #[test]
    fn selection_is_a_rectangle_in_any_drag_direction() {
        let s = sel((5, 3), (2, 1));
        assert!(s.contains(2, 1));
        assert!(s.contains(5, 3));
        assert!(s.contains(3, 2));
        assert!(!s.contains(1, 2));
        assert!(!s.contains(6, 2));
        assert!(!s.contains(3, 4));
    }

    #[test]
    fn selected_text_trims_each_line() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 2));
        buf.set_string(0, 0, "hello", ratatui::style::Style::new());
        buf.set_string(0, 1, "world", ratatui::style::Style::new());
        assert_eq!(selected_text(&buf, &sel((1, 0), (3, 1))), "ell\norl");
        assert_eq!(selected_text(&buf, &sel((0, 0), (7, 0))), "hello");
    }

    #[test]
    fn key_labels_map_to_keys() {
        assert_eq!(
            key_for_label("d"),
            Some((KeyCode::Char('d'), KeyModifiers::NONE))
        );
        assert_eq!(
            key_for_label("⏎"),
            Some((KeyCode::Enter, KeyModifiers::NONE))
        );
        assert_eq!(
            key_for_label("ctrl+s"),
            Some((KeyCode::Char('s'), KeyModifiers::CONTROL))
        );
        assert_eq!(key_for_label("↑↓"), None);
    }
}
