use std::collections::HashMap;

use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::protocol::{ClusterMetrics, ClusterSummary};
use crate::screens::metrics;
use crate::ui::theme;

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

            if prev_count
                .map(|prev| cluster.message_count > prev)
                .unwrap_or(true)
            {
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
            if let Some(index) = self.clusters.iter().position(|cluster| cluster.id == id) {
                self.selected = index;
                return;
            }
        }

        if self.selected >= self.clusters.len() {
            self.selected = self.clusters.len().saturating_sub(1);
        }
    }
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &State,
    metrics_map: &HashMap<String, ClusterMetrics>,
    now_ms: i64,
) {
    // Empty state
    if state.clusters.is_empty() {
        render_empty(frame, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("STATE"),
        Cell::from("PROVIDER"),
        Cell::from("CPU%"),
        Cell::from("MEM"),
        Cell::from("DURATION"),
        Cell::from("LAST"),
    ])
    .style(theme::table_header_style());

    let rows: Vec<Row> = state
        .clusters
        .iter()
        .map(|cluster| {
            let provider = cluster.provider.clone().unwrap_or_else(|| "-".to_string());
            let metrics = metrics_map.get(&cluster.id);
            let cpu = metrics::format_cpu_percent(metrics);
            let mem = metrics::format_memory_mb(metrics);
            let duration = format_duration(now_ms.saturating_sub(cluster.created_at));
            let last_activity = state
                .last_activity_at
                .get(&cluster.id)
                .map(|activity| format_duration(now_ms.saturating_sub(*activity)))
                .unwrap_or_else(|| "-".to_string());

            let state_style = theme::status_style(&cluster.state);
            let is_done = matches!(
                cluster.state.as_str(),
                "done" | "completed" | "complete" | "stopped"
            );
            let row_style = if is_done {
                theme::done_row_style()
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(cluster.id.clone()),
                Cell::from(Span::styled(cluster.state.clone(), state_style)),
                Cell::from(provider),
                Cell::from(cpu),
                Cell::from(mem),
                Cell::from(duration),
                Cell::from(last_activity),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(18),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(metrics::CPU_COLUMN_WIDTH as u16),
            Constraint::Length(metrics::MEM_COLUMN_WIDTH as u16),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .row_highlight_style(theme::selected_style())
    .highlight_symbol(" > ");

    let mut table_state = TableState::default();
    table_state.select(Some(state.selected));

    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_empty(frame: &mut Frame<'_>, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled("No active clusters", theme::muted_style())),
        Line::from(""),
        Line::from(Span::styled(
            "Start a cluster from the Launcher (Esc)",
            theme::dim_style(),
        )),
        Line::from(Span::styled(
            "or run: zeroshot run <issue> --ship",
            theme::dim_style(),
        )),
    ];
    let widget = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(widget, area);
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
