mod ui_snapshot_helpers;

use ui_snapshot_helpers::render_to_text;

use zeroshot_tui::app::{AppState, ScreenId, UiVariant};
use zeroshot_tui::protocol::ClusterSummary;
use zeroshot_tui::ui;

fn cluster_summary(id: &str, state: &str) -> ClusterSummary {
    ClusterSummary {
        id: id.to_string(),
        state: state.to_string(),
        provider: None,
        created_at: 0,
        agent_count: 0,
        message_count: 0,
        cwd: None,
    }
}

#[test]
fn radar_snapshot_empty_state() {
    let mut state = AppState::default();
    state.ui_variant = UiVariant::Disruptive;
    state.screen_stack = vec![ScreenId::FleetRadar];

    let content = render_to_text(60, 12, |frame| ui::render(frame, &state));
    assert!(content.contains("Fleet Radar"));
    assert!(content.contains("No clusters yet."));
    assert!(content.contains("Type an intent in the spine"));
}

#[test]
fn radar_snapshot_three_cluster_states() {
    let mut state = AppState::default();
    state.ui_variant = UiVariant::Disruptive;
    state.screen_stack = vec![ScreenId::FleetRadar];
    state.now_ms = 10_000;
    state.fleet_radar.set_clusters(
        vec![
            cluster_summary("run-1", "running"),
            cluster_summary("done-1", "done"),
            cluster_summary("err-1", "error"),
        ],
        state.now_ms,
    );

    let content = render_to_text(80, 16, |frame| ui::render(frame, &state));
    assert!(content.contains("Fleet Radar"));
    assert!(content.contains("run-1"));
    assert!(content.contains("done-1"));
    assert!(content.contains("err-1"));
    assert!(!content.contains("No clusters yet."));
}
