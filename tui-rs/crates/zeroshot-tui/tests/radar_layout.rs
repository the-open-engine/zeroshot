use zeroshot_tui::protocol::ClusterSummary;
use zeroshot_tui::screens::radar::{layout_position, FleetRadarState};

fn cluster_summary(id: &str, message_count: i64) -> ClusterSummary {
    ClusterSummary {
        id: id.to_string(),
        state: "running".to_string(),
        provider: None,
        created_at: 0,
        agent_count: 0,
        message_count,
        cwd: None,
    }
}

#[test]
fn radar_layout_is_deterministic() {
    let first = layout_position("cluster-1", 10_000);
    let second = layout_position("cluster-1", 10_000);
    assert_eq!(first, second);
}

#[test]
fn radar_selects_first_cluster() {
    let mut state = FleetRadarState::default();
    state.set_clusters(
        vec![
            cluster_summary("c2", 0),
            cluster_summary("c1", 0),
            cluster_summary("c3", 0),
        ],
        1000,
    );
    assert_eq!(state.selected_cluster_id(), Some("c2".to_string()));

    state.selected = 2;
    state.set_clusters(vec![cluster_summary("c1", 0)], 2000);
    assert_eq!(state.selected_cluster_id(), Some("c1".to_string()));
}
