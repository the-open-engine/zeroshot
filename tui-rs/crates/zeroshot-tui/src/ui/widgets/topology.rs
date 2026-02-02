use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::protocol::{
    ClusterSummary, ClusterTopology, TopologyAgent, TopologyEdge, TopologyEdgeKind,
};

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    block: Block<'_>,
    summary: Option<&ClusterSummary>,
    topology: Option<&ClusterTopology>,
    error: Option<&str>,
) {
    let lines = build_lines(summary, topology, error)
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    let widget = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

pub fn build_lines(
    summary: Option<&ClusterSummary>,
    topology: Option<&ClusterTopology>,
    error: Option<&str>,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(summary_line(summary));

    if let Some(message) = error {
        append_error(&mut lines, message);
        return lines;
    }

    let Some(topology) = topology else {
        append_pending(&mut lines);
        return lines;
    };

    let (agents, topics, edges) = sorted_topology(topology);
    append_counts(&mut lines, &agents, &topics, &edges);

    if edges.is_empty() {
        append_no_edges(&mut lines, &agents, &topics);
        return lines;
    }

    append_edges(&mut lines, edges);
    append_focus_hint(&mut lines);
    lines
}

fn append_error(lines: &mut Vec<String>, message: &str) {
    lines.push(format!("Topology unavailable: {message}"));
    append_focus_hint(lines);
}

fn append_pending(lines: &mut Vec<String>) {
    lines.push("Topology pending. Waiting for backend.".to_string());
    append_focus_hint(lines);
}

fn append_counts(
    lines: &mut Vec<String>,
    agents: &[TopologyAgent],
    topics: &[String],
    edges: &[TopologyEdge],
) {
    lines.push(format!(
        "Agents: {} | Topics: {} | Edges: {}",
        agents.len(),
        topics.len(),
        edges.len()
    ));
}

fn append_no_edges(lines: &mut Vec<String>, agents: &[TopologyAgent], topics: &[String]) {
    if !agents.is_empty() {
        let list = agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("Agents: {list}"));
    }
    if !topics.is_empty() {
        lines.push(format!("Topics: {}", topics.join(", ")));
    }
    lines.push("No edges yet.".to_string());
    append_focus_hint(lines);
}

fn append_edges(lines: &mut Vec<String>, edges: Vec<TopologyEdge>) {
    let mut current_from: Option<String> = None;
    for edge in edges {
        if current_from.as_deref() != Some(edge.from.as_str()) {
            current_from = Some(edge.from.clone());
            lines.push(format!("{}:", edge.from));
        }
        lines.push(format!("  -> {}", edge_details(&edge)));
    }
}

fn append_focus_hint(lines: &mut Vec<String>) {
    lines.push("Tab/Shift+Tab or h/l (Left/Right) to switch panes".to_string());
}

fn sorted_topology(
    topology: &ClusterTopology,
) -> (Vec<TopologyAgent>, Vec<String>, Vec<TopologyEdge>) {
    let mut agents = topology.agents.clone();
    agents.sort_by(|a, b| a.id.cmp(&b.id));

    let mut topics = topology.topics.clone();
    topics.sort();

    let mut edges = topology.edges.clone();
    edges.sort_by(|a, b| {
        let kind_a = kind_label(&a.kind);
        let kind_b = kind_label(&b.kind);
        (a.from.as_str(), a.to.as_str(), kind_a, a.topic.as_str()).cmp(&(
            b.from.as_str(),
            b.to.as_str(),
            kind_b,
            b.topic.as_str(),
        ))
    });

    (agents, topics, edges)
}

fn summary_line(summary: Option<&ClusterSummary>) -> String {
    summary
        .map(|summary| {
            let provider = summary.provider.as_deref().unwrap_or("default");
            format!("State: {} | Provider: {}", summary.state, provider)
        })
        .unwrap_or_else(|| "Summary pending.".to_string())
}

fn edge_details(edge: &TopologyEdge) -> String {
    let mut suffix = format!("{}:{}", kind_label(&edge.kind), edge.topic);
    if edge.dynamic.unwrap_or(false) {
        suffix.push_str(" dynamic");
    }
    format!("{} ({suffix})", edge.to)
}

fn kind_label(kind: &TopologyEdgeKind) -> &'static str {
    match kind {
        TopologyEdgeKind::Trigger => "trigger",
        TopologyEdgeKind::Publish => "publish",
        TopologyEdgeKind::Source => "source",
    }
}
