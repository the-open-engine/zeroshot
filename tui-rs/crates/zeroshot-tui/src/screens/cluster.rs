use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

use crate::protocol::{
    ClusterLogLine, ClusterMetrics, ClusterSummary, ClusterTopology, TimelineEvent,
};
use crate::screens::metrics;
use crate::ui::shared::{pane_block, HasTimestamp, ScrollableBuffer, TimeIndexedBuffer};
use crate::ui::theme;
use crate::ui::widgets::{stream, topology};

pub const MAX_LOG_LINES: usize = 1000;
pub const MAX_TIMELINE_EVENTS: usize = 500;

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
}

#[derive(Debug, Clone)]
pub struct State {
    pub focus: ClusterPane,
    pub summary: Option<ClusterSummary>,
    pub topology: Option<ClusterTopology>,
    pub topology_error: Option<String>,
    pub logs: ScrollableBuffer<ClusterLogLine>,
    pub logs_time: TimeIndexedBuffer<ClusterLogLine>,
    pub timeline: ScrollableBuffer<TimelineEvent>,
    pub timeline_time: TimeIndexedBuffer<TimelineEvent>,
    pub agents: Vec<AgentInfo>,
    pub selected_agent: usize,
    pub log_drop_seq: u64,
    pub log_subscription: Option<String>,
    pub timeline_subscription: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            focus: ClusterPane::Topology,
            summary: None,
            topology: None,
            topology_error: None,
            logs: ScrollableBuffer::new(MAX_LOG_LINES),
            logs_time: TimeIndexedBuffer::new(MAX_LOG_LINES),
            timeline: ScrollableBuffer::new(MAX_TIMELINE_EVENTS),
            timeline_time: TimeIndexedBuffer::new(MAX_TIMELINE_EVENTS),
            agents: Vec::new(),
            selected_agent: 0,
            log_drop_seq: 0,
            log_subscription: None,
            timeline_subscription: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub id: String,
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CycleFocus(FocusDirection),
    MoveFocused(i32),
    ActivateFocused,
    OpenAgent(String),
}

impl State {
    pub fn cycle_focus(&mut self, direction: FocusDirection) {
        self.focus = match direction {
            FocusDirection::Next => self.focus.next(),
            FocusDirection::Prev => self.focus.prev(),
        };
    }

    pub fn move_focused(&mut self, delta: i32) {
        match self.focus {
            ClusterPane::Topology => {}
            ClusterPane::Logs => self.logs.move_scroll(delta),
            ClusterPane::Timeline => self.timeline.move_scroll(delta),
            ClusterPane::Agents => self.move_agent_selection(delta),
        }
    }

    pub fn activate_focused(&self) -> Option<String> {
        match self.focus {
            ClusterPane::Agents => self.selected_agent_id(),
            _ => None,
        }
    }

    pub fn push_log_lines(&mut self, mut lines: Vec<ClusterLogLine>, dropped_count: Option<i64>) {
        self.update_agents_from_logs(&lines);

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
        let time_lines = to_push.clone();
        self.logs.push_many(to_push);
        self.logs_time.push_many(time_lines);
    }

    pub fn push_timeline_events(&mut self, events: Vec<TimelineEvent>) {
        let time_events = events.clone();
        self.timeline.push_many(events);
        self.timeline_time.push_many(time_events);
    }

    fn move_agent_selection(&mut self, delta: i32) {
        if self.agents.is_empty() {
            self.selected_agent = 0;
            return;
        }

        let len = self.agents.len() as i32;
        let mut next = self.selected_agent as i32 + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        self.selected_agent = next as usize;
    }

    fn update_agents_from_logs(&mut self, lines: &[ClusterLogLine]) {
        let selected_id = self.selected_agent_id();
        for line in lines {
            let Some(agent_id) = line.agent.as_ref() else {
                continue;
            };

            match self.agents.iter_mut().find(|agent| agent.id == *agent_id) {
                Some(agent) => {
                    if agent.role.is_none() {
                        agent.role = line.role.clone();
                    }
                }
                None => {
                    self.agents.push(AgentInfo {
                        id: agent_id.clone(),
                        role: line.role.clone(),
                    });
                }
            }
        }
        self.reconcile_agent_selection(selected_id);
    }

    fn reconcile_agent_selection(&mut self, selected_id: Option<String>) {
        if let Some(id) = selected_id {
            if let Some(index) = self.agents.iter().position(|agent| agent.id == id) {
                self.selected_agent = index;
                return;
            }
        }

        if self.selected_agent >= self.agents.len() {
            self.selected_agent = self.agents.len().saturating_sub(1);
        }
    }

