use std::collections::HashMap;

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Points};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::protocol::ClusterSummary;
use crate::ui::theme;

const POLL_INTERVAL_MS: i64 = 1000;
const WORLD_RADIUS: f64 = 48.0;
const LABEL_OFFSET: f64 = 6.0;
const RADIAL_LABEL_OFFSET: f64 = 2.0;
const BASE_ORB_RADIUS: f64 = 1.8;
const MAX_ORB_RADIUS: f64 = 4.6;
const ERROR_PULSE_RADIUS: f64 = 1.6;
const SELECTION_RING_RADIUS: f64 = 1.2;

const ACTIVITY_BANDS_MS: [i64; 4] = [5_000, 30_000, 120_000, 600_000];
const RING_RADII: [f64; 5] = [10.0, 20.0, 30.0, 40.0, 46.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutPosition {
    pub x: f64,
    pub y: f64,
    pub ring_radius: f64,
}

#[derive(Debug, Clone, Default)]
pub struct FleetRadarState {
    pub clusters: Vec<ClusterSummary>,
    pub selected: usize,
    pub last_poll_at: Option<i64>,
    pub last_message_counts: HashMap<String, i64>,
    pub last_activity_at: HashMap<String, i64>,
    pub last_message_deltas: HashMap<String, i64>,
    pub layout_angles: HashMap<String, f64>,
}

impl FleetRadarState {
    pub fn set_clusters(&mut self, clusters: Vec<ClusterSummary>, now_ms: i64) {
        let selected_id = self.selected_cluster_id();
        self.update_activity(&clusters, now_ms);
        self.ensure_angles(&clusters);
        self.clusters = clusters;
        self.reconcile_selection(selected_id);
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

    pub fn selected_cluster_id(&self) -> Option<String> {
        self.clusters
            .get(self.selected)
            .map(|cluster| cluster.id.clone())
    }

    pub fn activity_age_ms(&self, cluster: &ClusterSummary, now_ms: i64) -> i64 {
        let activity_at = self
            .last_activity_at
            .get(&cluster.id)
            .copied()
            .unwrap_or(cluster.created_at);
        now_ms.saturating_sub(activity_at)
    }

    pub fn activity_delta(&self, cluster_id: &str) -> i64 {
        self.last_message_deltas
            .get(cluster_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn layout_for(&self, cluster_id: &str, activity_age_ms: i64) -> LayoutPosition {
        let angle = self
            .layout_angles
            .get(cluster_id)
            .copied()
            .unwrap_or_else(|| stable_angle(cluster_id));
        layout_with_angle(angle, activity_age_ms)
    }

    fn update_activity(&mut self, clusters: &[ClusterSummary], now_ms: i64) {
        let mut next_counts = HashMap::new();
        let mut next_activity = HashMap::new();
        let mut next_deltas = HashMap::new();

        for cluster in clusters {
            let prev_count = self.last_message_counts.get(&cluster.id).copied();
            let prev_activity = self.last_activity_at.get(&cluster.id).copied();
            let delta = prev_count
                .map(|prev| cluster.message_count.saturating_sub(prev))
                .unwrap_or(cluster.message_count);
            let mut activity = prev_activity;

            if prev_count.map(|prev| cluster.message_count > prev).unwrap_or(true) {
                activity = Some(now_ms);
            }

            next_counts.insert(cluster.id.clone(), cluster.message_count);
            next_deltas.insert(cluster.id.clone(), delta);
            if let Some(activity_at) = activity {
                next_activity.insert(cluster.id.clone(), activity_at);
            }
        }

        self.last_message_counts = next_counts;
        self.last_activity_at = next_activity;
        self.last_message_deltas = next_deltas;
    }

    fn ensure_angles(&mut self, clusters: &[ClusterSummary]) {
        for cluster in clusters {
            self.layout_angles
                .entry(cluster.id.clone())
                .or_insert_with(|| stable_angle(cluster.id.as_str()));
        }
        self.layout_angles
            .retain(|id, _| clusters.iter().any(|cluster| cluster.id == *id));
    }

    fn reconcile_selection(&mut self, selected_id: Option<String>) {
        if self.clusters.is_empty() {
            self.selected = 0;
            return;
        }

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

        self.selected = 0;
    }
}

pub fn layout_position(cluster_id: &str, activity_age_ms: i64) -> LayoutPosition {
    let angle = stable_angle(cluster_id);
    layout_with_angle(angle, activity_age_ms)
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &FleetRadarState, now_ms: i64) {
    if state.clusters.is_empty() {
        render_empty(frame, area);
        return;
    }

    let selected_id = state.selected_cluster_id();
    let selected_id = selected_id.as_deref();

    let canvas = Canvas::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Fleet Radar"),
        )
        .x_bounds([-WORLD_RADIUS, WORLD_RADIUS])
        .y_bounds([-WORLD_RADIUS, WORLD_RADIUS])
        .marker(Marker::Braille)
        .paint(|ctx| {
            for ring in RING_RADII.iter().take(RING_RADII.len().saturating_sub(1)) {
                ctx.draw(&Circle {
                    x: 0.0,
                    y: 0.0,
                    radius: *ring,
                    color: theme::FG_DIM,
                });
            }

            for cluster in &state.clusters {
                let age_ms = state.activity_age_ms(cluster, now_ms);
                let delta = state.activity_delta(cluster.id.as_str());
                let layout = state.layout_for(cluster.id.as_str(), age_ms);
                let color = cluster_color(cluster);
                let orb_radius = orb_radius(delta, age_ms);
                let is_selected = selected_id == Some(cluster.id.as_str());
                let is_error = matches!(
                    cluster.state.as_str(),
                    "error" | "failed" | "failure"
                );

                if is_error {
                    ctx.draw(&Circle {
                        x: layout.x,
                        y: layout.y,
                        radius: orb_radius + ERROR_PULSE_RADIUS,
                        color: theme::STATUS_ERROR,
                    });
                }

                if is_selected {
                    ctx.draw(&Circle {
                        x: layout.x,
                        y: layout.y,
                        radius: orb_radius + SELECTION_RING_RADIUS,
                        color: theme::ACCENT,
                    });
                }

                ctx.draw(&Circle {
                    x: layout.x,
                    y: layout.y,
                    radius: orb_radius,
                    color,
                });

                ctx.draw(&Points {
                    coords: &[(layout.x, layout.y)],
                    color,
                });

                let label = truncate_label(cluster.id.as_str());
                let label_style = Style::default().fg(color);
                let line = Line::from(Span::styled(label, label_style));
                let (label_x, label_y) = label_position(layout.x, layout.y, cluster.id.as_str());
                ctx.print(label_x, label_y, line);
            }
        });

    frame.render_widget(canvas, area);
}

fn render_empty(frame: &mut Frame<'_>, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No clusters on the radar",
            theme::muted_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Start a cluster from the Launcher (Esc)",
            theme::dim_style(),
        )),
    ];
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Fleet Radar"),
        );
    frame.render_widget(widget, area);
}

