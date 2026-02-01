use std::collections::VecDeque;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::protocol::{ClusterLogLine, GuidanceDeliveryResult};

pub const MAX_LOG_LINES: usize = 500;

#[derive(Debug, Clone)]
pub struct State {
    pub logs: VecDeque<ClusterLogLine>,
    pub log_scroll_offset: usize,
    pub log_drop_seq: u64,
    pub log_subscription: Option<String>,
    pub guidance_input: InputState,
    pub guidance_pending: bool,
    pub last_guidance: Option<GuidanceDeliveryResult>,
    pub last_guidance_error: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    SubmitGuidance,
    InsertChar(char),
    Backspace,
    Delete,
    MoveCursorLeft,
    MoveCursorRight,
    MoveCursorHome,
    MoveCursorEnd,
    ScrollLogs(i32),
}

impl State {
    pub fn push_log_lines(&mut self, mut lines: Vec<ClusterLogLine>, dropped_count: Option<i64>) {
        let mut added = 0usize;
        if let Some(count) = dropped_count {
            if count > 0 {
                let line = ClusterLogLine {
                    id: format!("dropped-{}", self.log_drop_seq),
                    timestamp: lines.first().map(|line| line.timestamp).unwrap_or(0),
                    text: format!("[dropped {} log lines]", count),
                    agent: None,
                    role: None,
                    sender: None,
                };
                self.log_drop_seq = self.log_drop_seq.saturating_add(1);
                self.logs.push_back(line);
                added += 1;
            }
        }

        added += lines.len();
        self.logs.extend(lines.drain(..));
        Self::adjust_scroll_on_append(&mut self.log_scroll_offset, added);
        let dropped = trim_vecdeque(&mut self.logs, MAX_LOG_LINES);
        Self::adjust_scroll_on_trim(&mut self.log_scroll_offset, dropped);
        Self::clamp_scroll(&mut self.log_scroll_offset, self.logs.len());
    }

    pub fn move_log_scroll(&mut self, delta: i32) {
        Self::move_scroll(&mut self.log_scroll_offset, delta, self.logs.len());
    }

    pub fn apply_guidance_result(&mut self, result: GuidanceDeliveryResult) {
        self.last_guidance = Some(result);
        self.last_guidance_error = None;
        self.guidance_pending = false;
        self.guidance_input.clear();
    }

    pub fn apply_guidance_error(&mut self, message: String) {
        self.last_guidance_error = Some(message);
        self.guidance_pending = false;
    }

    pub fn guidance_status_line(&self) -> String {
        if self.guidance_pending {
            return "Guidance: sending...".to_string();
        }
        if let Some(error) = &self.last_guidance_error {
            return format!("Guidance error: {error}");
        }
        if let Some(result) = &self.last_guidance {
            return format_guidance_result(result);
        }
        "Guidance: (no delivery yet)".to_string()
    }

    fn adjust_scroll_on_append(offset: &mut usize, added: usize) {
        if *offset > 0 {
            *offset = offset.saturating_add(added);
        }
    }

    fn adjust_scroll_on_trim(offset: &mut usize, dropped: usize) {
        *offset = offset.saturating_sub(dropped);
    }

    fn clamp_scroll(offset: &mut usize, len: usize) {
        let max_offset = len.saturating_sub(1);
        if *offset > max_offset {
            *offset = max_offset;
        }
    }

