use std::collections::VecDeque;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::protocol::{ClusterLogLine, ClusterSummary, ClusterTopology, TimelineEvent};
use crate::ui::widgets::topology;

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
    pub logs: VecDeque<ClusterLogLine>,
    pub timeline: VecDeque<TimelineEvent>,
    pub log_scroll_offset: usize,
    pub timeline_scroll_offset: usize,
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
            logs: VecDeque::new(),
            timeline: VecDeque::new(),
            log_scroll_offset: 0,
            timeline_scroll_offset: 0,
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
            ClusterPane::Logs => self.move_log_scroll(delta),
            ClusterPane::Timeline => self.move_timeline_scroll(delta),
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

    pub fn push_timeline_events(&mut self, mut events: Vec<TimelineEvent>) {
        let added = events.len();
        self.timeline.extend(events.drain(..));
        Self::adjust_scroll_on_append(&mut self.timeline_scroll_offset, added);
        let dropped = trim_vecdeque(&mut self.timeline, MAX_TIMELINE_EVENTS);
        Self::adjust_scroll_on_trim(&mut self.timeline_scroll_offset, dropped);
        Self::clamp_scroll(&mut self.timeline_scroll_offset, self.timeline.len());
    }

    fn move_log_scroll(&mut self, delta: i32) {
        Self::move_scroll(&mut self.log_scroll_offset, delta, self.logs.len());
    }

    fn move_timeline_scroll(&mut self, delta: i32) {
        Self::move_scroll(&mut self.timeline_scroll_offset, delta, self.timeline.len());
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

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(area);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);

    render_topology(frame, top[0], state);
    render_agents(frame, top[1], state);
    render_logs(frame, bottom[0], state);
    render_timeline(frame, bottom[1], state);
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
    let block = pane_block("Agents", state.focus == ClusterPane::Agents);
    let inner = block.inner(area);
    let height = inner.height as usize;
    let mut lines = Vec::new();

    if state.agents.is_empty() || height == 0 {
        lines.push(Line::from("(no agents yet)"));
    } else {
        let start = agent_scroll_start(state.selected_agent, state.agents.len(), height);
        for (idx, agent) in state
            .agents
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
        {
            let prefix = if idx == state.selected_agent { "> " } else { "  " };
            let line = match &agent.role {
                Some(role) => format!("{prefix}{} ({role})", agent.id),
                None => format!("{prefix}{}", agent.id),
            };
            lines.push(Line::from(line));
        }
    }

    let widget = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

fn render_logs(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let title = if state.log_scroll_offset > 0 {
        format!("Logs (scroll {})", state.log_scroll_offset)
    } else {
        "Logs".to_string()
    };
    let block = pane_block(title, state.focus == ClusterPane::Logs);
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

fn render_timeline(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let title = if state.timeline_scroll_offset > 0 {
        format!("Timeline (scroll {})", state.timeline_scroll_offset)
    } else {
        "Timeline".to_string()
    };
    let block = pane_block(title, state.focus == ClusterPane::Timeline);
    let inner = block.inner(area);
    let height = inner.height as usize;

    let lines = if state.timeline.is_empty() || height == 0 {
        vec![Line::from("(no timeline events)")]
    } else {
        let total = state.timeline.len();
        let max_start = total.saturating_sub(height);
        let start = max_start.saturating_sub(state.timeline_scroll_offset.min(max_start));
        state
            .timeline
            .iter()
            .skip(start)
            .take(height)
            .map(format_timeline_event)
            .map(Line::from)
            .collect()
    };

    let widget = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

fn pane_block<'a>(title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    let style = if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
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

fn format_timeline_event(event: &TimelineEvent) -> String {
    if let Some(sender) = event.sender.as_deref() {
        format!("{} - {} ({})", event.topic, event.label, sender)
    } else {
        format!("{} - {}", event.topic, event.label)
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

fn agent_scroll_start(selected: usize, len: usize, height: usize) -> usize {
    if len <= height {
        return 0;
    }
    let end = selected.saturating_add(1);
    if end > height {
        end.saturating_sub(height)
    } else {
        0
    }
}
