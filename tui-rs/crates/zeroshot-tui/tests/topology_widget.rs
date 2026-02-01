use zeroshot_tui::protocol::{ClusterTopology, TopologyAgent, TopologyEdge, TopologyEdgeKind};
use zeroshot_tui::ui::widgets::topology;

fn sample_topology() -> ClusterTopology {
    ClusterTopology {
        agents: vec![
            TopologyAgent {
                id: "worker".to_string(),
                role: Some("implementation".to_string()),
            },
            TopologyAgent {
                id: "system".to_string(),
                role: None,
            },
            TopologyAgent {
                id: "validator".to_string(),
                role: Some("validator".to_string()),
            },
        ],
        edges: vec![
            TopologyEdge {
                from: "worker".to_string(),
                to: "IMPLEMENTATION_READY".to_string(),
                topic: "IMPLEMENTATION_READY".to_string(),
                kind: TopologyEdgeKind::Publish,
                dynamic: None,
            },
            TopologyEdge {
                from: "ISSUE_OPENED".to_string(),
                to: "worker".to_string(),
                topic: "ISSUE_OPENED".to_string(),
                kind: TopologyEdgeKind::Trigger,
                dynamic: Some(true),
            },
            TopologyEdge {
                from: "system".to_string(),
                to: "ISSUE_OPENED".to_string(),
                topic: "ISSUE_OPENED".to_string(),
                kind: TopologyEdgeKind::Source,
                dynamic: None,
            },
        ],
        topics: vec![
            "IMPLEMENTATION_READY".to_string(),
            "ISSUE_OPENED".to_string(),
        ],
    }
}

#[test]
fn renders_sorted_edges() {
    let topology = sample_topology();
    let lines = topology::build_lines(None, Some(&topology), None);

    assert_eq!(
        lines,
        vec![
            "Summary pending.",
            "Agents: 3 | Topics: 2 | Edges: 3",
            "ISSUE_OPENED:",
            "  -> worker (trigger:ISSUE_OPENED dynamic)",
            "system:",
            "  -> ISSUE_OPENED (source:ISSUE_OPENED)",
            "worker:",
            "  -> IMPLEMENTATION_READY (publish:IMPLEMENTATION_READY)",
            "Tab/Shift+Tab or h/l (Left/Right) to switch panes",
        ]
    );
}

#[test]
fn renders_placeholder_on_error() {
    let lines = topology::build_lines(None, None, Some("backend unavailable"));
    assert_eq!(
        lines,
        vec![
            "Summary pending.",
            "Topology unavailable: backend unavailable",
            "Tab/Shift+Tab or h/l (Left/Right) to switch panes",
        ]
    );
}
