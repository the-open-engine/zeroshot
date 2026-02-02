use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use crate::protocol::{ClusterLogLine, TimelineEvent};
use crate::ui::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogPlaceholderContext {
    Cluster,
    Agent,
    Overlay,
}

pub struct StreamOverlay<'a> {
    title: Line<'a>,
    lines: Vec<Line<'a>>,
    placeholder: Vec<Line<'a>>,
    border_style: Style,
}

impl<'a> StreamOverlay<'a> {
    pub fn new(title: impl Into<Line<'a>>, lines: Vec<Line<'a>>) -> Self {
        Self {
            title: title.into(),
            lines,
            placeholder: Vec::new(),
            border_style: theme::unfocus_border_style(),
        }
    }

    pub fn placeholder_lines(mut self, lines: Vec<Line<'a>>) -> Self {
        self.placeholder = lines;
        self
    }

    pub fn border_style(mut self, style: Style) -> Self {
        self.border_style = style;
        self
    }
}

impl<'a> Widget for StreamOverlay<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let block = Block::default()
            .title(self.title)
            .borders(Borders::ALL)
            .border_style(self.border_style);
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let mut lines = if self.lines.is_empty() {
            self.placeholder
        } else {
            self.lines
        };
        let max_lines = inner.height as usize;
        if lines.len() > max_lines {
            lines.truncate(max_lines);
        }

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        paragraph.render(inner, buf);
    }
}

pub fn log_placeholder_lines<'a>(context: LogPlaceholderContext) -> Vec<Line<'a>> {
    let detail = match context {
        LogPlaceholderContext::Cluster => "Waiting for cluster output.",
        LogPlaceholderContext::Agent => "Waiting for agent output.",
        LogPlaceholderContext::Overlay => "Waiting for stream output.",
    };
    vec![
        Line::from(Span::styled("No logs yet.", theme::muted_style())),
        Line::from(Span::styled(detail, theme::muted_style())),
    ]
}

pub fn timeline_placeholder_lines<'a>() -> Vec<Line<'a>> {
    vec![
        Line::from(Span::styled(
            "No timeline events yet.",
            theme::muted_style(),
        )),
        Line::from(Span::styled(
            "New activity will appear here.",
            theme::muted_style(),
        )),
    ]
}

pub fn format_log_line_styled(line: &ClusterLogLine) -> Line<'_> {
    if let Some(agent) = line.agent.as_deref().or(line.sender.as_deref()) {
        let color = theme::agent_color(agent);
        Line::from(vec![
            Span::styled(format!("[{agent}]"), Style::default().fg(color)),
            Span::raw(" "),
            Span::raw(line.text.as_str()),
        ])
    } else {
        Line::from(line.text.as_str())
    }
}

pub fn format_timeline_event_styled(event: &TimelineEvent) -> Line<'_> {
    let icon = timeline_icon(&event.topic);
    let label_style = timeline_label_style(&event.label);
    let mut spans = vec![
        Span::styled(icon, theme::dim_style()),
        Span::raw(" "),
        Span::styled(event.topic.as_str(), theme::dim_style()),
        Span::raw("  "),
        Span::styled(event.label.as_str(), label_style),
    ];
    if let Some(sender) = event.sender.as_deref() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("({sender})"),
            theme::muted_style(),
        ));
    }
    Line::from(spans)
}

fn timeline_icon(topic: &str) -> &'static str {
    let topic_lower = topic.to_lowercase();
    if topic_lower.contains("issue") {
        "\u{25b6}" // ▶
    } else if topic_lower.contains("implementation") || topic_lower.contains("impl") {
        "\u{25cf}" // ●
    } else if topic_lower.contains("validation") || topic_lower.contains("review") {
        "\u{25c6}" // ◆
    } else if topic_lower.contains("consensus") || topic_lower.contains("complete") {
        "\u{2605}" // ★
    } else {
        "\u{00b7}" // ·
    }
}

fn timeline_label_style(label: &str) -> Style {
    let label_lower = label.to_lowercase();
    if label_lower.contains("approved")
        || label_lower.contains("done")
        || label_lower.contains("complete")
    {
        theme::status_style("done")
    } else if label_lower.contains("rejected")
        || label_lower.contains("failed")
        || label_lower.contains("error")
    {
        theme::status_style("error")
    } else if label_lower.contains("pending") || label_lower.contains("waiting") {
        theme::status_style("pending")
    } else {
        Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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
    fn stream_overlay_renders_title_and_lines() {
        let backend = TestBackend::new(32, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| {
                let area = frame.area();
                let overlay = StreamOverlay::new(
                    Line::from("Logs - agent alpha"),
                    vec![Line::from("hello"), Line::from("world")],
                );
                frame.render_widget(overlay, area);
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "Logs - agent alpha"));
        assert!(buffer_contains(&terminal, "hello"));
        assert!(buffer_contains(&terminal, "world"));
    }

    #[test]
    fn stream_overlay_renders_empty_placeholder() {
        let backend = TestBackend::new(30, 7);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| {
                let area = frame.area();
                let overlay = StreamOverlay::new(Line::from("Logs"), Vec::new()).placeholder_lines(
                    log_placeholder_lines(LogPlaceholderContext::Overlay),
                );
                frame.render_widget(overlay, area);
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "No logs yet."));
        assert!(buffer_contains(&terminal, "Waiting for stream output."));
    }
}
