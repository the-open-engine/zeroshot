use ratatui::style::Style;

use crate::app::{ToastLevel, ToastState};
use crate::ui::theme;

/// Format toast for inline display in the status bar.
/// Returns (text, style) or None if no toast.
pub fn format_inline(toast: Option<&ToastState>) -> Option<(String, Style)> {
    let toast = toast?;
    let (prefix, style) = match toast.level {
        ToastLevel::Info => ("\u{2139}", theme::toast_info_style()), // ℹ
        ToastLevel::Success => ("\u{2713}", theme::toast_success_style()), // ✓
        ToastLevel::Error => ("\u{2717}", theme::toast_error_style()), // ✗
    };
    let first_line = toast.message.lines().next().unwrap_or("");
    let msg = format!("{prefix} {}", truncate_toast_line(first_line));
    Some((msg, style))
}

fn truncate_toast_line(line: &str) -> String {
    const MAX_LEN: usize = 40;
    const TRUNC_LEN: usize = 37;
    if line.chars().count() <= MAX_LEN {
        return line.to_string();
    }
    let mut out = String::new();
    for ch in line.chars().take(TRUNC_LEN) {
        out.push(ch);
    }
    out.push_str("...");
    out
}
