use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use crate::app::{TimeCursor, TimeCursorMode};
use crate::protocol::{ClusterLogLine, TimelineEvent};
use crate::ui::shared::{HasTimestamp, TimeIndexedBuffer};
use crate::ui::theme;

pub const PHASE_MARKER_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogPlaceholderContext {
    Cluster,
    Agent,
    Overlay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseMarker {
    pub timestamp_ms: i64,
    pub label: String,
    pub topic: String,
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

pub fn mode_tag_span(time_cursor: &TimeCursor) -> Span<'static> {
    let (label, style) = match time_cursor.mode {
        TimeCursorMode::Live => ("LIVE", theme::toast_success_style()),
        TimeCursorMode::Scrub => ("SCRUB", theme::key_style()),
    };
    Span::styled(format!("[{label}]"), style)
}

pub fn derive_phase_markers(
    timeline: &TimeIndexedBuffer<TimelineEvent>,
    time_cursor: &TimeCursor,
    max_markers: usize,
) -> Vec<PhaseMarker> {
    if max_markers == 0 || timeline.is_empty() {
        return Vec::new();
    }

    let max_items = timeline.len();
    let events = select_time_window(timeline, time_cursor, max_items, |_| true);
    if events.is_empty() {
        return Vec::new();
    }

    let mut markers = Vec::new();
    let mut last_topic = String::new();
    let mut last_label = String::new();
    let mut has_last = false;
    for event in events {
        if has_last && last_topic == event.topic && last_label == event.label {
            continue;
        }
        markers.push(PhaseMarker {
            timestamp_ms: event.timestamp,
            label: event.label.clone(),
            topic: event.topic.clone(),
        });
        last_topic = event.topic.clone();
        last_label = event.label.clone();
        has_last = true;
    }

    if markers.len() > max_markers {
        let start = markers.len().saturating_sub(max_markers);
        markers = markers.split_off(start);
    }
    markers
}

pub fn format_phase_marker_label(topic: &str, label: &str) -> String {
    if label.is_empty() {
        topic.to_string()
    } else {
        format!("{topic}: {label}")
    }
}

pub fn truncate_marker_label(label: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let label_len = label.chars().count();
    if label_len <= max_chars {
        return label.to_string();
    }
    if max_chars <= 3 {
        return label.chars().take(max_chars).collect();
    }
    let mut out: String = label.chars().take(max_chars - 3).collect();
    out.push_str("...");
    out
}

pub fn overlay_title(base: impl Into<String>, time_cursor: &TimeCursor) -> Line<'static> {
    Line::from(vec![
        Span::raw(base.into()),
        Span::raw(" "),
        mode_tag_span(time_cursor),
    ])
}

pub fn select_time_window<'a, T, F>(
    buffer: &'a TimeIndexedBuffer<T>,
    time_cursor: &TimeCursor,
    max_items: usize,
    filter: F,
) -> Vec<&'a T>
where
    T: HasTimestamp,
    F: Fn(&T) -> bool,
{
    if max_items == 0 || buffer.is_empty() {
        return Vec::new();
    }

    match time_cursor.mode {
        TimeCursorMode::Live => select_live_tail(buffer, max_items, filter),
        TimeCursorMode::Scrub => select_scrub_window(buffer, time_cursor, max_items, filter),
    }
}

fn select_live_tail<T, F>(buffer: &TimeIndexedBuffer<T>, max_items: usize, filter: F) -> Vec<&T>
where
    T: HasTimestamp,
    F: Fn(&T) -> bool,
{
    let mut collected = Vec::with_capacity(max_items);
    for item in buffer.iter_rev() {
        if filter(item) {
            collected.push(item);
            if collected.len() >= max_items {
                break;
            }
        }
    }
    collected.reverse();
    collected
}

