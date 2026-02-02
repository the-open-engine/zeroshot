use std::collections::HashMap;

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Points};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::app::animation::{self, AnimClock};
use crate::app::Camera;
use crate::protocol::ClusterSummary;
use crate::ui::shared::calm_empty_state;
use crate::ui::theme;

const POLL_INTERVAL_MS: i64 = 1000;
const WORLD_RADIUS: f64 = 48.0;
const LABEL_OFFSET: f64 = 6.0;
const RADIAL_LABEL_OFFSET: f64 = 2.0;
const BASE_ORB_RADIUS: f64 = 1.8;
const MAX_ORB_RADIUS: f64 = 4.6;
const ERROR_PULSE_RADIUS: f64 = 1.6;
const SELECTION_RING_RADIUS: f64 = 1.2;
const PIN_RING_RADIUS: f64 = 2.2;
const MIN_CAMERA_ZOOM: f32 = 0.2;
const ORB_SMOOTH_RATE: f64 = 0.25;

const ACTIVITY_BANDS_MS: [i64; 4] = [5_000, 30_000, 120_000, 600_000];
const RING_RADII: [f64; 5] = [10.0, 20.0, 30.0, 40.0, 46.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutPosition {
    pub x: f64,
    pub y: f64,
    pub ring_radius: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbVisual {
    pub radius: f64,
    pub intensity: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveSpeed {
    Step,
    Fast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    MoveSelection {
        direction: Direction,
        speed: MoveSpeed,
    },
    CenterOnSelection,
    ResetView,
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
    pub orb_states: HashMap<String, OrbVisual>,
}

impl FleetRadarState {
    pub fn set_clusters(&mut self, clusters: Vec<ClusterSummary>, now_ms: i64) {
        let selected_id = self.selected_cluster_id();
        self.update_activity(&clusters, now_ms);
        self.ensure_angles(&clusters);
        self.ensure_orb_states(&clusters, now_ms);
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

    pub fn selected_layout(&self, now_ms: i64) -> Option<LayoutPosition> {
        let cluster = self.clusters.get(self.selected)?;
        let age_ms = self.activity_age_ms(cluster, now_ms);
        Some(self.layout_for(cluster.id.as_str(), age_ms))
    }

    pub fn move_selection_direction(
        &mut self,
        now_ms: i64,
        direction: Direction,
        speed: MoveSpeed,
    ) -> bool {
        let steps = match speed {
            MoveSpeed::Step => 1,
            MoveSpeed::Fast => 2,
        };
        let mut moved = false;
        for _ in 0..steps {
            if self.move_selection_step(now_ms, direction) {
                moved = true;
            } else {
                break;
            }
        }
        moved
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

    pub fn tick_orb_smoothing(&mut self, now_ms: i64, dt_ms: i64) {
        if self.clusters.is_empty() {
            return;
        }
        for cluster in &self.clusters {
            let (target_radius, target_intensity) = orb_targets(self, cluster, now_ms);
            let entry = self
                .orb_states
                .entry(cluster.id.clone())
                .or_insert(OrbVisual {
                    radius: target_radius,
                    intensity: target_intensity,
                });
            entry.radius =
                animation::smooth_toward_f64(entry.radius, target_radius, dt_ms, ORB_SMOOTH_RATE);
            entry.intensity = animation::smooth_toward_f64(
                entry.intensity,
                target_intensity,
                dt_ms,
                ORB_SMOOTH_RATE,
            );
        }
    }

    pub fn orb_visual(&self, cluster: &ClusterSummary, now_ms: i64) -> OrbVisual {
        self.orb_states
            .get(&cluster.id)
            .copied()
            .unwrap_or_else(|| {
                let (radius, intensity) = orb_targets(self, cluster, now_ms);
                OrbVisual { radius, intensity }
            })
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

            if prev_count
                .map(|prev| cluster.message_count > prev)
                .unwrap_or(true)
            {
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

    fn ensure_orb_states(&mut self, clusters: &[ClusterSummary], now_ms: i64) {
        for cluster in clusters {
            let (radius, intensity) = orb_targets(self, cluster, now_ms);
            self.orb_states
                .entry(cluster.id.clone())
                .or_insert(OrbVisual { radius, intensity });
        }
        self.orb_states
            .retain(|id, _| clusters.iter().any(|cluster| cluster.id == *id));
    }

    fn reconcile_selection(&mut self, selected_id: Option<String>) {
        if self.clusters.is_empty() {
            self.selected = 0;
            return;
        }

        if let Some(id) = selected_id {
            if let Some(index) = self.clusters.iter().position(|cluster| cluster.id == id) {
                self.selected = index;
                return;
            }
        }

        self.selected = 0;
    }

    fn move_selection_step(&mut self, now_ms: i64, direction: Direction) -> bool {
        if self.clusters.is_empty() {
            self.selected = 0;
            return false;
        }
        if self.selected >= self.clusters.len() {
            self.selected = self.clusters.len().saturating_sub(1);
        }

        let current_cluster = &self.clusters[self.selected];
        let current_layout = self.layout_for(
            current_cluster.id.as_str(),
            self.activity_age_ms(current_cluster, now_ms),
        );

        let mut best: Option<(usize, f64, f64)> = None;

        for (idx, cluster) in self.clusters.iter().enumerate() {
            if idx == self.selected {
                continue;
            }
            let layout =
                self.layout_for(cluster.id.as_str(), self.activity_age_ms(cluster, now_ms));
            let dx = layout.x - current_layout.x;
            let dy = layout.y - current_layout.y;

            let (axis, off) = match direction {
                Direction::Right if dx > 0.0 => (dx, dy.abs()),
                Direction::Left if dx < 0.0 => (-dx, dy.abs()),
                Direction::Up if dy > 0.0 => (dy, dx.abs()),
                Direction::Down if dy < 0.0 => (-dy, dx.abs()),
                _ => continue,
            };

            let angle_score = off / axis;
            let dist2 = dx * dx + dy * dy;
            let replace = match best {
                None => true,
                Some((_best_idx, best_angle, best_dist)) => {
                    const EPS: f64 = 1e-6;
                    if (angle_score - best_angle).abs() > EPS {
                        angle_score < best_angle
                    } else if (dist2 - best_dist).abs() > EPS {
                        dist2 < best_dist
                    } else {
                        idx < _best_idx
                    }
                }
            };

            if replace {
                best = Some((idx, angle_score, dist2));
            }
        }

        if let Some((idx, _, _)) = best {
            self.selected = idx;
            true
        } else {
            false
        }
    }
}

pub fn layout_position(cluster_id: &str, activity_age_ms: i64) -> LayoutPosition {
    let angle = stable_angle(cluster_id);
    layout_with_angle(angle, activity_age_ms)
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &FleetRadarState,
    camera: &Camera,
    now_ms: i64,
    anim_clock: &AnimClock,
    pinned_cluster_id: Option<&str>,
) {
    if state.clusters.is_empty() {
        render_empty(frame, area);
        return;
    }

    let selected_id = state.selected_cluster_id();
    let selected_id = selected_id.as_deref();
    let pinned_id = pinned_cluster_id;
    let zoom = camera.zoom.max(MIN_CAMERA_ZOOM);
    let half_span = WORLD_RADIUS / zoom as f64;
    let center_x = camera.position.0 as f64;
    let center_y = camera.position.1 as f64;
    let pulse = animation::pulse_factor(anim_clock.phase) as f64;

    let canvas = Canvas::default()
        .block(Block::default().borders(Borders::ALL).title("Fleet Radar"))
        .x_bounds([center_x - half_span, center_x + half_span])
        .y_bounds([center_y - half_span, center_y + half_span])
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
                let layout = state.layout_for(cluster.id.as_str(), age_ms);
                let color = cluster_color(cluster);
                let orb = state.orb_visual(cluster, now_ms);
                let orb_radius = orb.radius;
                let intensity = orb.intensity;
                let is_selected = selected_id == Some(cluster.id.as_str());
                let is_pinned = pinned_id == Some(cluster.id.as_str());
                let is_error = matches!(cluster.state.as_str(), "error" | "failed" | "failure");

                if is_error {
                    let intensity_scale = 0.6 + 0.4 * intensity;
                    let pulse_scale = 0.7 + 0.6 * pulse;
                    ctx.draw(&Circle {
                        x: layout.x,
                        y: layout.y,
                        radius: orb_radius + ERROR_PULSE_RADIUS * intensity_scale * pulse_scale,
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

                if is_pinned {
                    ctx.draw(&Circle {
                        x: layout.x,
                        y: layout.y,
                        radius: orb_radius + PIN_RING_RADIUS,
                        color: theme::ACCENT2,
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
    let widget = calm_empty_state(
        "Fleet Radar",
        "No clusters yet.",
        Some("Type an intent in the spine to start a cluster."),
        None,
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

fn orb_intensity(delta: i64, age_ms: i64) -> f64 {
    let delta_norm = (delta.max(0) as f64).min(6.0) / 6.0;
    let recency = if age_ms <= 5_000 {
        1.0
    } else if age_ms <= 30_000 {
        0.6
    } else {
        0.3
    };
    (0.3 + delta_norm * 0.5 + recency * 0.2).min(1.0)
}

fn orb_targets(state: &FleetRadarState, cluster: &ClusterSummary, now_ms: i64) -> (f64, f64) {
    let age_ms = state.activity_age_ms(cluster, now_ms);
    let delta = state.activity_delta(cluster.id.as_str());
    (orb_radius(delta, age_ms), orb_intensity(delta, age_ms))
}

fn truncate_label(id: &str) -> String {
    const LIMIT: usize = 10;
    let mut iter = id.chars();
    let mut out = String::new();
    for _ in 0..LIMIT {
        match iter.next() {
            Some(ch) => out.push(ch),
            None => return id.to_string(),
        }
    }
    if iter.next().is_some() {
        out.push_str("..");
    }
    out
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
    use crate::ui::widgets::test_utils::line_text;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn cluster(id: &str) -> ClusterSummary {
        ClusterSummary {
            id: id.to_string(),
            state: "running".to_string(),
            provider: None,
            created_at: 0,
            agent_count: 1,
            message_count: 0,
            cwd: None,
        }
    }

    fn buffer_contains(buffer: &Buffer, needle: &str) -> bool {
        for y in 0..buffer.area.height {
            if line_text(buffer, y).contains(needle) {
                return true;
            }
        }
        false
    }

    #[test]
    fn fleet_radar_renders_empty_state() {
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = FleetRadarState::default();
        let camera = Camera::default();
        let anim_clock = AnimClock::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &state, &camera, 0, &anim_clock, None);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, "No clusters yet."));
        assert!(buffer_contains(
            buffer,
            "Type an intent in the spine to start a cluster."
        ));
    }

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

    #[test]
    fn truncate_label_handles_unicode() {
        let label = "αβγδεζηθικλμ";
        let truncated = truncate_label(label);
        assert_eq!(truncated, "αβγδεζηθικ..");
    }

    #[test]
    fn radar_directional_selection() {
        let now_ms = 10_000;
        let mut state = FleetRadarState::default();
        state.set_clusters(
            vec![
                cluster("east"),
                cluster("west"),
                cluster("north"),
                cluster("south"),
            ],
            now_ms,
        );
        state.layout_angles.insert("east".to_string(), 0.0);
        state
            .layout_angles
            .insert("north".to_string(), std::f64::consts::FRAC_PI_2);
        state
            .layout_angles
            .insert("west".to_string(), std::f64::consts::PI);
        state
            .layout_angles
            .insert("south".to_string(), std::f64::consts::TAU * 0.75);

        state.selected = state
            .clusters
            .iter()
            .position(|cluster| cluster.id == "west")
            .unwrap();

        assert!(state.move_selection_direction(now_ms, Direction::Right, MoveSpeed::Step));
        assert_eq!(state.selected_cluster_id().as_deref(), Some("east"));

        assert!(state.move_selection_direction(now_ms, Direction::Up, MoveSpeed::Step));
        assert_eq!(state.selected_cluster_id().as_deref(), Some("north"));

        assert!(state.move_selection_direction(now_ms, Direction::Down, MoveSpeed::Step));
        assert_eq!(state.selected_cluster_id().as_deref(), Some("south"));

        assert!(!state.move_selection_direction(now_ms, Direction::Down, MoveSpeed::Step));
        assert_eq!(state.selected_cluster_id().as_deref(), Some("south"));
    }
}
