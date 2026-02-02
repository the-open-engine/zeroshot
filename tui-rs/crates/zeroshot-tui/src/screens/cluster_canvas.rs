use std::collections::HashMap;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Points};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::app::animation::AnimClock;
use crate::app::{animation, FocusTarget, TimeCursor};
use crate::protocol::{ClusterTopology, TopologyAgent};
use crate::screens::cluster;
use crate::ui::shared::calm_empty_state;
use crate::ui::theme;
use crate::ui::widgets::stream::{self, StreamOverlay};

const WORLD_RADIUS: f64 = 48.0;
const AGENT_RING_RADIUS: f64 = 28.0;
const TOPIC_RING_RADIUS: f64 = 14.0;
const AGENT_ORB_RADIUS: f64 = 1.8;
const TOPIC_ORB_RADIUS: f64 = 1.2;
const LABEL_OFFSET: f64 = 4.0;
const LABEL_RADIAL_OFFSET: f64 = 1.4;
const LABEL_LIMIT: usize = 14;
const PENDING_MESSAGE: &str = "Topology pending";
const UNAVAILABLE_MESSAGE: &str = "Topology unavailable";
const FOCUS_EPSILON: f64 = 0.0001;
const CANVAS_CAMERA_ACCEL: f64 = 0.16;
const CANVAS_CAMERA_FRICTION: f64 = 0.82;
const CANVAS_CAMERA_SNAP_EPSILON: f64 = 0.08;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Agent,
    Topic,
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
    MoveFocus {
        direction: Direction,
        speed: MoveSpeed,
    },
    ZoomIn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeLayout {
    pub id: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutBounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutCache {
    pub nodes: HashMap<String, NodeLayout>,
    pub edges: Vec<LayoutEdge>,
    pub bounds: LayoutBounds,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub focused_id: Option<String>,
    pub layout: Option<LayoutCache>,
    pub log_subscription: Option<String>,
    pub timeline_subscription: Option<String>,
    pub camera: (f64, f64),
    pub camera_target: (f64, f64),
    pub camera_velocity: (f64, f64),
}

impl State {
    pub fn update_layout(&mut self, topology: &ClusterTopology) {
        let had_layout = self.layout.is_some();
        self.layout = Some(layout_for(topology));
        self.ensure_focus(topology);
        if had_layout {
            self.set_camera_target_to_focus();
        } else {
            self.snap_camera_to_focus();
        }
    }

    pub fn ensure_focus(&mut self, topology: &ClusterTopology) {
        let mut needs_focus = self.focused_id.is_none();
        if let Some(focused) = self.focused_id.as_ref() {
            let in_agents = topology.agents.iter().any(|agent| agent.id == *focused);
            let in_topics = topology.topics.iter().any(|topic| topic == focused);
            if !in_agents && !in_topics {
                needs_focus = true;
            }
        }
        if needs_focus {
            self.focused_id = default_focus_id(topology);
        }
    }

    pub fn move_focus(&mut self, direction: Direction, speed: MoveSpeed) {
        if self.layout.is_none() {
            return;
        }
        if let Some(layout) = self.layout.as_ref() {
            match self.focused_id.as_ref() {
                Some(focused) => {
                    if !layout.nodes.contains_key(focused) {
                        self.focused_id = default_focus_id_from_layout(layout);
                    }
                }
                None => {
                    self.focused_id = default_focus_id_from_layout(layout);
                }
            }
        }
        if self.focused_id.is_none() {
            return;
        }

        self.set_camera_target_to_focus();

        let mut steps = match speed {
            MoveSpeed::Step => 1,
            MoveSpeed::Fast => 2,
        };
        while steps > 0 {
            let Some(next) = self.next_focus_id(direction) else {
                break;
            };
            self.focused_id = Some(next);
            self.set_camera_target_to_focus();
            steps -= 1;
        }
    }

    pub fn focused_agent_id(&self) -> Option<String> {
        let layout = self.layout.as_ref()?;
        let focused = self.focused_id.as_ref()?;
        let node = layout.nodes.get(focused)?;
        if node.kind == NodeKind::Agent {
            Some(node.id.clone())
        } else {
            None
        }
    }

    pub fn clear_layout(&mut self) {
        self.layout = None;
        self.camera = (0.0, 0.0);
        self.camera_target = (0.0, 0.0);
        self.camera_velocity = (0.0, 0.0);
    }

    pub fn tick_camera(&mut self, dt_ms: i64) {
        let (position, velocity) = animation::step_spring_f64(
            self.camera,
            self.camera_velocity,
            self.camera_target,
            dt_ms,
            CANVAS_CAMERA_ACCEL,
            CANVAS_CAMERA_FRICTION,
        );
        self.camera = position;
        self.camera_velocity = velocity;

        let dx = self.camera.0 - self.camera_target.0;
        let dy = self.camera.1 - self.camera_target.1;
        if dx.abs() <= CANVAS_CAMERA_SNAP_EPSILON && dy.abs() <= CANVAS_CAMERA_SNAP_EPSILON {
            self.camera = self.camera_target;
            self.camera_velocity = (0.0, 0.0);
        }
    }

    fn next_focus_id(&self, direction: Direction) -> Option<String> {
        let layout = self.layout.as_ref()?;
        let focused_id = self.focused_id.as_ref()?;
        let focused = layout.nodes.get(focused_id)?;
        let (dir_x, dir_y) = direction_vector(direction);
        let mut best: Option<(f64, String)> = None;

        for node in layout.nodes.values() {
            if node.id == focused.id {
                continue;
            }
            let dx = node.x - focused.x;
            let dy = node.y - focused.y;
            let dot = dx * dir_x + dy * dir_y;
            if dot <= FOCUS_EPSILON {
                continue;
            }
            let dist = dx * dx + dy * dy;
            match &mut best {
                Some((best_dist, best_id)) => {
                    if dist + FOCUS_EPSILON < *best_dist
                        || (dist - *best_dist).abs() <= FOCUS_EPSILON && node.id < *best_id
                    {
                        *best_dist = dist;
                        *best_id = node.id.clone();
                    }
                }
                None => best = Some((dist, node.id.clone())),
            }
        }

        best.map(|(_, id)| id)
    }

    fn set_camera_target_to_focus(&mut self) {
        let Some((x, y)) = self.focus_position() else {
            return;
        };
        self.camera_target = (x, y);
    }

    fn snap_camera_to_focus(&mut self) {
        let Some((x, y)) = self.focus_position() else {
            return;
        };
        self.camera = (x, y);
        self.camera_target = (x, y);
        self.camera_velocity = (0.0, 0.0);
    }

    fn focus_position(&self) -> Option<(f64, f64)> {
        let Some(layout) = self.layout.as_ref() else {
            return None;
        };
        let Some(focused_id) = self.focused_id.as_ref() else {
            return None;
        };
        let Some(node) = layout.nodes.get(focused_id) else {
            return None;
        };
        Some((node.x, node.y))
    }
}

struct OverlayTarget<'a> {
    id: &'a str,
    force_cluster: bool,
}

