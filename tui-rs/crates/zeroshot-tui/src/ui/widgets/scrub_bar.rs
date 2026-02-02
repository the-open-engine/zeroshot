use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{TimeCursor, TimeCursorMode};
use crate::protocol::ClusterLogLine;
use crate::ui::shared::TimeIndexedBuffer;
use crate::ui::theme;

const DENSITY_LEVELS: &[u8] = b" .:-=+*#";

pub struct ScrubBarState<'a> {
    pub time_cursor: &'a TimeCursor,
    pub logs: Option<&'a TimeIndexedBuffer<ClusterLogLine>>,
    pub agent_id: Option<&'a str>,
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: ScrubBarState<'_>) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let label = match state.time_cursor.mode {
        TimeCursorMode::Live => "LIVE",
        TimeCursorMode::Scrub => "SCRUB",
    };
    let label_style = match state.time_cursor.mode {
        TimeCursorMode::Live => theme::toast_success_style(),
        TimeCursorMode::Scrub => theme::key_style(),
    };

    let label_len = label.chars().count() as u16;
    let mut spans = Vec::new();
    spans.push(Span::styled(label, label_style));

    let bar_width = area.width.saturating_sub(label_len.saturating_add(1)) as usize;
    if bar_width > 0 {
        spans.push(Span::raw(" "));
        let bar = build_bar(bar_width, &state);
        spans.push(Span::styled(bar, theme::dim_style()));
    }

    let widget = Paragraph::new(Line::from(spans));
    frame.render_widget(widget, area);
}

fn build_bar(width: usize, state: &ScrubBarState<'_>) -> String {
    if width == 0 {
        return String::new();
    }

    let window_ms = state.time_cursor.window_ms.max(1);
    let window_end = state.time_cursor.t_ms;
    let window_start = window_end.saturating_sub(window_ms);

    let latest_ts = state.logs.and_then(|logs| {
        logs.iter()
            .filter(|line| matches_agent(line, state.agent_id))
            .map(|line| line.timestamp)
            .max()
    });

    let mut bins = vec![0u32; width];
    if let Some(logs) = state.logs {
        let windowed = logs.window(window_end, window_ms);
        for line in windowed {
            if !matches_agent(line, state.agent_id) {
                continue;
            }
            let rel = line.timestamp.saturating_sub(window_start);
            let mut pos = ((rel * width as i64) / window_ms) as usize;
            if pos >= width {
                pos = width - 1;
            }
            bins[pos] = bins[pos].saturating_add(1);
        }
    }

    let max = bins.iter().copied().max().unwrap_or(0);
    let mut chars: Vec<char> = bins
        .into_iter()
        .map(|count| {
            if max == 0 {
                ' '
            } else {
                let idx = (count as usize * (DENSITY_LEVELS.len() - 1)) / max as usize;
                DENSITY_LEVELS[idx] as char
            }
        })
        .collect();

    let now_pos = latest_ts.map_or(width.saturating_sub(1), |latest| {
        let rel = latest.saturating_sub(window_start);
        let mut pos = ((rel * width as i64) / window_ms) as usize;
        if pos >= width {
            pos = width - 1;
        }
        pos
    });
    if !chars.is_empty() {
        chars[now_pos] = '|';
    }

    if matches!(state.time_cursor.mode, TimeCursorMode::Scrub) && !chars.is_empty() {
        let rel = state.time_cursor.t_ms.saturating_sub(window_start);
        let mut pos = ((rel * width as i64) / window_ms) as usize;
        if pos >= width {
            pos = width - 1;
        }
        if pos == now_pos {
            chars[pos] = '*';
        } else {
            chars[pos] = '^';
        }
    }

    chars.into_iter().collect()
}

fn matches_agent(line: &ClusterLogLine, agent_id: Option<&str>) -> bool {
    let Some(agent_id) = agent_id else {
        return true;
    };
    line.agent.as_deref() == Some(agent_id) || line.sender.as_deref() == Some(agent_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::ui::widgets::test_utils::line_text;

    fn sample_logs(timestamps: &[i64]) -> TimeIndexedBuffer<ClusterLogLine> {
        let mut buffer = TimeIndexedBuffer::new(64);
        let lines = timestamps.iter().map(|ts| ClusterLogLine {
            id: format!("log-{ts}"),
            timestamp: *ts,
            text: "event".to_string(),
            agent: None,
            role: None,
            sender: None,
        });
        buffer.push_many(lines);
        buffer
    }

    #[test]
    fn scrub_bar_renders_live_mode() {
        let logs = sample_logs(&[100, 200, 300]);
        let cursor = TimeCursor {
            mode: TimeCursorMode::Live,
            t_ms: 300,
            window_ms: 300,
        };

        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    ScrubBarState {
                        time_cursor: &cursor,
                        logs: Some(&logs),
                        agent_id: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let text = line_text(buffer, 0);
        assert!(text.contains("LIVE"));
        assert!(text.contains("|"));
    }

    #[test]
    fn scrub_bar_renders_scrub_mode_marker() {
        let logs = sample_logs(&[100, 200, 300]);
        let cursor = TimeCursor {
            mode: TimeCursorMode::Scrub,
            t_ms: 150,
            window_ms: 300,
        };

        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    ScrubBarState {
                        time_cursor: &cursor,
                        logs: Some(&logs),
                        agent_id: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let text = line_text(buffer, 0);
        assert!(text.contains("SCRUB"));
        assert!(text.contains("*"));
    }

    #[test]
    fn scrub_bar_handles_empty_buffers() {
        let logs = sample_logs(&[]);
        let cursor = TimeCursor {
            mode: TimeCursorMode::Live,
            t_ms: 0,
            window_ms: 500,
        };

        let backend = TestBackend::new(24, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    ScrubBarState {
                        time_cursor: &cursor,
                        logs: Some(&logs),
                        agent_id: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let text = line_text(buffer, 0);
        assert!(text.contains("LIVE"));
    }
}
