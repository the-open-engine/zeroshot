//! Centralized color palette and style definitions.
//!
//! All render functions should import styles from here instead of using
//! inline `Style::default().fg(...)`. Based on Catppuccin Mocha palette.

use ratatui::style::{Color, Modifier, Style};

// ── Base palette ────────────────────────────────────────────────────────────

pub const ACCENT: Color = Color::Rgb(137, 180, 250); // #89b4fa blue
pub const ACCENT2: Color = Color::Rgb(166, 227, 161); // #a6e3a1 green
pub const FG_PRIMARY: Color = Color::Rgb(205, 214, 244); // #cdd6f4
pub const FG_DIM: Color = Color::DarkGray;
pub const FG_MUTED: Color = Color::Rgb(108, 112, 134); // #6c7086
pub const SURFACE: Color = Color::Rgb(30, 30, 46); // #1e1e2e
pub const FOCUS_BORDER: Color = ACCENT;
pub const UNFOCUS_BORDER: Color = Color::Rgb(69, 71, 90); // #45475a

// ── Status colors ───────────────────────────────────────────────────────────

pub const STATUS_RUNNING: Color = Color::Green;
pub const STATUS_DONE: Color = Color::Rgb(166, 227, 161); // #a6e3a1
pub const STATUS_ERROR: Color = Color::Rgb(243, 139, 168); // #f38ba8
pub const STATUS_PENDING: Color = Color::Yellow;
pub const STATUS_IDLE: Color = Color::DarkGray;

// ── Agent colors (rotating) ────────────────────────────────────────────────

const AGENT_COLORS: [Color; 6] = [
    Color::Rgb(137, 180, 250), // blue
    Color::Rgb(166, 227, 161), // green
    Color::Rgb(249, 226, 175), // yellow #f9e2af
    Color::Rgb(203, 166, 247), // mauve  #cba6f7
    Color::Rgb(148, 226, 213), // teal   #94e2d5
    Color::Rgb(242, 205, 205), // flamingo #f2cdcd
];

/// Get a color for an agent by hashing its ID to an index.
pub fn agent_color(agent_id: &str) -> Color {
    let hash = agent_id.bytes().fold(0u32, |acc, b| acc.wrapping_add(b as u32));
    AGENT_COLORS[hash as usize % AGENT_COLORS.len()]
}

// ── Pre-built styles ────────────────────────────────────────────────────────

/// Logo / branding text.
pub fn logo_style() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Screen title in the header.
pub fn title_style() -> Style {
    Style::default()
        .fg(FG_PRIMARY)
        .add_modifier(Modifier::BOLD)
}

/// Hint / secondary text.
pub fn dim_style() -> Style {
    Style::default().fg(FG_DIM)
}

/// Muted / disabled text.
pub fn muted_style() -> Style {
    Style::default().fg(FG_MUTED)
}

/// Keyboard shortcut key label (e.g., "Enter", "Esc").
pub fn key_style() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Keyboard shortcut description.
pub fn key_desc_style() -> Style {
    Style::default().fg(FG_DIM)
}

/// Focused pane border.
pub fn focus_border_style() -> Style {
    Style::default()
        .fg(FOCUS_BORDER)
        .add_modifier(Modifier::BOLD)
}

/// Unfocused pane border.
pub fn unfocus_border_style() -> Style {
    Style::default().fg(UNFOCUS_BORDER)
}

/// Selected row in a list/table (accent bg, dark fg).
pub fn selected_style() -> Style {
    Style::default()
        .fg(SURFACE)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

/// Backend status style by connection state.
pub fn backend_connected_style() -> Style {
    Style::default().fg(STATUS_RUNNING)
}

pub fn backend_error_style() -> Style {
    Style::default().fg(STATUS_ERROR)
}

/// Return a style for a cluster state string.
pub fn status_style(state: &str) -> Style {
    match state {
        "running" | "active" => Style::default().fg(STATUS_RUNNING),
        "done" | "completed" | "complete" => Style::default().fg(STATUS_DONE),
        "error" | "failed" => Style::default().fg(STATUS_ERROR),
        "pending" | "starting" | "queued" => Style::default().fg(STATUS_PENDING),
        "stopped" | "idle" => Style::default().fg(STATUS_IDLE),
        _ => Style::default().fg(FG_DIM),
    }
}

/// Style for a "done" row (entire row dimmed).
pub fn done_row_style() -> Style {
    Style::default().fg(FG_MUTED)
}

/// Table header style.
pub fn table_header_style() -> Style {
    Style::default()
        .fg(FG_DIM)
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::UNDERLINED)
}

/// Toast styles by level.
pub fn toast_success_style() -> Style {
    Style::default().fg(ACCENT2)
}

pub fn toast_error_style() -> Style {
    Style::default().fg(STATUS_ERROR)
}

pub fn toast_info_style() -> Style {
    Style::default().fg(FG_DIM)
}