pub struct RenderContext<'a> {
    pub cluster_id: &'a str,
    pub cluster_state: Option<&'a cluster::State>,
    pub canvas_state: Option<&'a State>,
    pub time_cursor: &'a TimeCursor,
    pub anim_clock: &'a AnimClock,
    pub pinned_target: Option<&'a FocusTarget>,
}

pub fn render(frame: &mut Frame<'_>, area: Rect, context: RenderContext<'_>) {
    let RenderContext {
        cluster_id,
        cluster_state,
        canvas_state,
        time_cursor,
        anim_clock,
        pinned_target,
    } = context;
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Cluster Canvas");

    let Some(cluster_state) = cluster_state else {
        render_placeholder(frame, area, UNAVAILABLE_MESSAGE, None);
        return;
    };

    if let Some(error) = cluster_state.topology_error.as_deref() {
        render_placeholder(frame, area, UNAVAILABLE_MESSAGE, Some(error));
        return;
    }

    let Some(topology) = cluster_state.topology.as_ref() else {
        render_placeholder(frame, area, PENDING_MESSAGE, None);
        return;
    };

    let fallback_layout;
    let layout = match canvas_state.and_then(|state| state.layout.as_ref()) {
        Some(layout) => layout,
        None => {
            fallback_layout = layout_for(topology);
            &fallback_layout
        }
    };

    let focused = canvas_state.and_then(|state| state.focused_id.as_deref());
    let pinned_highlight = match pinned_target {
        Some(FocusTarget::Agent {
            cluster_id: pinned_cluster_id,
            agent_id,
        }) if *pinned_cluster_id == cluster_id => Some(agent_id.as_str()),
        _ => None,
    };
    let pinned_overlay =
        resolve_pinned_overlay(pinned_target, cluster_id, canvas_state, Some(cluster_state));
    let camera = canvas_state.map(|state| state.camera).unwrap_or((0.0, 0.0));
    let focus_pulse = animation::pulse_factor(anim_clock.phase) as f64;

    render_canvas(
        frame,
        CanvasRenderContext {
            area,
            cluster_id,
            cluster_state,
            topology,
            layout,
            focused,
            pinned_highlight,
            pinned_overlay,
            camera,
            block,
            time_cursor,
            focus_pulse,
        },
    );
}

fn render_placeholder(frame: &mut Frame<'_>, area: Rect, headline: &str, detail: Option<&str>) {
    let widget = calm_empty_state(
        "Cluster Canvas",
        headline,
        detail,
        Some("Press Esc to return to Fleet Radar"),
    );
    frame.render_widget(widget, area);
}

fn resolve_pinned_overlay<'a>(
    pinned_target: Option<&'a FocusTarget>,
    cluster_id: &str,
    canvas_state: Option<&'a State>,
    cluster_state: Option<&'a cluster::State>,
) -> Option<OverlayTarget<'a>> {
    let pinned_target = pinned_target?;
    match pinned_target {
        FocusTarget::Agent {
            cluster_id: pinned_cluster_id,
            agent_id,
        } if pinned_cluster_id == cluster_id => Some(OverlayTarget {
            id: agent_id.as_str(),
            force_cluster: false,
        }),
        FocusTarget::Cluster { id } if id == cluster_id => {
            if let Some(focused_id) = canvas_state.and_then(|state| state.focused_id.as_deref()) {
                return Some(OverlayTarget {
                    id: focused_id,
                    force_cluster: true,
                });
            }
            if let Some(layout) = canvas_state.and_then(|state| state.layout.as_ref()) {
                if let Some(id) = layout.nodes.keys().next() {
                    return Some(OverlayTarget {
                        id: id.as_str(),
                        force_cluster: true,
                    });
                }
            }
            let topology = cluster_state.and_then(|state| state.topology.as_ref())?;
            if let Some(agent) = topology.agents.first() {
                return Some(OverlayTarget {
                    id: agent.id.as_str(),
                    force_cluster: true,
                });
            }
            topology.topics.first().map(|topic| OverlayTarget {
                id: topic.as_str(),
                force_cluster: true,
            })
        }
        _ => None,
    }
}

