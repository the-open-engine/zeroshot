use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{agent_microscope, TimeCursor};
use crate::protocol::{ClusterLogLine, TimelineEvent};
use crate::ui::shared::TimeIndexedBuffer;
use crate::ui::theme;
use crate::ui::widgets::stream::{self, StreamOverlay};

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    cluster_id: &str,
    agent_id: &str,
    cluster_timeline: Option<&TimeIndexedBuffer<TimelineEvent>>,
    microscope_state: Option<&agent_microscope::State>,
    time_cursor: &TimeCursor,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let metadata = build_metadata_overlay(area, cluster_id, agent_id, microscope_state);
    let reserved_lines = metadata
        .as_ref()
        .map(|overlay| overlay.reserved_lines)
        .unwrap_or(0);

    let max_lines = area.height.saturating_sub(2) as usize;
    let window_max = max_lines.saturating_sub(reserved_lines);
    let log_entries = microscope_state
        .map(|state| {
            stream::select_time_window(&state.logs_time, time_cursor, window_max, |_| true)
        })
        .unwrap_or_default()
        .into_iter()
        .collect::<Vec<_>>();

    let marker_margin =
        build_phase_marker_margin(area, cluster_timeline, time_cursor, &log_entries);

    let mut content_lines = if log_entries.is_empty() {
        stream::log_placeholder_lines(stream::LogPlaceholderContext::Agent)
    } else {
        let log_lines = log_entries
            .iter()
            .map(|line| stream::format_log_line_styled(line))
            .collect::<Vec<_>>();
        if let Some(marker_margin) = marker_margin {
            apply_phase_marker_margin(log_lines, &marker_margin)
        } else {
            log_lines
        }
    };

    if reserved_lines > 0 {
        let mut padded = Vec::with_capacity(reserved_lines + content_lines.len());
        for _ in 0..reserved_lines {
            padded.push(Line::from(""));
        }
        padded.extend(content_lines);
        content_lines = padded;
    }

    let title = stream::overlay_title("Stream", time_cursor);
    let overlay =
        StreamOverlay::new(title, content_lines).border_style(theme::focus_border_style());
    frame.render_widget(overlay, area);

    if let Some(metadata) = metadata {
        render_metadata_overlay(frame, metadata);
    }
}

struct PhaseMarkerMargin {
    labels: Vec<Option<String>>,
    margin_width: usize,
}

struct MetadataOverlay {
    area: Rect,
    lines: Vec<Line<'static>>,
    reserved_lines: usize,
}

fn build_metadata_overlay(
    area: Rect,
    cluster_id: &str,
    agent_id: &str,
    microscope_state: Option<&agent_microscope::State>,
) -> Option<MetadataOverlay> {
    let available_width = area.width.saturating_sub(2);
    let available_height = area.height.saturating_sub(2);
    if available_width < 6 || available_height < 4 {
        return None;
    }

    let role = microscope_state
        .and_then(|state| state.role.as_deref())
        .unwrap_or("unknown");
    let status = microscope_state
        .and_then(|state| state.status.as_deref())
        .unwrap_or("unknown");

    let raw_lines = [
        format!("Agent: {agent_id}"),
        format!("Role: {role}"),
        format!("Status: {status}"),
        format!("Cluster: {cluster_id}"),
    ];

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Agent: ", theme::muted_style()),
            Span::styled(
                agent_id.to_string(),
                Style::default().fg(theme::agent_color(agent_id)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Role: ", theme::muted_style()),
            Span::styled(role.to_string(), theme::dim_style()),
        ]),
        Line::from(vec![
            Span::styled("Status: ", theme::muted_style()),
            Span::styled(status.to_string(), theme::status_style(status)),
        ]),
        Line::from(vec![
            Span::styled("Cluster: ", theme::muted_style()),
            Span::styled(cluster_id.to_string(), theme::muted_style()),
        ]),
    ];

    let max_line_len = raw_lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or_default() as u16;

    let overlay_width = (max_line_len + 2).min(available_width);
    let overlay_height = (lines.len() as u16 + 2).min(available_height);
    if overlay_width < 4 || overlay_height < 3 || overlay_height >= available_height {
        return None;
    }

    let max_lines = overlay_height.saturating_sub(2) as usize;
    if lines.len() > max_lines {
        lines.truncate(max_lines);
    }

    let overlay_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: overlay_width,
        height: overlay_height,
    };

    Some(MetadataOverlay {
        area: overlay_area,
        lines,
        reserved_lines: overlay_height as usize,
    })
}