fn activity_ring(age_ms: i64) -> f64 {
    let age = if age_ms < 0 { 0 } else { age_ms };
    for (index, cutoff) in ACTIVITY_BANDS_MS.iter().enumerate() {
        if age <= *cutoff {
            return RING_RADII[index];
        }
    }
    *RING_RADII.last().unwrap_or(&WORLD_RADIUS)
}

fn layout_with_angle(angle: f64, activity_age_ms: i64) -> LayoutPosition {
    let ring_radius = activity_ring(activity_age_ms);
    let x = ring_radius * angle.cos();
    let y = ring_radius * angle.sin();
    LayoutPosition { x, y, ring_radius }
}

fn label_position(x: f64, y: f64, cluster_id: &str) -> (f64, f64) {
    let hash = stable_hash(cluster_id);
    let side = if hash & 1 == 0 { 1.0 } else { -1.0 };
    let angle = if x == 0.0 && y == 0.0 {
        0.0
    } else {
        y.atan2(x)
    };
    let tangent_x = -angle.sin();
    let tangent_y = angle.cos();
    let radial_x = -angle.cos();
    let radial_y = -angle.sin();
    let mut lx = x + tangent_x * LABEL_OFFSET * side + radial_x * RADIAL_LABEL_OFFSET;
    let mut ly = y + tangent_y * LABEL_OFFSET * side + radial_y * RADIAL_LABEL_OFFSET;
    let min = -WORLD_RADIUS + 1.0;
    let max = WORLD_RADIUS - 1.0;
    if lx < min {
        lx = min;
    } else if lx > max {
        lx = max;
    }
    if ly < min {
        ly = min;
    } else if ly > max {
        ly = max;
    }
    (lx, ly)
}

fn orb_radius(delta: i64, age_ms: i64) -> f64 {
    let delta_boost = (delta.max(0) as f64).min(6.0) * 0.35;
    let recency_boost = if age_ms <= 5_000 {
        0.9
    } else if age_ms <= 30_000 {
        0.4
    } else {
        0.0
    };
    (BASE_ORB_RADIUS + delta_boost + recency_boost).min(MAX_ORB_RADIUS)
}

fn truncate_label(id: &str) -> String {
    const LIMIT: usize = 10;
    if id.len() <= LIMIT {
        id.to_string()
    } else {
        format!("{}..", &id[..LIMIT])
    }
}

fn cluster_color(cluster: &ClusterSummary) -> Color {
    theme::status_style(&cluster.state)
        .fg
        .unwrap_or(theme::FG_MUTED)
}

fn stable_angle(input: &str) -> f64 {
    let hash = stable_hash(input);
    let fraction = (hash % 3600) as f64 / 3600.0;
    std::f64::consts::TAU * fraction
}

fn stable_hash(input: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;
    let mut hash = FNV_OFFSET;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_position_is_deterministic() {
        let first = layout_position("cluster-1", 10_000);
        let second = layout_position("cluster-1", 10_000);
        assert_eq!(first, second);
    }

    #[test]
    fn layout_position_changes_with_age_band() {
        let recent = layout_position("cluster-1", 1_000);
        let older = layout_position("cluster-1", 1_000_000);
        assert!(recent.ring_radius < older.ring_radius);
    }
}