struct CanvasRenderContext<'a> {
    area: Rect,
    cluster_id: &'a str,
    cluster_state: &'a cluster::State,
    topology: &'a ClusterTopology,
    layout: &'a LayoutCache,
    focused: Option<&'a str>,
    pinned_highlight: Option<&'a str>,
    pinned_overlay: Option<OverlayTarget<'a>>,
    camera: (f64, f64),
    block: Block<'a>,
    time_cursor: &'a TimeCursor,
    focus_pulse: f64,
}

fn render_canvas(frame: &mut Frame<'_>, canvas_ctx: CanvasRenderContext<'_>) {
    let title = format!("Cluster Canvas {}", canvas_ctx.cluster_id);
    let render_bounds = camera_bounds(canvas_ctx.layout, canvas_ctx.camera);
    let block = canvas_ctx.block.title(title);
    let canvas_inner = block.inner(canvas_ctx.area);
    let layout = canvas_ctx.layout;
    let focused = canvas_ctx.focused;
    let pinned_highlight = canvas_ctx.pinned_highlight;
    let topology = canvas_ctx.topology;
    let focus_pulse = canvas_ctx.focus_pulse;
    let canvas = Canvas::default()
        .block(block)
        .x_bounds([render_bounds.min_x, render_bounds.max_x])
        .y_bounds([render_bounds.min_y, render_bounds.max_y])
        .marker(Marker::Braille)
        .paint(|ctx| {
            for edge in &layout.edges {
                let Some(from) = layout.nodes.get(&edge.from) else {
                    continue;
                };
                let Some(to) = layout.nodes.get(&edge.to) else {
                    continue;
                };
                ctx.draw(&CanvasLine {
                    x1: from.x,
                    y1: from.y,
                    x2: to.x,
                    y2: to.y,
                    color: theme::FG_DIM,
                });
            }

            for node in layout.nodes.values() {
                let color = match node.kind {
                    NodeKind::Agent => theme::agent_color(node.id.as_str()),
                    NodeKind::Topic => theme::FG_MUTED,
                };
                let orb_radius = match node.kind {
                    NodeKind::Agent => AGENT_ORB_RADIUS,
                    NodeKind::Topic => TOPIC_ORB_RADIUS,
                };

                if focused == Some(node.id.as_str()) {
                    ctx.draw(&Circle {
                        x: node.x,
                        y: node.y,
                        radius: orb_radius + 0.8 + 0.25 * focus_pulse,
                        color: theme::ACCENT,
                    });
                }

                if pinned_highlight == Some(node.id.as_str()) && focused != Some(node.id.as_str()) {
                    ctx.draw(&Circle {
                        x: node.x,
                        y: node.y,
                        radius: orb_radius + 1.2,
                        color: theme::ACCENT2,
                    });
                }

                ctx.draw(&Circle {
                    x: node.x,
                    y: node.y,
                    radius: orb_radius,
                    color,
                });

                ctx.draw(&Points {
                    coords: &[(node.x, node.y)],
                    color,
                });

                let (label_x, label_y) = label_position(node.x, node.y, node.id.as_str());
                let label_style = Style::default().fg(color);
                let label = node.label.clone();
                let line = Line::from(Span::styled(label, label_style));
                ctx.print(label_x, label_y, line);
            }

            let summary_line = topology_summary(topology);
            if let Some(summary_line) = summary_line {
                let line = Line::from(Span::styled(summary_line, theme::dim_style()));
                ctx.print(render_bounds.min_x + 1.0, render_bounds.max_y - 1.0, line);
            }
        });

    frame.render_widget(canvas, canvas_ctx.area);
    let overlay_layout = StreamOverlayLayout {
        area: canvas_inner,
        layout: canvas_ctx.layout,
        focused: canvas_ctx.focused,
        pinned_overlay: canvas_ctx.pinned_overlay,
        render_bounds: &render_bounds,
        spine_area: None,
    };
    render_stream_overlay(
        frame,
        overlay_layout,
        canvas_ctx.cluster_state,
        canvas_ctx.time_cursor,
    );
}

struct StreamOverlayLayout<'a> {
    area: Rect,
    layout: &'a LayoutCache,
    focused: Option<&'a str>,
    pinned_overlay: Option<OverlayTarget<'a>>,
    render_bounds: &'a LayoutBounds,
    spine_area: Option<Rect>,
}