fn select_scrub_window<'a, T, F>(
    buffer: &'a TimeIndexedBuffer<T>,
    time_cursor: &TimeCursor,
    max_items: usize,
    filter: F,
) -> Vec<&'a T>
where
    T: HasTimestamp,
    F: Fn(&T) -> bool,
{
    let windowed = buffer.window(time_cursor.t_ms, time_cursor.window_ms);
    let mut collected: Vec<&T> = windowed.into_iter().filter(|item| filter(item)).collect();
    if collected.len() > max_items {
        let start = collected.len().saturating_sub(max_items);
        collected = collected.split_off(start);
    }
    collected
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
        spans.push(Span::styled(format!("({sender})"), theme::muted_style()));
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
                let overlay = StreamOverlay::new(Line::from("Logs"), Vec::new())
                    .placeholder_lines(log_placeholder_lines(LogPlaceholderContext::Overlay));
                frame.render_widget(overlay, area);
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "No logs yet."));
        assert!(buffer_contains(&terminal, "Waiting for stream output."));
    }

    fn sample_log(id: &str, timestamp: i64, agent: Option<&str>) -> ClusterLogLine {
        ClusterLogLine {
            id: id.to_string(),
            timestamp,
            text: format!("log-{id}"),
            agent: agent.map(|value| value.to_string()),
            role: None,
            sender: None,
        }
    }

    fn sample_event(id: &str, timestamp: i64, topic: &str, label: &str) -> TimelineEvent {
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
    fn stream_window_live_uses_tail() {
        let mut buffer = TimeIndexedBuffer::new(16);
        buffer.push_many(vec![
            sample_log("one", 100, Some("alpha")),
            sample_log("two", 200, Some("alpha")),
            sample_log("three", 300, Some("alpha")),
        ]);

        let cursor = TimeCursor::default();
        let selected = select_time_window(&buffer, &cursor, 2, |_| true);
        let ids: Vec<&str> = selected.iter().map(|line| line.id.as_str()).collect();
        assert_eq!(ids, vec!["two", "three"]);
    }

    #[test]
    fn stream_window_scrub_uses_window() {
        let mut buffer = TimeIndexedBuffer::new(16);
        buffer.push_many(vec![
            sample_log("one", 100, Some("alpha")),
            sample_log("two", 200, Some("alpha")),
            sample_log("three", 300, Some("alpha")),
        ]);

        let cursor = TimeCursor {
            mode: TimeCursorMode::Scrub,
            t_ms: 250,
            window_ms: 120,
        };
        let selected = select_time_window(&buffer, &cursor, 10, |_| true);
        let ids: Vec<&str> = selected.iter().map(|line| line.id.as_str()).collect();
        assert_eq!(ids, vec!["two"]);
    }

    #[test]
    fn stream_overlay_renders_mode_tag() {
        let backend = TestBackend::new(40, 6);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let cursor = TimeCursor::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                let overlay = StreamOverlay::new(overlay_title("Logs", &cursor), Vec::new())
                    .placeholder_lines(log_placeholder_lines(LogPlaceholderContext::Overlay));
                frame.render_widget(overlay, area);
            })
            .expect("draw");

        assert!(buffer_contains(&terminal, "LIVE"));
    }

    #[test]
    fn derive_phase_markers_caps_and_dedup() {
        let mut buffer = TimeIndexedBuffer::new(128);
        buffer.push_many(vec![
            sample_event("e1", 100, "topic-a", "phase-1"),
            sample_event("e2", 110, "topic-a", "phase-1"),
            sample_event("e3", 120, "topic-b", "phase-2"),
            sample_event("e4", 130, "topic-b", "phase-2"),
            sample_event("e5", 140, "topic-b", "phase-3"),
        ]);

        let cursor = TimeCursor::default();
        let markers = derive_phase_markers(&buffer, &cursor, PHASE_MARKER_LIMIT);
        assert_eq!(markers.len(), 3);
        assert_eq!(markers[0].topic, "topic-a");
        assert_eq!(markers[1].label, "phase-2");
        assert_eq!(markers[2].label, "phase-3");

        let mut buffer = TimeIndexedBuffer::new(128);
        let mut events = Vec::new();
        for idx in 0..60 {
            events.push(sample_event(
                &format!("cap-{idx}"),
                1000 + idx as i64,
                &format!("topic-{idx}"),
                "phase",
            ));
        }
        buffer.push_many(events);
        let markers = derive_phase_markers(&buffer, &cursor, 50);
        assert_eq!(markers.len(), 50);
        assert_eq!(markers.first().unwrap().topic, "topic-10");
        assert_eq!(markers.last().unwrap().topic, "topic-59");
    }
}
