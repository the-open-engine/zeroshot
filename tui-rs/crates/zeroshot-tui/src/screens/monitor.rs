use std::collections::HashMap;

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::protocol::ClusterSummary;

const POLL_INTERVAL_MS: i64 = 1000;

#[derive(Debug, Clone, Default)]
pub struct State {
    pub clusters: Vec<ClusterSummary>,
    pub selected: usize,
    pub last_poll_at: Option<i64>,
    pub last_message_counts: HashMap<String, i64>,
    pub last_activity_at: HashMap<String, i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    MoveSelection(i32),
    OpenSelected,
}

impl State {
    pub fn set_clusters(&mut self, clusters: Vec<ClusterSummary>, now_ms: i64) {
        let selected_id = self.selected_cluster_id();
        self.update_activity(&clusters, now_ms);
        self.clusters = clusters;
        self.reconcile_selection(selected_id);
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.clusters.is_empty() {
            self.selected = 0;
            return;
        }

        let len = self.clusters.len() as i32;
        let mut next = self.selected as i32 + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        self.selected = next as usize;
    }

    pub fn selected_cluster_id(&self) -> Option<String> {
        self.clusters
            .get(self.selected)
            .map(|cluster| cluster.id.clone())
    }

    pub fn poll_due(&self, now_ms: i64) -> bool {
        match self.last_poll_at {
            None => true,
            Some(last) => now_ms.saturating_sub(last) >= POLL_INTERVAL_MS,
        }
    }

    pub fn mark_polled(&mut self, now_ms: i64) {
        self.last_poll_at = Some(now_ms);
    }

    fn update_activity(&mut self, clusters: &[ClusterSummary], now_ms: i64) {
        let mut next_counts = HashMap::new();
        let mut next_activity = HashMap::new();

        for cluster in clusters {
            let prev_count = self.last_message_counts.get(&cluster.id).copied();
            let prev_activity = self.last_activity_at.get(&cluster.id).copied();
            let mut activity = prev_activity;

            if prev_count.map(|prev| cluster.message_count > prev).unwrap_or(true) {
                activity = Some(now_ms);
            }

            next_counts.insert(cluster.id.clone(), cluster.message_count);
            if let Some(activity_at) = activity {
                next_activity.insert(cluster.id.clone(), activity_at);
            }
        }

        self.last_message_counts = next_counts;
        self.last_activity_at = next_activity;
    }

    fn reconcile_selection(&mut self, selected_id: Option<String>) {
        if let Some(id) = selected_id {
            if let Some(index) = self
                .clusters
                .iter()
                .position(|cluster| cluster.id == id)
            {
                self.selected = index;
                return;
            }
        }

        if self.selected >= self.clusters.len() {
            self.selected = self.clusters.len().saturating_sub(1);
        }
    }
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State, now_ms: i64) {
    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("State"),
        Cell::from("Provider"),
        Cell::from("Duration"),
        Cell::from("Last activity"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = state.clusters.iter().map(|cluster| {
        let provider = cluster
            .provider
            .clone()
            .unwrap_or_else(|| "-".to_string());
        let duration = format_duration(now_ms.saturating_sub(cluster.created_at));
        let last_activity = state
            .last_activity_at
            .get(&cluster.id)
            .map(|activity| format!("{} ago", format_duration(now_ms.saturating_sub(*activity))))
            .unwrap_or_else(|| "-".to_string());

        Row::new(vec![
            Cell::from(cluster.id.clone()),
            Cell::from(cluster.state.clone()),
            Cell::from(provider),
            Cell::from(duration),
            Cell::from(last_activity),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(36),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(14),
        ],
    )
    .header(header)
    .block(Block::default().title("Monitor").borders(Borders::ALL))
    .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .highlight_symbol(">> ");

    let mut table_state = TableState::default();
    if state.clusters.is_empty() {
        table_state.select(None);
    } else {
        table_state.select(Some(state.selected));
    }

    frame.render_stateful_widget(table, area, &mut table_state);
}

fn format_duration(delta_ms: i64) -> String {
    let seconds = if delta_ms < 0 { 0 } else { delta_ms } / 1000;
    if seconds < 60 {
        return format!("{}s", seconds);
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{}m", minutes);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{}h", hours);
    }
    let days = hours / 24;
    format!("{}d", days)
}