fn render_stream_overlay(
    frame: &mut Frame<'_>,
    layout_ctx: StreamOverlayLayout<'_>,
    cluster_state: &cluster::State,
    time_cursor: &TimeCursor,
) {
    if layout_ctx.area.width < 6 || layout_ctx.area.height < 4 {
        return;
    }

    if let Some(focused_id) = layout_ctx.focused {
        if let Some(node) = layout_ctx.layout.nodes.get(focused_id) {
            render_overlay_for_node(
                frame,
                &layout_ctx,
                node,
                cluster_state,
                time_cursor,
                theme::focus_border_style(),
                false,
            );
        }
    }

    if let Some(pinned) = layout_ctx.pinned_overlay.as_ref() {
        if Some(pinned.id) != layout_ctx.focused || pinned.force_cluster {
            if let Some(node) = layout_ctx.layout.nodes.get(pinned.id) {
                let style = Style::default()
                    .fg(theme::ACCENT2)
                    .add_modifier(Modifier::BOLD);
                render_overlay_for_node(
                    frame,
                    &layout_ctx,
                    node,
                    cluster_state,
                    time_cursor,
                    style,
                    pinned.force_cluster,
                );
            }
        }
    }
}

fn render_overlay_for_node(
    frame: &mut Frame<'_>,
    layout_ctx: &StreamOverlayLayout<'_>,
    node: &NodeLayout,
    cluster_state: &cluster::State,
    time_cursor: &TimeCursor,
    border_style: Style,
    force_cluster: bool,
) {
    let focus_point = world_to_screen(layout_ctx.area, layout_ctx.render_bounds, node.x, node.y);
    let overlay_size = overlay_dimensions(layout_ctx.area);
    if overlay_size.0 == 0 || overlay_size.1 == 0 {
        return;
    }

    let overlay_rect = overlay_rect_near_focus(
        layout_ctx.area,
        focus_point,
        overlay_size,
        layout_ctx.spine_area,
    );
    let inner = Block::default().borders(Borders::ALL).inner(overlay_rect);
    let max_lines = inner.height as usize;
    if max_lines == 0 {
        return;
    }

    let (title, lines) =
        build_overlay_lines(cluster_state, node, time_cursor, max_lines, force_cluster);
    let overlay = StreamOverlay::new(title, lines)
        .placeholder_lines(stream::log_placeholder_lines(
            stream::LogPlaceholderContext::Overlay,
        ))
        .border_style(border_style);
    frame.render_widget(overlay, overlay_rect);
}

fn build_overlay_lines<'a>(
    cluster_state: &'a cluster::State,
    node: &NodeLayout,
    time_cursor: &TimeCursor,
    max_lines: usize,
    force_cluster: bool,
) -> (Line<'a>, Vec<Line<'a>>) {
    let is_agent = if force_cluster {
        false
    } else {
        node.kind == NodeKind::Agent
    };
    let log_title = if is_agent {
        stream::overlay_title(format!("Logs - agent {}", node.id), time_cursor)
    } else {
        stream::overlay_title("Logs - cluster", time_cursor)
    };
    let timeline_title = if is_agent {
        stream::overlay_title(format!("Timeline - agent {}", node.id), time_cursor)
    } else {
        stream::overlay_title("Timeline - cluster", time_cursor)
    };

    let log_lines = collect_log_lines(
        cluster_state,
        time_cursor,
        is_agent.then_some(node.id.as_str()),
        max_lines,
    );
    if !log_lines.is_empty() {
        return (log_title, log_lines);
    }

    let timeline_lines = collect_timeline_lines(cluster_state, time_cursor, max_lines);
    if !timeline_lines.is_empty() {
        return (timeline_title, timeline_lines);
    }

    (log_title, Vec::new())
}

fn collect_log_lines<'a>(
    cluster_state: &'a cluster::State,
    time_cursor: &TimeCursor,
    agent_id: Option<&str>,
    max_lines: usize,
) -> Vec<Line<'a>> {
    if max_lines == 0 {
        return Vec::new();
    }
    let collected =
        stream::select_time_window(&cluster_state.logs_time, time_cursor, max_lines, |line| {
            if let Some(agent_id) = agent_id {
                line.agent.as_deref() == Some(agent_id) || line.sender.as_deref() == Some(agent_id)
            } else {
                true
            }
        });
    collected
        .into_iter()
        .map(stream::format_log_line_styled)
        .collect()
}

fn collect_timeline_lines<'a>(
    cluster_state: &'a cluster::State,
    time_cursor: &TimeCursor,
    max_lines: usize,
) -> Vec<Line<'a>> {
    if max_lines == 0 {
        return Vec::new();
    }
    let collected =
        stream::select_time_window(&cluster_state.timeline_time, time_cursor, max_lines, |_| {
            true
        });
    collected
        .into_iter()
        .map(stream::format_timeline_event_styled)
        .collect()
}

fn overlay_dimensions(area: Rect) -> (u16, u16) {
    if area.width < 6 || area.height < 4 {
        return (0, 0);
    }
    let max_width = area.width.saturating_sub(2);
    let max_height = area.height.saturating_sub(2);
    if max_width == 0 || max_height == 0 {
        return (0, 0);
    }

    let mut width = ((area.width as f32) * 0.45).round() as u16;
    let mut height = ((area.height as f32) * 0.35).round() as u16;
    width = width.clamp(18, 52).min(max_width);
    height = height.clamp(5, 12).min(max_height);

    if width == 0 || height == 0 {
        return (0, 0);
    }
    (width, height)
}

