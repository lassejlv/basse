//! Palette from the Paper design: a harbor at night under sodium lamps.

use ratatui::style::{Color, Style};

pub const BG: Color = Color::Rgb(0x0E, 0x11, 0x13);
pub const SURFACE: Color = Color::Rgb(0x16, 0x1A, 0x1D);
pub const LINE: Color = Color::Rgb(0x2A, 0x30, 0x35);
pub const TEXT: Color = Color::Rgb(0xD8, 0xDE, 0xE3);
pub const MUTED: Color = Color::Rgb(0x7C, 0x87, 0x8F);
pub const ACCENT: Color = Color::Rgb(0xF5, 0xA5, 0x24);
pub const OK: Color = Color::Rgb(0x4F, 0xBF, 0x7F);
pub const INFO: Color = Color::Rgb(0x6C, 0xA6, 0xE0);
pub const ERR: Color = Color::Rgb(0xE5, 0x53, 0x4B);

pub fn text() -> Style {
    Style::new().fg(TEXT)
}

pub fn muted() -> Style {
    Style::new().fg(MUTED)
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

/// Selected row: a raised hull background; the caller colors the name in accent.
pub fn selected() -> Style {
    Style::new().bg(SURFACE)
}

/// Filled chip for the active choice in a form, like a lit button.
pub fn chip() -> Style {
    Style::new().fg(BG).bg(ACCENT)
}