    fn selected_agent_id(&self) -> Option<String> {
        self.agents
            .get(self.selected_agent)
            .map(|agent| agent.id.clone())
    }
}

impl HasTimestamp for ClusterLogLine {
    fn timestamp_ms(&self) -> i64 {
        self.timestamp
    }
}

impl HasTimestamp for TimelineEvent {
    fn timestamp_ms(&self) -> i64 {
        self.timestamp
    }
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State, metrics: Option<&ClusterMetrics>) {
    let [metrics_area, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);

    render_metrics_line(frame, metrics_area, metrics);

    let [top, bottom] =
        Layout::vertical([Constraint::Percentage(30), Constraint::Percentage(70)]).areas(content);

    let [topo_area, agents_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(top);

    let [logs_area, timeline_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom);

    render_topology(frame, topo_area, state);
    render_agents(frame, agents_area, state);
    render_logs(frame, logs_area, state);
    render_timeline(frame, timeline_area, state);
}

fn render_metrics_line(frame: &mut Frame<'_>, area: Rect, metrics: Option<&ClusterMetrics>) {
    let line = Line::from(vec![
        Span::styled("Metrics:", theme::dim_style()),
        Span::raw(" "),
        Span::styled(metrics::format_metrics_line(metrics), theme::dim_style()),
    ]);
    let widget = Paragraph::new(line);
    frame.render_widget(widget, area);
}

fn render_topology(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let block = pane_block("Topology", state.focus == ClusterPane::Topology);
    topology::render(
        frame,
        area,
        block,
        state.summary.as_ref(),
        state.topology.as_ref(),
        state.topology_error.as_deref(),
    );
}

fn render_agents(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let title = format!("Agents ({})", state.agents.len());
    let block = pane_block(title, state.focus == ClusterPane::Agents);
    let inner = block.inner(area);

    if state.agents.is_empty() || inner.height == 0 {
        let lines = vec![
            Line::from(Span::styled("No agents yet.", theme::muted_style())),
            Line::from(Span::styled(
                "Wait for logs to identify agents.",
                theme::muted_style(),
            )),
        ];
        let widget = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(widget, area);
        return;
    }

    let items: Vec<ListItem> = state
        .agents
        .iter()
        .map(|agent| {
            let agent_color = theme::agent_color(&agent.id);
            let text = match &agent.role {
                Some(role) => format!("{} ({role})", agent.id),
                None => agent.id.clone(),
            };
            ListItem::new(text).style(Style::default().fg(agent_color))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(theme::selected_style())
        .highlight_symbol(" > ");

    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_agent));

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_logs(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let title = if state.logs.scroll_offset > 0 {
        format!("Logs (up {})", state.logs.scroll_offset)
    } else {
        "Logs".to_string()
    };
    let block = pane_block(title, state.focus == ClusterPane::Logs);
    let inner = block.inner(area);
    let height = inner.height as usize;

    let lines: Vec<Line> = if state.logs.is_empty() || height == 0 {
        stream::log_placeholder_lines(stream::LogPlaceholderContext::Cluster)
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

fn render_timeline(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let title = if state.timeline.scroll_offset > 0 {
        format!("Timeline (up {})", state.timeline.scroll_offset)
    } else {
        "Timeline".to_string()
    };
    let block = pane_block(title, state.focus == ClusterPane::Timeline);
    let inner = block.inner(area);
    let height = inner.height as usize;

    let lines: Vec<Line> = if state.timeline.is_empty() || height == 0 {
        stream::timeline_placeholder_lines()
    } else {
        let total = state.timeline.len();
        let max_start = total.saturating_sub(height);
        let start = max_start.saturating_sub(state.timeline.scroll_offset.min(max_start));
        state
            .timeline
            .items
            .iter()
            .skip(start)
            .take(height)
            .map(stream::format_timeline_event_styled)
            .collect()
    };

    let widget = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);

    // Scrollbar
    if !state.timeline.is_empty() && height > 0 {
        let total = state.timeline.len();
        let position = total
            .saturating_sub(height)
            .saturating_sub(state.timeline.scroll_offset);
        let mut scrollbar_state =
            ScrollbarState::new(total.saturating_sub(height)).position(position);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            inner,
            &mut scrollbar_state,
        );
    }
}

// shared stream formatters live in ui/widgets/stream.rs
