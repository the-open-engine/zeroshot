use std::collections::HashMap;

use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Points};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::protocol::{ClusterTopology, TopologyAgent};
use crate::screens::cluster;
use crate::ui::theme;

const WORLD_RADIUS: f64 = 48.0;
const AGENT_RING_RADIUS: f64 = 28.0;
const TOPIC_RING_RADIUS: f64 = 14.0;
const AGENT_ORB_RADIUS: f64 = 1.8;
const TOPIC_ORB_RADIUS: f64 = 1.2;
const LABEL_OFFSET: f64 = 4.0;
const LABEL_RADIAL_OFFSET: f64 = 1.4;
const LABEL_LIMIT: usize = 14;
const PENDING_MESSAGE: &str = "Waiting for cluster topology";
const FOCUS_EPSILON: f64 = 0.0001;

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
    MoveFocus { direction: Direction, speed: MoveSpeed },
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
}

impl State {
    pub fn update_layout(&mut self, topology: &ClusterTopology) {
        self.layout = Some(layout_for(topology));
        self.ensure_focus(topology);
        self.center_camera_on_focus();
    }

    pub fn ensure_focus(&mut self, topology: &ClusterTopology) {
        let mut needs_focus = self.focused_id.is_none();
        if !needs_focus {
            let focused = self.focused_id.as_ref().expect("focused id");
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
        if self.focused_id.is_none() {
            if let Some(layout) = self.layout.as_ref() {
                self.focused_id = default_focus_id_from_layout(layout);
            }
        } else if let Some(layout) = self.layout.as_ref() {
            let focused = self.focused_id.as_ref().expect("focused id");
            if !layout.nodes.contains_key(focused) {
                self.focused_id = default_focus_id_from_layout(layout);
            }
        }
        if self.focused_id.is_none() {
            return;
        }

        self.center_camera_on_focus();

        let mut steps = match speed {
            MoveSpeed::Step => 1,
            MoveSpeed::Fast => 2,
        };
        while steps > 0 {
            let Some(next) = self.next_focus_id(direction) else {
                break;
            };
            self.focused_id = Some(next);
            self.center_camera_on_focus();
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

    fn center_camera_on_focus(&mut self) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let Some(focused_id) = self.focused_id.as_ref() else {
            return;
        };
        let Some(node) = layout.nodes.get(focused_id) else {
            return;
        };
        self.camera = (node.x, node.y);
    }
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    cluster_id: &str,
    cluster_state: Option<&cluster::State>,
    canvas_state: Option<&State>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Cluster Canvas");

    let Some(cluster_state) = cluster_state else {
        render_placeholder(frame, area, block, PENDING_MESSAGE, None);
        return;
    };

    if let Some(error) = cluster_state.topology_error.as_deref() {
        render_placeholder(
            frame,
            area,
            block,
            "Topology error",
            Some(error),
        );
        return;
    }

    let Some(topology) = cluster_state.topology.as_ref() else {
        render_placeholder(frame, area, block, PENDING_MESSAGE, None);
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
    let camera = canvas_state
        .map(|state| state.camera)
        .unwrap_or((0.0, 0.0));

    render_canvas(
        frame, area, cluster_id, topology, layout, focused, camera, block,
    );
}

fn render_placeholder(
    frame: &mut Frame<'_>,
    area: Rect,
    block: Block,
    headline: &str,
    detail: Option<&str>,
) {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(headline, theme::muted_style())));
    if let Some(detail) = detail {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(detail, theme::dim_style())));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press Esc to return to Fleet Radar",
        theme::dim_style(),
    )));

    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(widget, area);
}

fn render_canvas(
    frame: &mut Frame<'_>,
    area: Rect,
    cluster_id: &str,
    topology: &ClusterTopology,
    layout: &LayoutCache,
    focused: Option<&str>,
    camera: (f64, f64),
    block: Block,
) {
    let title = format!("Cluster Canvas {cluster_id}");
    let render_bounds = camera_bounds(layout, camera);
    let canvas = Canvas::default()
        .block(block.title(title))
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
                        radius: orb_radius + 0.8,
                        color: theme::ACCENT,
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
                ctx.print(
                    render_bounds.min_x + 1.0,
                    render_bounds.max_y - 1.0,
                    line,
                );
            }
        });

    frame.render_widget(canvas, area);
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
    let angle = if x == 0.0 && y == 0.0 { 0.0 } else { y.atan2(x) };
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
    use ratatui::buffer::Buffer;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::protocol::{TopologyAgent, TopologyEdge, TopologyEdgeKind};
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

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    "cluster-1",
                    Some(&cluster_state),
                    Some(&canvas_state),
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, PENDING_MESSAGE));
    }

    #[test]
    fn render_topology_error() {
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut cluster_state = cluster::State::default();
        cluster_state.topology_error = Some("backend timeout".to_string());
        let canvas_state = State::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    "cluster-2",
                    Some(&cluster_state),
                    Some(&canvas_state),
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, "Topology error"));
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

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(
                    frame,
                    area,
                    "cluster-3",
                    Some(&cluster_state),
                    Some(&canvas_state),
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
                    "cluster-4",
                    Some(&cluster_state),
                    Some(&canvas_state),
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
                    "cluster-4",
                    Some(&cluster_state),
                    Some(&canvas_state),
                );
            })
            .expect("draw");
        let second = terminal.backend().buffer().clone();

        assert!(buffers_differ(&first, &second));
    }

    #[test]
    fn truncate_label_handles_unicode() {
        let label = "αβγδεζηθικλμνξοπρσ";
        let prefix: String = label.chars().take(LABEL_LIMIT).collect();
        let truncated = truncate_label(label);
        assert_eq!(truncated, format!("{}..", prefix));
    }
}