    fn move_scroll(offset: &mut usize, delta: i32, len: usize) {
        if len == 0 {
            *offset = 0;
            return;
        }
        if delta < 0 {
            *offset = offset.saturating_add(delta.abs() as usize);
        } else {
            *offset = offset.saturating_sub(delta as usize);
        }
        Self::clamp_scroll(offset, len);
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            logs: VecDeque::new(),
            log_scroll_offset: 0,
            log_drop_seq: 0,
            log_subscription: None,
            guidance_input: InputState::default(),
            guidance_pending: false,
            last_guidance: None,
            last_guidance_error: None,
            role: None,
            status: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
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

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &State,
    cluster_id: &str,
    agent_id: &str,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(frame, rows[0], state, cluster_id, agent_id);
    render_logs(frame, rows[1], state);
    render_guidance_status(frame, rows[2], state);
    render_guidance_input(frame, rows[3], state);
}

fn render_header(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &State,
    cluster_id: &str,
    agent_id: &str,
) {
    let role = state.role.as_deref().unwrap_or("unknown");
    let status = state.status.as_deref().unwrap_or("unknown");
    let lines = vec![
        Line::from(format!("Agent: {agent_id}")),
        Line::from(format!("Cluster: {cluster_id} | Role: {role}")),
        Line::from(format!("Status: {status}")),
    ];
    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}

fn render_logs(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let title = if state.log_scroll_offset > 0 {
        format!("Logs (scroll {})", state.log_scroll_offset)
    } else {
        "Logs".to_string()
    };
    let block = pane_block(title);
    let inner = block.inner(area);
    let height = inner.height as usize;
    let lines = if state.logs.is_empty() || height == 0 {
        vec![Line::from("(no logs yet)")]
    } else {
        let total = state.logs.len();
        let max_start = total.saturating_sub(height);
        let start = max_start.saturating_sub(state.log_scroll_offset.min(max_start));
        state
            .logs
            .iter()
            .skip(start)
            .take(height)
            .map(format_log_line)
            .map(Line::from)
            .collect()
    };
    let widget = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

fn render_guidance_status(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let lines = vec![Line::from(state.guidance_status_line())];
    let block = Block::default()
        .title("Guidance Status")
        .borders(Borders::ALL);
    let widget = Paragraph::new(lines).block(block);
    frame.render_widget(widget, area);
}

fn render_guidance_input(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let block = Block::default()
        .title("Guidance Input")
        .borders(Borders::ALL);
    let input = Paragraph::new(state.guidance_input.input.as_str()).block(block);
    frame.render_widget(input, area);

    if area.height > 2 && area.width > 2 {
        let max_x = area.x + area.width.saturating_sub(2);
        let cursor_x = area.x + 1 + state.guidance_input.cursor as u16;
        let cursor_x = cursor_x.min(max_x);
        let cursor_y = area.y + 1;
        frame.set_cursor(cursor_x, cursor_y);
    }
}

pub fn format_guidance_result(result: &GuidanceDeliveryResult) -> String {
    let mut parts = vec![format!("Delivery: {}", result.status)];
    if let Some(method) = &result.method {
        parts.push(format!("method={method}"));
    }
    if let Some(task_id) = &result.task_id {
        parts.push(format!("task={task_id}"));
    }
    if let Some(reason) = &result.reason {
        parts.push(format!("reason={reason}"));
    }
    parts.join(" | ")
}

fn pane_block<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    let style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(style)
}

fn format_log_line(line: &ClusterLogLine) -> String {
    if let Some(agent) = line.agent.as_deref().or(line.sender.as_deref()) {
        format!("[{}] {}", agent, line.text)
    } else {
        line.text.clone()
    }
}

fn trim_vecdeque<T>(items: &mut VecDeque<T>, max: usize) -> usize {
    if items.len() <= max {
        return 0;
    }
    let mut dropped = 0usize;
    while items.len() > max {
        items.pop_front();
        dropped += 1;
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guidance_success_clears_input() {
        let mut state = State::default();
        state.guidance_input.input = "keep".to_string();
        state.guidance_input.cursor = 4;
        state.guidance_pending = true;

        let result = GuidanceDeliveryResult {
            status: "injected".to_string(),
            reason: None,
            method: Some("pty".to_string()),
            task_id: Some("task-1".to_string()),
        };

        state.apply_guidance_result(result.clone());

        assert_eq!(state.guidance_input.input, "");
        assert_eq!(state.guidance_input.cursor, 0);
        assert!(!state.guidance_pending);
        assert_eq!(state.last_guidance, Some(result));
        assert_eq!(state.last_guidance_error, None);
    }

    #[test]
    fn guidance_error_preserves_input() {
        let mut state = State::default();
        state.guidance_input.input = "stay".to_string();
        state.guidance_input.cursor = 4;
        state.guidance_pending = true;

        state.apply_guidance_error("network".to_string());

        assert_eq!(state.guidance_input.input, "stay");
        assert_eq!(state.guidance_input.cursor, 4);
        assert!(!state.guidance_pending);
        assert_eq!(state.last_guidance_error, Some("network".to_string()));
    }

    #[test]
    fn guidance_status_format_includes_details() {
        let result = GuidanceDeliveryResult {
            status: "queued".to_string(),
            reason: Some("no tty".to_string()),
            method: Some("queue".to_string()),
            task_id: Some("task-9".to_string()),
        };

        let formatted = format_guidance_result(&result);

        assert!(formatted.contains("queued"));
        assert!(formatted.contains("method=queue"));
        assert!(formatted.contains("task=task-9"));
        assert!(formatted.contains("reason=no tty"));
    }
}
