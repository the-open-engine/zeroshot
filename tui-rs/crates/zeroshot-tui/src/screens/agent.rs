use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

use crate::protocol::{ClusterLogLine, GuidanceDeliveryResult};
use crate::ui::shared::{pane_block, InputState, ScrollableBuffer};
use crate::ui::theme;
use crate::ui::widgets::stream;

pub const MAX_LOG_LINES: usize = 500;

#[derive(Debug, Clone)]
pub struct State {
    pub logs: ScrollableBuffer<ClusterLogLine>,
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
        let mut to_push = Vec::new();
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
                to_push.push(line);
            }
        }

        to_push.append(&mut lines);
        self.logs.push_many(to_push);
    }

    pub fn move_log_scroll(&mut self, delta: i32) {
        self.logs.move_scroll(delta);
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
            return "Sending guidance...".to_string();
        }
        if let Some(error) = &self.last_guidance_error {
            return format!("Last send failed: {error}");
        }
        if let Some(result) = &self.last_guidance {
            return format!("Last send: {}", format_guidance_result(result));
        }
        "Guidance ready. Enter to send.".to_string()
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            logs: ScrollableBuffer::new(MAX_LOG_LINES),
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

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State, cluster_id: &str, agent_id: &str) {
    let [header_area, logs_area, guidance_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(4),
    ])
    .areas(area);

    render_header(frame, header_area, state, cluster_id, agent_id);
    render_logs(frame, logs_area, state);
    render_guidance(frame, guidance_area, state);
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
    let (status_dot, status_style) = match status {
        "executing" | "running" | "active" => ("\u{25cf}", theme::status_style("running")),
        "waiting" | "idle" => ("\u{25cf}", theme::status_style("pending")),
        "error" | "failed" => ("\u{25cf}", theme::status_style("error")),
        "done" | "completed" => ("\u{25cf}", theme::status_style("done")),
        _ => ("\u{25cb}", theme::dim_style()),
    };
    let agent_color = theme::agent_color(agent_id);

    let lines = vec![
        Line::from(vec![
            Span::styled(" Agent: ", theme::dim_style()),
            Span::styled(agent_id, Style::default().fg(agent_color)),
            Span::styled("  Role: ", theme::dim_style()),
            Span::styled(role, theme::dim_style()),
            Span::styled("  Status: ", theme::dim_style()),
            Span::styled(status_dot, status_style),
            Span::raw(" "),
            Span::styled(status, status_style),
        ]),
        Line::from(vec![
            Span::styled(" Cluster: ", theme::dim_style()),
            Span::styled(cluster_id, theme::muted_style()),
        ]),
    ];
    let widget = Paragraph::new(lines);
    frame.render_widget(widget, area);
}

fn render_logs(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let title = if state.logs.scroll_offset > 0 {
        format!("Logs (up {})", state.logs.scroll_offset)
    } else {
        "Logs".to_string()
    };
    let block = pane_block(title, true);
    let inner = block.inner(area);
    let height = inner.height as usize;
    let lines = if state.logs.is_empty() || height == 0 {
        stream::log_placeholder_lines(stream::LogPlaceholderContext::Agent)
    } else {
        let total = state.logs.len();
        let max_start = total.saturating_sub(height);
        let start = max_start.saturating_sub(state.logs.scroll_offset.min(max_start));
        state
            .logs
            .items
            .iter()
            .skip(start)
            .take(height)
            .map(stream::format_log_line_styled)
            .collect()
    };
    let widget = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);

    // Scrollbar
    if !state.logs.is_empty() && height > 0 {
        let total = state.logs.len();
        let position = total
            .saturating_sub(height)
            .saturating_sub(state.logs.scroll_offset);
        let mut scrollbar_state =
            ScrollbarState::new(total.saturating_sub(height)).position(position);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            inner,
            &mut scrollbar_state,
        );
    }
}

fn render_guidance(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let status_line = Line::from(state.guidance_status_line());
    let input_line = if state.guidance_input.input.is_empty() {
        Line::from(Span::styled("Type guidance...", theme::muted_style()))
    } else {
        Line::from(state.guidance_input.input.as_str())
    };
    let lines = vec![status_line, input_line];
    let block = Block::default()
        .title("Guidance (Enter to send)")
        .borders(Borders::ALL)
        .border_style(theme::focus_border_style());
    let input = Paragraph::new(lines).block(block);
    frame.render_widget(input, area);

    if area.height > 3 && area.width > 2 {
        let max_x = area.x + area.width.saturating_sub(2);
        let cursor_x = area.x + 1 + state.guidance_input.cursor as u16;
        let cursor_x = cursor_x.min(max_x);
        let cursor_y = area.y + 2;
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

pub fn format_guidance_result(result: &GuidanceDeliveryResult) -> String {
    let mut parts = vec![result.status.clone()];
    if let Some(method) = &result.method {
        parts.push(format!("via {method}"));
    }
    if let Some(task_id) = &result.task_id {
        parts.push(format!("task {task_id}"));
    }
    if let Some(reason) = &result.reason {
        parts.push(format!("reason {reason}"));
    }
    parts.join(" | ")
}

// shared stream formatters live in ui/widgets/stream.rs

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
        assert!(formatted.contains("via queue"));
        assert!(formatted.contains("task task-9"));
        assert!(formatted.contains("reason no tty"));
    }
}
