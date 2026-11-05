//! Every colour the TUI uses, by role. Charcoal surface, one foreground plus a dim step, plain
//! thin borders, and a single orange accent that marks identity and focus only. Status is always
//! a symbol plus a word, never colour alone.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

pub const SURFACE: Color = Color::Rgb(0x16, 0x16, 0x18);
pub const TEXT: Color = Color::Rgb(0xdc, 0xdc, 0xd8);
pub const TEXT_DIM: Color = Color::Rgb(0x80, 0x80, 0x7c);
pub const BORDER: Color = Color::Rgb(0x4a, 0x4a, 0x48);
pub const ACCENT: Color = Color::Rgb(0xe8, 0x82, 0x2a);
pub const STATUS_OK: Color = Color::Rgb(0x6f, 0xb3, 0x7f);
pub const STATUS_WARN: Color = Color::Rgb(0xd9, 0xc0, 0x5a);
pub const STATUS_FAIL: Color = Color::Rgb(0xd9, 0x6a, 0x6a);
pub const STATUS_UNKNOWN: Color = Color::Rgb(0x9a, 0x9a, 0x96);

pub fn base() -> Style {
    Style::new().fg(TEXT).bg(SURFACE)
}

pub fn dim() -> Style {
    base().fg(TEXT_DIM)
}

pub fn border() -> Style {
    base().fg(BORDER)
}

/// The active tab, the selected row, the focused field.
pub fn focus() -> Style {
    base().fg(ACCENT).add_modifier(Modifier::BOLD)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Unknown,
    /// Nothing to score against; not an error.
    Unscored,
}

impl Status {
    fn symbol(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warn => "!",
            Self::Fail => "✗",
            Self::Unknown => "?",
            Self::Unscored => "·",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Ok => STATUS_OK,
            Self::Warn => STATUS_WARN,
            Self::Fail => STATUS_FAIL,
            Self::Unknown | Self::Unscored => STATUS_UNKNOWN,
        }
    }
}

/// `✓ complete`, `✗ failed`, `? unpriced`: symbol, then the word.
pub fn status(status: Status, label: &str) -> Span<'static> {
    Span::styled(
        format!("{} {label}", status.symbol()),
        base().fg(status.color()),
    )
}