fn render_metadata_overlay(frame: &mut Frame<'_>, metadata: MetadataOverlay) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::unfocus_border_style());
    let widget = Paragraph::new(metadata.lines).block(block);
    frame.render_widget(widget, metadata.area);
}

fn build_phase_marker_margin(
    area: Rect,
    timeline: Option<&TimeIndexedBuffer<TimelineEvent>>,
    time_cursor: &TimeCursor,
    log_entries: &[&ClusterLogLine],
) -> Option<PhaseMarkerMargin> {
    let timeline = timeline?;
    if log_entries.is_empty() {
        return None;
    }
    let available_width = area.width.saturating_sub(2);
    let margin_width = phase_marker_margin_width(available_width);
    if margin_width == 0 {
        return None;
    }

    let markers = stream::derive_phase_markers(timeline, time_cursor, stream::PHASE_MARKER_LIMIT);
    if markers.is_empty() {
        return None;
    }

    let mut labels = vec![None; log_entries.len()];
    for marker in markers {
        let index = marker_line_index(log_entries, marker.timestamp_ms);
        let raw = stream::format_phase_marker_label(&marker.topic, &marker.label);
        let truncated = stream::truncate_marker_label(&raw, margin_width);
        labels[index] = Some(pad_phase_label(&truncated, margin_width));
    }

    Some(PhaseMarkerMargin {
        labels,
        margin_width,
    })
}

fn phase_marker_margin_width(available_width: u16) -> usize {
    let available = available_width as usize;
    let min_content = 16usize;
    let min_margin = 8usize;
    if available < min_content + min_margin {
        return 0;
    }
    let mut margin = available / 4;
    if margin < min_margin {
        margin = min_margin;
    }
    if margin > 14 {
        margin = 14;
    }
    if available.saturating_sub(margin) < min_content {
        return 0;
    }
    margin
}

fn marker_line_index(log_entries: &[&ClusterLogLine], timestamp: i64) -> usize {
    if log_entries.is_empty() {
        return 0;
    }
    let mut left = 0usize;
    let mut right = log_entries.len();
    while left < right {
        let mid = left + (right - left) / 2;
        if log_entries[mid].timestamp < timestamp {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    if left >= log_entries.len() {
        log_entries.len().saturating_sub(1)
    } else {
        left
    }
}

fn pad_phase_label(label: &str, width: usize) -> String {
    let len = label.chars().count();
    if len >= width {
        return label.to_string();
    }
    let mut out = String::with_capacity(width);
    out.push_str(label);
    for _ in 0..(width - len) {
        out.push(' ');
    }
    out
}

fn apply_phase_marker_margin<'a>(
    lines: Vec<Line<'a>>,
    marker_margin: &PhaseMarkerMargin,
) -> Vec<Line<'a>> {
    let empty_label = " ".repeat(marker_margin.margin_width);
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let label = marker_margin
                .labels
                .get(idx)
                .and_then(|label| label.as_ref())
                .unwrap_or(&empty_label);
            prepend_phase_marker_label(line, label, marker_margin.margin_width)
        })
        .collect()
}

