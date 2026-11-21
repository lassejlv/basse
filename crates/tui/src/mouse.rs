//! Mouse support: click targets registered while drawing, wheel scrolling, and
//! drag-to-select with copy to the system clipboard through OSC 52.

use crate::app::*;
use crate::keys;
use crate::theme;
use base64::Engine as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use std::io::Write as _;
use std::sync::Mutex;
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
    Side(usize),
    Service(usize),
    Tab(Tab),
    /// A row in the list of the current service tab.
    Row(usize),
    Server(usize),
    /// A saved context on the Settings screen.
    Context(usize),
    /// A scrollable log view.
    Logs,
    FormField(usize),
    FormChoice(usize, usize),
    PaletteRow(usize),
    PickRow(usize),
    /// A row of the list on a full-screen list such as Secrets.
    Item(usize),
}

static HITS: Mutex<Vec<(Rect, Hit)>> = Mutex::new(Vec::new());

/// Forget the targets of the previous frame; called at the start of every draw.
pub fn reset() {
    HITS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Register a click target. Later registrations win, so overlays register after the screen below.
pub fn hit(rect: Rect, hit: Hit) {
    if rect.width > 0 && rect.height > 0 {
        HITS.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((rect, hit));
    }
}

fn hit_at(col: u16, row: u16) -> Option<Hit> {
    let pos = Position::new(col, row);
    HITS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, h)| *h)
}

/// Maps the label of a key hint ("d", "⏎", "esc", "ctrl+s") to the key it stands for.
pub fn key_for_label(label: &str) -> Option<(KeyCode, KeyModifiers)> {
    let none = KeyModifiers::NONE;
    Some(match label {
        "⏎" => (KeyCode::Enter, none),
        "tab" => (KeyCode::Tab, none),
        "⇧tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
        "esc" => (KeyCode::Esc, none),
        "ctrl+s" => (KeyCode::Char('s'), KeyModifiers::CONTROL),
        "ctrl+r" => (KeyCode::Char('r'), KeyModifiers::CONTROL),
        "y" => (KeyCode::Char('y'), none),
        _ => {
            let mut chars = label.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => (KeyCode::Char(c), none),
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
    last_click: Option<(Instant, Hit)>,
}

impl MouseState {
    pub fn new() -> Self {
        Self {
            selection: None,
            last_click: None,
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

pub fn handle(app: &mut App, ev: MouseEvent) {
    let (col, row) = (ev.column, ev.row);
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            app.mouse.selection = Some(Selection {
                anchor: (col, row),
                cursor: (col, row),
                dragging: false,
                copy_pending: false,
            });
            let Some(hit) = hit_at(col, row) else { return };
            let double = app
                .mouse
                .last_click
                .is_some_and(|(at, prev)| prev == hit && at.elapsed() < DOUBLE_CLICK);
            app.mouse.last_click = (!double).then(|| (Instant::now(), hit));
            click(app, hit, double);
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(sel) = app.mouse.selection.as_mut() {
                sel.cursor = (col, row);
                sel.dragging = sel.cursor != sel.anchor;
            }
        }
        MouseEventKind::Up(MouseButton::Left) => match app.mouse.selection.as_mut() {
            Some(sel) if sel.dragging => {
                sel.dragging = false;
                sel.copy_pending = true;
            }
            _ => app.mouse.selection = None,
        },
        MouseEventKind::ScrollUp => scroll(app, hit_at(col, row), -1),
        MouseEventKind::ScrollDown => scroll(app, hit_at(col, row), 1),
        _ => {}
    }
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    keys::handle(app, KeyEvent::new(code, modifiers));
}

fn click(app: &mut App, hit: Hit, double: bool) {
    match hit {
        Hit::Key(code, mods) => press(app, code, mods),
        Hit::Block | Hit::Logs => {}
        Hit::Pane(pane) => app.overview.pane = pane,
        Hit::Side(i) => {
            app.overview.pane = Pane::Sidebar;
            let delta = i as isize - app.overview.side as isize;
            if delta != 0 {
                app.move_side(delta);
            } else if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::Service(i) => {
            app.overview.pane = Pane::Services;
            let delta = i as isize - app.overview.row as isize;
            if delta != 0 {
                app.move_row(delta);
            }
            if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::Tab(tab) => app.set_tab(tab),
        Hit::Row(i) => {
            if let Some(view) = app.service.as_mut() {
                view.row = i;
            }
            if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::Server(i) => app.server_row = i,
        Hit::Context(i) => {
            app.settings.row = i;
            if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::FormField(i) => {
            if let Some(Overlay::Form(form)) = app.overlay.as_mut() {
                form.click(i, None);
                if form.fields[i].kind == crate::form::FieldKind::Pick && form.focus == i {
                    press(app, KeyCode::Enter, KeyModifiers::NONE);
                }
            }
        }
        Hit::PickRow(i) => {
            if let Some(Overlay::Picker(p)) = app.overlay.as_mut() {
                p.row = i;
            }
            if double {
                press(app, KeyCode::Enter, KeyModifiers::NONE);
            }
        }
        Hit::Item(i) => {
            if app.screen == Screen::Secrets {
                app.select_secret(i);
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
            press(app, KeyCode::Enter, KeyModifiers::NONE);
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
        Some(Hit::Side(_)) | Some(Hit::Pane(Pane::Sidebar)) => app.move_side(dir),
        Some(Hit::Service(_)) | Some(Hit::Pane(Pane::Services)) => app.move_row(dir),
        Some(Hit::Block) => {}
        _ => {
            let code = if dir < 0 { KeyCode::Up } else { KeyCode::Down };
            press(app, code, KeyModifiers::NONE);
        }
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