fn world_to_screen(
    area: Rect,
    render_bounds: &LayoutBounds,
    world_x: f64,
    world_y: f64,
) -> (u16, u16) {
    if area.width == 0 || area.height == 0 {
        return (area.x, area.y);
    }
    let width = (render_bounds.max_x - render_bounds.min_x).max(1.0);
    let height = (render_bounds.max_y - render_bounds.min_y).max(1.0);
    let mut rel_x = (world_x - render_bounds.min_x) / width;
    let mut rel_y = (render_bounds.max_y - world_y) / height;
    rel_x = rel_x.clamp(0.0, 1.0);
    rel_y = rel_y.clamp(0.0, 1.0);
    let x = area.x as f64 + rel_x * ((area.width - 1) as f64);
    let y = area.y as f64 + rel_y * ((area.height - 1) as f64);
    (x.round() as u16, y.round() as u16)
}

fn overlay_rect_near_focus(
    bounds: Rect,
    focus: (u16, u16),
    size: (u16, u16),
    spine: Option<Rect>,
) -> Rect {
    let width = size.0.min(bounds.width);
    let height = size.1.min(bounds.height);
    let candidates = [(true, true), (true, false), (false, true), (false, false)];

    for (right, down) in candidates {
        let x = if right {
            focus.0.saturating_add(1)
        } else {
            focus.0.saturating_sub(width.saturating_add(1))
        };
        let y = if down {
            focus.1.saturating_add(1)
        } else {
            focus.1.saturating_sub(height.saturating_add(1))
        };
        let rect = clamp_rect_to_bounds(
            Rect {
                x,
                y,
                width,
                height,
            },
            bounds,
        );
        let rect = avoid_spine(rect, bounds, spine);
        if !rect_intersects_spine(rect, spine) {
            return rect;
        }
    }

    let rect = clamp_rect_to_bounds(
        Rect {
            x: bounds.x,
            y: bounds.y,
            width,
            height,
        },
        bounds,
    );
    avoid_spine(rect, bounds, spine)
}

fn clamp_rect_to_bounds(rect: Rect, bounds: Rect) -> Rect {
    let width = rect.width.min(bounds.width);
    let height = rect.height.min(bounds.height);
    if bounds.width == 0 || bounds.height == 0 || width == 0 || height == 0 {
        return Rect {
            x: bounds.x,
            y: bounds.y,
            width,
            height,
        };
    }

    let max_x = bounds.x.saturating_add(bounds.width.saturating_sub(width));
    let max_y = bounds
        .y
        .saturating_add(bounds.height.saturating_sub(height));
    let mut x = rect.x;
    let mut y = rect.y;
    if x < bounds.x {
        x = bounds.x;
    } else if x > max_x {
        x = max_x;
    }
    if y < bounds.y {
        y = bounds.y;
    } else if y > max_y {
        y = max_y;
    }

    Rect {
        x,
        y,
        width,
        height,
    }
}

