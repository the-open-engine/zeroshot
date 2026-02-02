use zeroshot_tui::app::{self, Action, AppState, BackendRequest, Effect, ScreenAction, ScreenId};
use zeroshot_tui::protocol::ClusterSummary;
use zeroshot_tui::screens::monitor;

fn cluster_summary(id: &str, message_count: i64) -> ClusterSummary {
    ClusterSummary {
        id: id.to_string(),
        state: "running".to_string(),
        provider: Some("codex".to_string()),
        created_at: 0,
        agent_count: 0,
        message_count,
        cwd: None,
    }
}

#[test]
fn monitor_selection_clamps_on_shrink() {
    let mut state = monitor::State::default();
    state.set_clusters(
        vec![
            cluster_summary("c1", 0),
            cluster_summary("c2", 0),
            cluster_summary("c3", 0),
        ],
        1000,
    );
    state.selected = 2;

    state.set_clusters(vec![cluster_summary("c1", 0)], 2000);

    assert_eq!(state.selected, 0);
}

#[test]
fn monitor_tick_triggers_polling_interval() {
    let mut state = AppState::default();
    state.screen_stack = vec![ScreenId::Monitor];
    state.monitor.last_poll_at = Some(1000);

    let (state, effects) = app::update(state, Action::Tick { now_ms: 1500 });
    assert!(effects.is_empty());
    assert_eq!(state.monitor.last_poll_at, Some(1000));

    let (_state, effects) = app::update(state, Action::Tick { now_ms: 2001 });
    assert_eq!(effects, vec![Effect::Backend(BackendRequest::ListClusters)]);
}

#[test]
fn monitor_open_selected_pushes_cluster() {
    let mut state = AppState::default();
    state
        .monitor
        .set_clusters(vec![cluster_summary("c1", 0)], 1000);

    let (_state, effects) = app::update(
        state,
        Action::Screen(ScreenAction::Monitor(monitor::Action::OpenSelected)),
    );

    assert_eq!(
        effects,
        vec![
            Effect::Backend(BackendRequest::GetClusterSummary {
                cluster_id: "c1".to_string()
            }),
            Effect::Backend(BackendRequest::GetClusterTopology {
                cluster_id: "c1".to_string()
            }),
            Effect::Backend(BackendRequest::SubscribeClusterLogs {
                cluster_id: "c1".to_string(),
                agent_id: None,
            }),
            Effect::Backend(BackendRequest::SubscribeClusterTimeline {
                cluster_id: "c1".to_string()
            }),
        ]
    );
}

#[test]
fn monitor_last_activity_updates_only_on_increase() {
    let mut state = monitor::State::default();
    state.set_clusters(
        vec![cluster_summary("c1", 1), cluster_summary("c2", 2)],
        1000,
    );

    assert_eq!(state.last_activity_at.get("c1"), Some(&1000));
    assert_eq!(state.last_activity_at.get("c2"), Some(&1000));

    state.set_clusters(
        vec![cluster_summary("c1", 1), cluster_summary("c2", 2)],
        2000,
    );

    assert_eq!(state.last_activity_at.get("c1"), Some(&1000));
    assert_eq!(state.last_activity_at.get("c2"), Some(&1000));

    state.set_clusters(
        vec![cluster_summary("c1", 3), cluster_summary("c2", 2)],
        3000,
    );

    assert_eq!(state.last_activity_at.get("c1"), Some(&3000));
    assert_eq!(state.last_activity_at.get("c2"), Some(&1000));
}
