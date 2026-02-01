use std::collections::VecDeque;

use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

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
            self.scroll_offset = self.scroll_offset.saturating_add(delta.unsigned_abs() as usize);
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
