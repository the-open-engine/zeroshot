use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::protocol::{ClusterLogLine, ClusterSummary, TimelineEvent};

const MAX_LOG_LINES: usize = 1000;
const MAX_TIMELINE_EVENTS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusDirection {
    Next,
    Prev,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterPane {
    Topology,
    Logs,
    Timeline,
    Agents,
}

impl ClusterPane {
    fn next(&self) -> Self {
        match self {
            ClusterPane::Topology => ClusterPane::Logs,
            ClusterPane::Logs => ClusterPane::Timeline,
            ClusterPane::Timeline => ClusterPane::Agents,
            ClusterPane::Agents => ClusterPane::Topology,
        }
    }

    fn prev(&self) -> Self {
        match self {
            ClusterPane::Topology => ClusterPane::Agents,
            ClusterPane::Logs => ClusterPane::Topology,
            ClusterPane::Timeline => ClusterPane::Logs,
            ClusterPane::Agents => ClusterPane::Timeline,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            ClusterPane::Topology => "Topology",
            ClusterPane::Logs => "Logs",
            ClusterPane::Timeline => "Timeline",
            ClusterPane::Agents => "Agents",
        }
    }
}

#[derive(Debug, Clone)]
pub struct State {
    pub focus: ClusterPane,
    pub summary: Option<ClusterSummary>,
    pub logs: Vec<ClusterLogLine>,
    pub timeline: Vec<TimelineEvent>,
    pub log_subscription: Option<String>,
    pub timeline_subscription: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            focus: ClusterPane::Topology,
            summary: None,
            logs: Vec::new(),
            timeline: Vec::new(),
            log_subscription: None,
            timeline_subscription: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CycleFocus(FocusDirection),
    OpenAgent(String),
}

impl State {
    pub fn cycle_focus(&mut self, direction: FocusDirection) {
        self.focus = match direction {
            FocusDirection::Next => self.focus.next(),
            FocusDirection::Prev => self.focus.prev(),
        };
    }

    pub fn push_log_lines(&mut self, mut lines: Vec<ClusterLogLine>) {
        self.logs.append(&mut lines);
        if self.logs.len() > MAX_LOG_LINES {
            let drain_count = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(0..drain_count);
        }
    }

    pub fn push_timeline_events(&mut self, mut events: Vec<TimelineEvent>) {
        self.timeline.append(&mut events);
        if self.timeline.len() > MAX_TIMELINE_EVENTS {
            let drain_count = self.timeline.len() - MAX_TIMELINE_EVENTS;
            self.timeline.drain(0..drain_count);
        }
    }
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let summary_line = state
        .summary
        .as_ref()
        .map(|summary| format!("State: {} | Provider: {:?}", summary.state, summary.provider))
        .unwrap_or_else(|| "Summary: (pending)".to_string());

    let lines = vec![
        Line::from("Cluster View"),
        Line::from(summary_line),
        Line::from(format!("Focus: {}", state.focus.label())),
        Line::from(format!("Log lines: {}", state.logs.len())),
        Line::from(format!("Timeline events: {}", state.timeline.len())),
        Line::from("Tab/Left/Right cycles focus"),
    ];

    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}
