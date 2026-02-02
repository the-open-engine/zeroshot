use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::TimeCursor;
use crate::screens::cluster;
use crate::ui::theme;
use crate::ui::widgets::stream::{self, StreamOverlay};

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    cluster_id: &str,
    agent_id: &str,
    cluster_state: Option<&cluster::State>,
    time_cursor: &TimeCursor,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Agent Microscope");

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let [header_area, log_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);

    let header_lines = vec![
        Line::from(Span::styled(agent_id, theme::title_style())),
        Line::from(Span::styled(
            format!("Cluster {cluster_id}"),
            theme::muted_style(),
        )),
        Line::from(Span::styled("Press Esc to return", theme::dim_style())),
    ];
    let header = Paragraph::new(header_lines).alignment(Alignment::Center);
    frame.render_widget(header, header_area);

    if log_area.width == 0 || log_area.height == 0 {
        return;
    }

    let inner = Block::default().borders(Borders::ALL).inner(log_area);
    let max_lines = inner.height as usize;
    if max_lines == 0 {
        return;
    }

    let log_lines = cluster_state
        .map(|state| {
            stream::select_time_window(
                &state.logs_time,
                time_cursor,
                max_lines,
                |line| {
                    line.agent.as_deref() == Some(agent_id)
                        || line.sender.as_deref() == Some(agent_id)
                },
            )
        })
        .unwrap_or_default()
        .into_iter()
        .map(stream::format_log_line_styled)
        .collect();

    let title = stream::overlay_title(format!("Logs - agent {agent_id}"), time_cursor);
    let overlay = StreamOverlay::new(title, log_lines)
        .placeholder_lines(stream::log_placeholder_lines(
            stream::LogPlaceholderContext::Agent,
        ))
        .border_style(theme::focus_border_style());
    frame.render_widget(overlay, log_area);
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

    #[test]
    fn agent_microscope_renders_windowed_logs() {
        let backend = TestBackend::new(70, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut cluster_state = cluster::State::default();
        cluster_state.push_log_lines(
            vec![
                ClusterLogLine {
                    id: "old".to_string(),
                    timestamp: 100,
                    text: "old-line".to_string(),
                    agent: Some("agent-1".to_string()),
                    role: None,
                    sender: None,
                },
                ClusterLogLine {
                    id: "other".to_string(),
                    timestamp: 180,
                    text: "other-agent".to_string(),
                    agent: Some("agent-2".to_string()),
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
            ],
            None,
        );

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
                    Some(&cluster_state),
                    &cursor,
                );
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "mid-line"));
        assert!(!buffer_contains(&terminal, "old-line"));
        assert!(!buffer_contains(&terminal, "new-line"));
        assert!(!buffer_contains(&terminal, "other-agent"));
    }
}