fn prepend_phase_marker_label<'a>(line: Line<'a>, label: &str, margin_width: usize) -> Line<'a> {
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    let mut label_text = label.to_string();
    if label_text.chars().count() < margin_width {
        label_text = pad_phase_label(&label_text, margin_width);
    }
    spans.push(Span::styled(label_text, theme::muted_style()));
    spans.push(Span::raw(" "));
    spans.extend(line.spans);
    Line {
        style: line.style,
        alignment: line.alignment,
        spans,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::app::TimeCursorMode;
    use crate::protocol::ClusterLogLine;
    use crate::ui::widgets::test_utils::line_text;

    fn buffer_contains(terminal: &Terminal<TestBackend>, needle: &str) -> bool {
        let buffer = terminal.backend().buffer();
        for y in 0..buffer.area.height {
            if line_text(buffer, y).contains(needle) {
                return true;
            }
        }
        false
    }

    fn sample_state(lines: Vec<ClusterLogLine>) -> agent_microscope::State {
        let mut state = agent_microscope::State::default();
        state.push_log_lines(lines, None);
        state
    }

    fn sample_timeline_event(id: &str, timestamp: i64, topic: &str, label: &str) -> TimelineEvent {
        TimelineEvent {
            id: id.to_string(),
            timestamp,
            topic: topic.to_string(),
            label: label.to_string(),
            approved: None,
            sender: None,
        }
    }

    #[test]
    fn agent_microscope_renders_empty_state() {
        let backend = TestBackend::new(70, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let cursor = TimeCursor {
            mode: TimeCursorMode::Live,
            t_ms: 0,
            window_ms: 60,
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, "cluster-1", "agent-1", None, None, &cursor);
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "No logs yet."));
        assert!(buffer_contains(&terminal, "Agent: agent-1"));
    }

    #[test]
    fn agent_microscope_renders_live_mode() {
        let backend = TestBackend::new(70, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = sample_state(vec![
            ClusterLogLine {
                id: "old".to_string(),
                timestamp: 100,
                text: "old-line".to_string(),
                agent: Some("agent-1".to_string()),
                role: None,
                sender: None,
            },
            ClusterLogLine {
                id: "new".to_string(),
                timestamp: 300,
                text: "new-line".to_string(),
                agent: Some("agent-1".to_string()),
                role: None,
                sender: None,
            },
        ]);

        let cursor = TimeCursor {
            mode: TimeCursorMode::Live,
            t_ms: 300,
            window_ms: 60,
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    "cluster-1",
                    "agent-1",
                    None,
                    Some(&state),
                    &cursor,
                );
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "new-line"));
    }

    #[test]
    fn agent_microscope_renders_scrub_window() {
        let backend = TestBackend::new(70, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = sample_state(vec![
            ClusterLogLine {
                id: "old".to_string(),
                timestamp: 100,
                text: "old-line".to_string(),
                agent: Some("agent-1".to_string()),
                role: None,
                sender: None,
            },
            ClusterLogLine {
                id: "mid".to_string(),
                timestamp: 220,
                text: "mid-line".to_string(),
                agent: Some("agent-1".to_string()),
                role: None,
                sender: None,
            },
            ClusterLogLine {
                id: "new".to_string(),
                timestamp: 400,
                text: "new-line".to_string(),
                agent: Some("agent-1".to_string()),
                role: None,
                sender: None,
            },
        ]);

        let cursor = TimeCursor {
            mode: TimeCursorMode::Scrub,
            t_ms: 230,
            window_ms: 60,
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    "cluster-1",
                    "agent-1",
                    None,
                    Some(&state),
                    &cursor,
                );
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "mid-line"));
        assert!(!buffer_contains(&terminal, "old-line"));
        assert!(!buffer_contains(&terminal, "new-line"));
    }

    #[test]
    fn agent_microscope_renders_phase_markers_margin() {
        let backend = TestBackend::new(70, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = sample_state(vec![
            ClusterLogLine {
                id: "line-1".to_string(),
                timestamp: 100,
                text: "first-line".to_string(),
                agent: Some("agent-1".to_string()),
                role: None,
                sender: None,
            },
            ClusterLogLine {
                id: "line-2".to_string(),
                timestamp: 220,
                text: "second-line".to_string(),
                agent: Some("agent-1".to_string()),
                role: None,
                sender: None,
            },
        ]);
        let mut timeline = TimeIndexedBuffer::new(16);
        timeline.push_many(vec![sample_timeline_event("event-1", 150, "plan", "start")]);

        let cursor = TimeCursor {
            mode: TimeCursorMode::Live,
            t_ms: 220,
            window_ms: 60,
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    "cluster-1",
                    "agent-1",
                    Some(&timeline),
                    Some(&state),
                    &cursor,
                );
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "plan: start"));
        assert!(buffer_contains(&terminal, "second-line"));
    }
}
