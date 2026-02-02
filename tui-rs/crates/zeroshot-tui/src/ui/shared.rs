use std::collections::VecDeque;

use ratatui::layout::Alignment;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::ui::theme;

// ── ScrollableBuffer ──────────────────────────────────────────────────────────

/// A capped VecDeque with scroll offset tracking.
///
/// Used by cluster logs, timeline events, and agent logs to manage
/// scrollable content with a maximum capacity.
#[derive(Debug, Clone)]
pub struct ScrollableBuffer<T> {
    pub items: VecDeque<T>,
    pub scroll_offset: usize,
    max_capacity: usize,
}

impl<T> ScrollableBuffer<T> {
    pub fn new(max_capacity: usize) -> Self {
        Self {
            items: VecDeque::new(),
            scroll_offset: 0,
            max_capacity,
        }
    }

    pub fn push_many(&mut self, items: impl IntoIterator<Item = T>) {
        let before = self.items.len();
        self.items.extend(items);
        let added = self.items.len() - before;
        self.adjust_scroll_on_append(added);
        let dropped = self.trim();
        self.adjust_scroll_on_trim(dropped);
        self.clamp_scroll();
    }

    pub fn move_scroll(&mut self, delta: i32) {
        let len = self.items.len();
        if len == 0 {
            self.scroll_offset = 0;
            return;
        }
        if delta < 0 {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(delta.unsigned_abs() as usize);
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(delta as usize);
        }
        self.clamp_scroll();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn adjust_scroll_on_append(&mut self, added: usize) {
        if self.scroll_offset > 0 {
            self.scroll_offset = self.scroll_offset.saturating_add(added);
        }
    }

    fn adjust_scroll_on_trim(&mut self, dropped: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(dropped);
    }

    fn clamp_scroll(&mut self) {
        let max_offset = self.items.len().saturating_sub(1);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    fn trim(&mut self) -> usize {
        if self.items.len() <= self.max_capacity {
            return 0;
        }
        let mut dropped = 0usize;
        while self.items.len() > self.max_capacity {
            self.items.pop_front();
            dropped += 1;
        }
        dropped
    }
}

// ── TimeIndexedBuffer ────────────────────────────────────────────────────────

pub trait HasTimestamp {
    fn timestamp_ms(&self) -> i64;
}

/// A capped, time-indexed buffer with stable insertion ordering.
///
/// Optimized for windowed reads by timestamp while maintaining bounded memory.
#[derive(Debug, Clone)]
pub struct TimeIndexedBuffer<T: HasTimestamp> {
    items: VecDeque<T>,
    max_capacity: usize,
}

impl<T: HasTimestamp> TimeIndexedBuffer<T> {
    pub fn new(max_capacity: usize) -> Self {
        Self {
            items: VecDeque::new(),
            max_capacity,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn push_many(&mut self, items: impl IntoIterator<Item = T>) {
        self.items.extend(items);
        self.trim();
    }

    pub fn window(&self, t_ms: i64, window_ms: i64) -> Vec<&T> {
        if self.items.is_empty() {
            return Vec::new();
        }
        let window_ms = window_ms.max(0);
        let start = t_ms.saturating_sub(window_ms);
        let end = t_ms;
        let lower = self.lower_bound(start);
        let upper = self.upper_bound(end);
        let mut out = Vec::with_capacity(upper.saturating_sub(lower));
        for idx in lower..upper {
            if let Some(item) = self.items.get(idx) {
                out.push(item);
            }
        }
        out
    }

    pub fn latest(&self, n: usize) -> Vec<&T> {
        if n == 0 || self.items.is_empty() {
            return Vec::new();
        }
        let len = self.items.len();
        let start = len.saturating_sub(n);
        let mut out = Vec::with_capacity(len - start);
        for idx in start..len {
            if let Some(item) = self.items.get(idx) {
                out.push(item);
            }
        }
        out
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    pub fn iter_rev(&self) -> impl Iterator<Item = &T> {
        self.items.iter().rev()
    }

    fn lower_bound(&self, target: i64) -> usize {
        let mut left = 0usize;
        let mut right = self.items.len();
        while left < right {
            let mid = left + (right - left) / 2;
            let Some(item) = self.items.get(mid) else {
                break;
            };
            if item.timestamp_ms() < target {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        left
    }

    fn upper_bound(&self, target: i64) -> usize {
        let mut left = 0usize;
        let mut right = self.items.len();
        while left < right {
            let mid = left + (right - left) / 2;
            let Some(item) = self.items.get(mid) else {
                break;
            };
            if item.timestamp_ms() <= target {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        left
    }

    fn trim(&mut self) {
        while self.items.len() > self.max_capacity {
            self.items.pop_front();
        }
    }
}

// ── InputState ────────────────────────────────────────────────────────────────

/// Character-indexed cursor input state, shared between agent guidance,
/// launcher input, and command bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputState {
    pub input: String,
    pub cursor: usize,
}

impl InputState {
    pub fn insert_char(&mut self, ch: char) {
        let idx = self.byte_index(self.cursor);
        self.input.insert(idx, ch);
        self.cursor = self.cursor.saturating_add(1);
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        if start < end {
            self.input.replace_range(start..end, "");
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn delete(&mut self) {
        let len = self.len_chars();
        if self.cursor >= len {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        if start < end {
            self.input.replace_range(start..end, "");
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        let len = self.len_chars();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.len_chars();
    }

    pub fn clear(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    pub fn clamp_cursor(&mut self) {
        let len = self.len_chars();
        if self.cursor > len {
            self.cursor = len;
        }
    }

    fn len_chars(&self) -> usize {
        self.input.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }
        self.input
            .char_indices()
            .nth(char_index)
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| self.input.len())
    }
}

// ── pane_block ────────────────────────────────────────────────────────────────

/// Shared pane block with focus-dependent border styling.
pub fn pane_block<'a>(title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    if focused {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(theme::focus_border_style())
    } else {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(theme::unfocus_border_style())
    }
}

/// Builds a calm, centered empty-state card with optional detail + footer.
pub fn calm_empty_state<'a>(
    title: impl Into<Line<'a>>,
    headline: &'a str,
    detail: Option<&'a str>,
    footer: Option<&'a str>,
) -> Paragraph<'a> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(headline, theme::muted_style())));
    if let Some(detail) = detail {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(detail, theme::dim_style())));
    }
    if let Some(footer) = footer {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(footer, theme::dim_style())));
    }

    Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title(title))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct Sample {
        ts: i64,
        label: &'static str,
    }

    impl HasTimestamp for Sample {
        fn timestamp_ms(&self) -> i64 {
            self.ts
        }
    }

    #[test]
    fn time_indexed_buffer_window_returns_expected_items() {
        let mut buffer = TimeIndexedBuffer::new(10);
        buffer.push_many([
            Sample {
                ts: 100,
                label: "a",
            },
            Sample {
                ts: 110,
                label: "b",
            },
            Sample {
                ts: 120,
                label: "c",
            },
            Sample {
                ts: 130,
                label: "d",
            },
            Sample {
                ts: 140,
                label: "e",
            },
        ]);

        let window = buffer.window(130, 20);
        let labels: Vec<&str> = window.iter().map(|item| item.label).collect();
        assert_eq!(labels, vec!["b", "c", "d"]);
    }

    #[test]
    fn time_indexed_buffer_trims_to_capacity_preserving_order() {
        let mut buffer = TimeIndexedBuffer::new(3);
        buffer.push_many([
            Sample { ts: 1, label: "a" },
            Sample { ts: 2, label: "b" },
            Sample { ts: 3, label: "c" },
            Sample { ts: 4, label: "d" },
            Sample { ts: 5, label: "e" },
        ]);

        let latest = buffer.latest(10);
        let labels: Vec<&str> = latest.iter().map(|item| item.label).collect();
        assert_eq!(labels, vec!["c", "d", "e"]);
    }

    #[test]
    fn time_indexed_buffer_window_includes_equal_timestamps() {
        let mut buffer = TimeIndexedBuffer::new(10);
        buffer.push_many([
            Sample {
                ts: 100,
                label: "a",
            },
            Sample {
                ts: 100,
                label: "b",
            },
            Sample {
                ts: 100,
                label: "c",
            },
            Sample {
                ts: 110,
                label: "d",
            },
        ]);

        let window = buffer.window(100, 0);
        let labels: Vec<&str> = window.iter().map(|item| item.label).collect();
        assert_eq!(labels, vec!["a", "b", "c"]);
    }
}
