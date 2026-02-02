mod ui_snapshot_helpers;

use ui_snapshot_helpers::render_to_text;

use zeroshot_tui::app::{AppState, ScreenId, TimeCursor, TimeCursorMode, UiVariant};
use zeroshot_tui::protocol::{
    ClusterLogLine, ClusterTopology, TimelineEvent, TopologyAgent, TopologyEdge, TopologyEdgeKind,
};
use zeroshot_tui::screens::{cluster, cluster_canvas};
use zeroshot_tui::ui;

fn sample_topology() -> ClusterTopology {
    ClusterTopology {
        agents: vec![
            TopologyAgent {
                id: "worker".to_string(),
                role: Some("implementation".to_string()),
            },
            TopologyAgent {
                id: "validator".to_string(),
                role: Some("validator".to_string()),
            },
        ],
        edges: vec![
            TopologyEdge {
                from: "ISSUE_OPENED".to_string(),
                to: "worker".to_string(),
                topic: "ISSUE_OPENED".to_string(),
                kind: TopologyEdgeKind::Trigger,
                dynamic: Some(true),
            },
            TopologyEdge {
                from: "worker".to_string(),
                to: "IMPLEMENTATION_READY".to_string(),
                topic: "IMPLEMENTATION_READY".to_string(),
                kind: TopologyEdgeKind::Publish,
                dynamic: None,
            },
        ],
        topics: vec![
            "ISSUE_OPENED".to_string(),
            "IMPLEMENTATION_READY".to_string(),
        ],
    }
}

fn sample_cluster_state() -> cluster::State {
    let mut state = cluster::State::default();
    let topology = sample_topology();
    state.topology = Some(topology);

    state.logs_time.push_many(vec![
        ClusterLogLine {
            id: "log-1".to_string(),
            timestamp: 900,
            text: "agent started".to_string(),
            agent: Some("worker".to_string()),
            role: Some("implementation".to_string()),
            sender: Some("worker".to_string()),
        },
        ClusterLogLine {
            id: "log-2".to_string(),
            timestamp: 950,
            text: "cluster event".to_string(),
            agent: None,
            role: None,
            sender: Some("system".to_string()),
        },
    ]);

    state.timeline_time.push_many(vec![TimelineEvent {
        id: "evt-1".to_string(),
        timestamp: 920,
        topic: "ISSUE_OPENED".to_string(),
        label: "opened".to_string(),
        approved: None,
        sender: Some("system".to_string()),
    }]);

    state
}

#[test]
fn cluster_canvas_snapshot_with_focus_and_overlay() {
    let cluster_id = "cluster-1".to_string();
    let mut state = AppState::default();
    state.ui_variant = UiVariant::Disruptive;
    state.screen_stack = vec![ScreenId::ClusterCanvas {
        id: cluster_id.clone(),
    }];
    state.time_cursor = TimeCursor {
        mode: TimeCursorMode::Live,
        t_ms: 1000,
        window_ms: 1000,
    };

    let cluster_state = sample_cluster_state();
    let topology = cluster_state.topology.clone().expect("topology");

    let mut canvas_state = cluster_canvas::State::default();
    canvas_state.focused_id = Some("worker".to_string());
    canvas_state.update_layout(&topology);

    state.clusters.insert(cluster_id.clone(), cluster_state);
    state
        .cluster_canvases
        .insert(cluster_id.clone(), canvas_state);

    let content = render_to_text(100, 26, |frame| ui::render(frame, &state));
    assert!(content.contains("Cluster Canvas cluster-1"));
    assert!(content.contains("worker"));
    assert!(content.contains("Logs - agent worker"));
    assert!(content.contains("[LIVE]"));
    assert!(content.contains("agent started"));
}
