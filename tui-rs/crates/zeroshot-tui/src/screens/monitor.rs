use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::protocol::ClusterSummary;

#[derive(Debug, Clone, Default)]
pub struct State {
    pub clusters: Vec<ClusterSummary>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    MoveSelection(i32),
    OpenSelected,
}

impl State {
    pub fn set_clusters(&mut self, clusters: Vec<ClusterSummary>) {
        self.clusters = clusters;
        if self.selected >= self.clusters.len() {
            self.selected = self.clusters.len().saturating_sub(1);
        }
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
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let mut lines = vec![Line::from("Monitor"), Line::from("Clusters:")];
    if state.clusters.is_empty() {
        lines.push(Line::from("(none)"));
    } else {
        for (idx, cluster) in state.clusters.iter().enumerate() {
            let marker = if idx == state.selected { ">" } else { " " };
            lines.push(Line::from(format!("{marker} {} ({})", cluster.id, cluster.state)));
        }
    }

    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}
