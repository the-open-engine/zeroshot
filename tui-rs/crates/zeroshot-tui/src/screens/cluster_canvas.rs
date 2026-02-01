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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Agent,
    Topic,
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
}

impl State {
    pub fn update_layout(&mut self, topology: &ClusterTopology) {
        self.layout = Some(layout_for(topology));
    }

    pub fn clear_layout(&mut self) {
        self.layout = None;
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

    render_canvas(frame, area, cluster_id, topology, layout, focused, block);
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
    block: Block,
) {
    let title = format!("Cluster Canvas {cluster_id}");
    let canvas = Canvas::default()
        .block(block.title(title))
        .x_bounds([layout.bounds.min_x, layout.bounds.max_x])
        .y_bounds([layout.bounds.min_y, layout.bounds.max_y])
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
                    layout.bounds.min_x + 1.0,
                    layout.bounds.max_y - 1.0,
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
    fn truncate_label_handles_unicode() {
        let label = "αβγδεζηθικλμνξοπρσ";
        let prefix: String = label.chars().take(LABEL_LIMIT).collect();
        let truncated = truncate_label(label);
        assert_eq!(truncated, format!("{}..", prefix));
    }
}