fn rect_intersects_spine(rect: Rect, spine: Option<Rect>) -> bool {
    spine.is_some_and(|spine| rects_intersect(rect, spine))
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    let a_right = a.x.saturating_add(a.width);
    let a_bottom = a.y.saturating_add(a.height);
    let b_right = b.x.saturating_add(b.width);
    let b_bottom = b.y.saturating_add(b.height);
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

fn avoid_spine(rect: Rect, bounds: Rect, spine: Option<Rect>) -> Rect {
    let Some(spine) = spine else {
        return rect;
    };
    if !rects_intersect(rect, spine) {
        return rect;
    }

    let options = vec![
        Rect {
            x: rect.x,
            y: spine.y.saturating_sub(rect.height.saturating_add(1)),
            width: rect.width,
            height: rect.height,
        },
        Rect {
            x: rect.x,
            y: spine.y.saturating_add(spine.height).saturating_add(1),
            width: rect.width,
            height: rect.height,
        },
        Rect {
            x: spine.x.saturating_sub(rect.width.saturating_add(1)),
            y: rect.y,
            width: rect.width,
            height: rect.height,
        },
        Rect {
            x: spine.x.saturating_add(spine.width).saturating_add(1),
            y: rect.y,
            width: rect.width,
            height: rect.height,
        },
    ];

    for option in options {
        let candidate = clamp_rect_to_bounds(option, bounds);
        if !rects_intersect(candidate, spine) {
            return candidate;
        }
    }

    rect
}

fn topology_summary(topology: &ClusterTopology) -> Option<String> {
    if topology.agents.is_empty() && topology.topics.is_empty() && topology.edges.is_empty() {
        return None;
    }
    Some(format!(
        "{} agents, {} topics, {} edges",
        topology.agents.len(),
        topology.topics.len(),
        topology.edges.len()
    ))
}

fn camera_bounds(layout: &LayoutCache, camera: (f64, f64)) -> LayoutBounds {
    let width = layout.bounds.max_x - layout.bounds.min_x;
    let height = layout.bounds.max_y - layout.bounds.min_y;
    let half_w = width / 2.0;
    let half_h = height / 2.0;
    LayoutBounds {
        min_x: camera.0 - half_w,
        max_x: camera.0 + half_w,
        min_y: camera.1 - half_h,
        max_y: camera.1 + half_h,
    }
}

fn direction_vector(direction: Direction) -> (f64, f64) {
    match direction {
        Direction::Left => (-1.0, 0.0),
        Direction::Right => (1.0, 0.0),
        Direction::Up => (0.0, 1.0),
        Direction::Down => (0.0, -1.0),
    }
}

fn default_focus_id(topology: &ClusterTopology) -> Option<String> {
    let mut agent_ids: Vec<&String> = topology.agents.iter().map(|agent| &agent.id).collect();
    agent_ids.sort();
    if let Some(id) = agent_ids.first() {
        return Some((*id).clone());
    }
    let mut topics: Vec<&String> = topology.topics.iter().collect();
    topics.sort();
    topics.first().map(|id| (*id).clone())
}

fn default_focus_id_from_layout(layout: &LayoutCache) -> Option<String> {
    let mut agent_ids: Vec<&String> = layout
        .nodes
        .values()
        .filter(|node| node.kind == NodeKind::Agent)
        .map(|node| &node.id)
        .collect();
    agent_ids.sort();
    if let Some(id) = agent_ids.first() {
        return Some((*id).clone());
    }
    let mut topic_ids: Vec<&String> = layout
        .nodes
        .values()
        .filter(|node| node.kind == NodeKind::Topic)
        .map(|node| &node.id)
        .collect();
    topic_ids.sort();
    topic_ids.first().map(|id| (*id).clone())
}

fn layout_for(topology: &ClusterTopology) -> LayoutCache {
    let mut nodes = HashMap::new();

    for agent in &topology.agents {
        let (x, y) = ring_position(&agent.id, AGENT_RING_RADIUS);
        let label = truncate_label(&agent_label(agent));
        nodes.insert(
            agent.id.clone(),
            NodeLayout {
                id: agent.id.clone(),
                label,
                x,
                y,
                kind: NodeKind::Agent,
            },
        );
    }

    for topic in &topology.topics {
        let (x, y) = ring_position(topic, TOPIC_RING_RADIUS);
        let label = truncate_label(topic);
        nodes.insert(
            topic.clone(),
            NodeLayout {
                id: topic.clone(),
                label,
                x,
                y,
                kind: NodeKind::Topic,
            },
        );
    }

    let mut edges = Vec::new();
    for edge in &topology.edges {
        if nodes.contains_key(&edge.from) && nodes.contains_key(&edge.to) {
            edges.push(LayoutEdge {
                from: edge.from.clone(),
                to: edge.to.clone(),
            });
        }
    }

    LayoutCache {
        nodes,
        edges,
        bounds: LayoutBounds {
            min_x: -WORLD_RADIUS,
            max_x: WORLD_RADIUS,
            min_y: -WORLD_RADIUS,
            max_y: WORLD_RADIUS,
        },
    }
}

fn ring_position(id: &str, radius: f64) -> (f64, f64) {
    let angle = stable_angle(id);
    let jitter = jitter_offset(id);
    let r = (radius + jitter).max(4.0);
    (r * angle.cos(), r * angle.sin())
}

fn jitter_offset(id: &str) -> f64 {
    let hash = stable_hash(id);
    let step = (hash % 7) as f64 - 3.0;
    step * 0.35
}

fn agent_label(agent: &TopologyAgent) -> String {
    match agent.role.as_ref() {
        Some(role) if !role.trim().is_empty() => format!("{} ({})", agent.id, role),
        _ => agent.id.clone(),
    }
}

fn label_position(x: f64, y: f64, id: &str) -> (f64, f64) {
    let hash = stable_hash(id);
    let side = if hash & 1 == 0 { 1.0 } else { -1.0 };
    let angle = if x == 0.0 && y == 0.0 {
        0.0
    } else {
        y.atan2(x)
    };
    let tangent_x = -angle.sin();
    let tangent_y = angle.cos();
    let radial_x = angle.cos();
    let radial_y = angle.sin();
    let mut lx = x + tangent_x * LABEL_OFFSET * side + radial_x * LABEL_RADIAL_OFFSET;
    let mut ly = y + tangent_y * LABEL_OFFSET * side + radial_y * LABEL_RADIAL_OFFSET;
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

fn truncate_label(label: &str) -> String {
    let mut iter = label.chars();
    let mut out = String::new();
    for _ in 0..LABEL_LIMIT {
        match iter.next() {
            Some(ch) => out.push(ch),
            None => return label.to_string(),
        }
    }
    if iter.next().is_some() {
        out.push_str("..");
    }
    out
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
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    use crate::app::animation::AnimClock;
    use crate::protocol::{ClusterLogLine, TopologyAgent, TopologyEdge, TopologyEdgeKind};
    use crate::ui::widgets::test_utils::line_text;
    use std::collections::HashMap;

    fn sample_topology() -> ClusterTopology {
        ClusterTopology {
            agents: vec![
                TopologyAgent {
                    id: "agent-alpha".to_string(),
                    role: Some("planner".to_string()),
                },
                TopologyAgent {
                    id: "agent-bravo".to_string(),
                    role: Some("worker".to_string()),
                },
                TopologyAgent {
                    id: "agent-charlie".to_string(),
                    role: None,
                },
            ],
            topics: vec!["ISSUE_OPENED".to_string()],
            edges: vec![TopologyEdge {
                from: "agent-alpha".to_string(),
                to: "agent-bravo".to_string(),
                topic: "ISSUE_OPENED".to_string(),
                kind: TopologyEdgeKind::Publish,
                dynamic: None,
            }],
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

    fn buffers_differ(first: &Buffer, second: &Buffer) -> bool {
        if first.area != second.area {
            return true;
        }
        for y in first.area.top()..first.area.bottom() {
            for x in first.area.left()..first.area.right() {
                let first_cell = first.cell((x, y));
                let second_cell = second.cell((x, y));
                if first_cell != second_cell {
                    return true;
                }
            }
        }
        false
    }

    fn rect_within_bounds(rect: Rect, bounds: Rect) -> bool {
        let rect_right = rect.x.saturating_add(rect.width);
        let rect_bottom = rect.y.saturating_add(rect.height);
        let bounds_right = bounds.x.saturating_add(bounds.width);
        let bounds_bottom = bounds.y.saturating_add(bounds.height);
        rect.x >= bounds.x
            && rect.y >= bounds.y
            && rect_right <= bounds_right
            && rect_bottom <= bounds_bottom
    }

    fn layout_with_nodes(nodes: Vec<NodeLayout>) -> LayoutCache {
        let mut map = HashMap::new();
        for node in nodes {
            map.insert(node.id.clone(), node);
        }
        LayoutCache {
            nodes: map,
            edges: Vec::new(),
            bounds: LayoutBounds {
                min_x: -WORLD_RADIUS,
                max_x: WORLD_RADIUS,
                min_y: -WORLD_RADIUS,
                max_y: WORLD_RADIUS,
            },
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let topology = sample_topology();
        let first = layout_for(&topology);
        let second = layout_for(&topology);
        assert_eq!(first, second);
    }

    #[test]
    fn render_pending_topology() {
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let cluster_state = cluster::State::default();
        let canvas_state = State::default();
        let anim_clock = AnimClock::default();
        let time_cursor = TimeCursor::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-1",
                        cluster_state: Some(&cluster_state),
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, PENDING_MESSAGE));
    }

    #[test]
    fn render_missing_cluster_as_unavailable() {
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let canvas_state = State::default();
        let anim_clock = AnimClock::default();
        let time_cursor = TimeCursor::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-missing",
                        cluster_state: None,
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, UNAVAILABLE_MESSAGE));
    }

    #[test]
    fn render_topology_unavailable_on_error() {
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut cluster_state = cluster::State::default();
        cluster_state.topology_error = Some("backend timeout".to_string());
        let canvas_state = State::default();
        let anim_clock = AnimClock::default();
        let time_cursor = TimeCursor::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-2",
                        cluster_state: Some(&cluster_state),
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, UNAVAILABLE_MESSAGE));
        assert!(buffer_contains(buffer, "backend timeout"));
    }

    #[test]
    fn render_basic_topology() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let topology = sample_topology();
        let mut cluster_state = cluster::State::default();
        cluster_state.topology = Some(topology.clone());
        let mut canvas_state = State::default();
        canvas_state.update_layout(&topology);
        let anim_clock = AnimClock::default();
        let time_cursor = TimeCursor::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-3",
                        cluster_state: Some(&cluster_state),
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, "agent-alpha"));
        assert!(buffer_contains(buffer, "ISSUE_OPENED"));
    }

    #[test]
    fn default_focus_prefers_agents() {
        let topology = ClusterTopology {
            agents: vec![
                TopologyAgent {
                    id: "worker".to_string(),
                    role: None,
                },
                TopologyAgent {
                    id: "planner".to_string(),
                    role: None,
                },
            ],
            topics: vec!["topic-b".to_string(), "topic-a".to_string()],
            edges: Vec::new(),
        };
        assert_eq!(default_focus_id(&topology), Some("planner".to_string()));

        let topology = ClusterTopology {
            agents: Vec::new(),
            topics: vec!["topic-b".to_string(), "topic-a".to_string()],
            edges: Vec::new(),
        };
        assert_eq!(default_focus_id(&topology), Some("topic-a".to_string()));
    }

    #[test]
    fn move_focus_direction() {
        let layout = layout_with_nodes(vec![
            NodeLayout {
                id: "center".to_string(),
                label: "center".to_string(),
                x: 0.0,
                y: 0.0,
                kind: NodeKind::Agent,
            },
            NodeLayout {
                id: "right".to_string(),
                label: "right".to_string(),
                x: 10.0,
                y: 0.0,
                kind: NodeKind::Agent,
            },
            NodeLayout {
                id: "right-far".to_string(),
                label: "right-far".to_string(),
                x: 18.0,
                y: 2.0,
                kind: NodeKind::Agent,
            },
            NodeLayout {
                id: "left".to_string(),
                label: "left".to_string(),
                x: -10.0,
                y: 0.0,
                kind: NodeKind::Agent,
            },
            NodeLayout {
                id: "up".to_string(),
                label: "up".to_string(),
                x: 0.0,
                y: 10.0,
                kind: NodeKind::Agent,
            },
            NodeLayout {
                id: "down".to_string(),
                label: "down".to_string(),
                x: 0.0,
                y: -10.0,
                kind: NodeKind::Agent,
            },
        ]);

        let mut state = State {
            focused_id: Some("center".to_string()),
            layout: Some(layout),
            ..State::default()
        };
        state.move_focus(Direction::Right, MoveSpeed::Step);
        assert_eq!(state.focused_id.as_deref(), Some("right"));
    }

    #[test]
    fn move_focus_fast() {
        let layout = layout_with_nodes(vec![
            NodeLayout {
                id: "a".to_string(),
                label: "a".to_string(),
                x: 0.0,
                y: 0.0,
                kind: NodeKind::Agent,
            },
            NodeLayout {
                id: "b".to_string(),
                label: "b".to_string(),
                x: 10.0,
                y: 0.0,
                kind: NodeKind::Agent,
            },
            NodeLayout {
                id: "c".to_string(),
                label: "c".to_string(),
                x: 20.0,
                y: 0.0,
                kind: NodeKind::Agent,
            },
        ]);

        let mut state = State {
            focused_id: Some("a".to_string()),
            layout: Some(layout),
            ..State::default()
        };
        state.move_focus(Direction::Right, MoveSpeed::Fast);
        assert_eq!(state.focused_id.as_deref(), Some("c"));
    }

    #[test]
    fn render_focus_ring_changes_output() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let topology = sample_topology();
        let layout = layout_for(&topology);
        let mut cluster_state = cluster::State::default();
        cluster_state.topology = Some(topology.clone());
        let anim_clock = AnimClock::default();
        let time_cursor = TimeCursor::default();

        let canvas_state = State {
            focused_id: Some("agent-alpha".to_string()),
            layout: Some(layout.clone()),
            ..State::default()
        };
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-4",
                        cluster_state: Some(&cluster_state),
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");
        let first = terminal.backend().buffer().clone();

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let canvas_state = State {
            focused_id: Some("agent-bravo".to_string()),
            layout: Some(layout),
            ..State::default()
        };
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-4",
                        cluster_state: Some(&cluster_state),
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");
        let second = terminal.backend().buffer().clone();

        assert!(buffers_differ(&first, &second));
    }

    #[test]
    fn overlay_rect_clamps_to_bounds() {
        let bounds = Rect {
            x: 2,
            y: 1,
            width: 40,
            height: 18,
        };
        let focus = (41, 18);
        let size = (26, 10);
        let rect = overlay_rect_near_focus(bounds, focus, size, None);
        assert!(rect_within_bounds(rect, bounds));
    }

    #[test]
    fn overlay_rect_avoids_spine() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 20,
        };
        let spine = Rect {
            x: 22,
            y: 12,
            width: 12,
            height: 4,
        };
        let focus = (26, 13);
        let rect = overlay_rect_near_focus(bounds, focus, (20, 8), Some(spine));
        assert!(!rects_intersect(rect, spine));
    }

    #[test]
    fn cluster_canvas_renders_log_overlay() {
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let topology = sample_topology();
        let mut cluster_state = cluster::State::default();
        cluster_state.topology = Some(topology.clone());
        cluster_state.push_log_lines(
            vec![ClusterLogLine {
                id: "line-1".to_string(),
                timestamp: 0,
                text: "build complete".to_string(),
                agent: Some("agent-alpha".to_string()),
                role: None,
                sender: None,
            }],
            None,
        );

        let mut canvas_state = State::default();
        canvas_state.update_layout(&topology);
        canvas_state.focused_id = Some("agent-alpha".to_string());
        let anim_clock = AnimClock::default();
        let time_cursor = TimeCursor::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-5",
                        cluster_state: Some(&cluster_state),
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, "Logs - agent agent-alpha"));
        assert!(buffer_contains(buffer, "build complete"));
    }

    #[test]
    fn cluster_canvas_overlay_respects_time_window() {
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let topology = sample_topology();
        let mut cluster_state = cluster::State::default();
        cluster_state.topology = Some(topology.clone());
        cluster_state.push_log_lines(
            vec![
                ClusterLogLine {
                    id: "line-old".to_string(),
                    timestamp: 100,
                    text: "old".to_string(),
                    agent: Some("agent-alpha".to_string()),
                    role: None,
                    sender: None,
                },
                ClusterLogLine {
                    id: "line-mid".to_string(),
                    timestamp: 200,
                    text: "mid".to_string(),
                    agent: Some("agent-alpha".to_string()),
                    role: None,
                    sender: None,
                },
                ClusterLogLine {
                    id: "line-new".to_string(),
                    timestamp: 300,
                    text: "new".to_string(),
                    agent: Some("agent-alpha".to_string()),
                    role: None,
                    sender: None,
                },
            ],
            None,
        );

        let mut canvas_state = State::default();
        canvas_state.update_layout(&topology);
        canvas_state.focused_id = Some("agent-alpha".to_string());

        let time_cursor = TimeCursor {
            mode: crate::app::TimeCursorMode::Scrub,
            t_ms: 250,
            window_ms: 120,
        };
        let anim_clock = AnimClock::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-5",
                        cluster_state: Some(&cluster_state),
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, "mid"));
        assert!(!buffer_contains(buffer, "old"));
        assert!(!buffer_contains(buffer, "new"));
    }

    #[test]
    fn cluster_canvas_renders_overlay_placeholder() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let topology = sample_topology();
        let mut cluster_state = cluster::State::default();
        cluster_state.topology = Some(topology.clone());

        let mut canvas_state = State::default();
        canvas_state.update_layout(&topology);
        canvas_state.focused_id = Some("agent-alpha".to_string());
        let anim_clock = AnimClock::default();
        let time_cursor = TimeCursor::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    RenderContext {
                        cluster_id: "cluster-6",
                        cluster_state: Some(&cluster_state),
                        canvas_state: Some(&canvas_state),
                        time_cursor: &time_cursor,
                        anim_clock: &anim_clock,
                        pinned_target: None,
                    },
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, "No logs yet."));
    }

    #[test]
    fn truncate_label_handles_unicode() {
        let label = "αβγδεζηθικλμνξοπρσ";
        let prefix: String = label.chars().take(LABEL_LIMIT).collect();
        let truncated = truncate_label(label);
        assert_eq!(truncated, format!("{}..", prefix));
    }
}
