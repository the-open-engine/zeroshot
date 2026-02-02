use zeroshot_tui::app::{
    self, Action, AppState, BackendRequest, Effect, NavigationAction, ScreenAction, ScreenId,
};
use zeroshot_tui::screens::cluster;

#[test]
fn esc_pops_until_launcher_root() {
    let state = AppState::default();
    let (state, _) = app::update(
        state,
        Action::Navigate(NavigationAction::Push(ScreenId::Monitor)),
    );
    let (state, _) = app::update(
        state,
        Action::Navigate(NavigationAction::Push(ScreenId::Cluster {
            id: "cluster-1".to_string(),
        })),
    );

    let (state, _) = app::update(state, Action::Navigate(NavigationAction::Pop));
    assert!(matches!(state.active_screen(), ScreenId::Monitor));

    let (state, _) = app::update(state, Action::Navigate(NavigationAction::Pop));
    assert!(matches!(state.active_screen(), ScreenId::Launcher));

    let (state, _) = app::update(state, Action::Navigate(NavigationAction::Pop));
    assert_eq!(state.screen_stack, vec![ScreenId::Launcher]);
}

#[test]
fn push_replace_pop_behave_correctly() {
    let state = AppState::default();
    let (state, _) = app::update(
        state,
        Action::Navigate(NavigationAction::Push(ScreenId::Monitor)),
    );
    assert_eq!(state.screen_stack.len(), 2);

    let (state, _) = app::update(
        state,
        Action::Navigate(NavigationAction::ReplaceTop(ScreenId::Cluster {
            id: "cluster-2".to_string(),
        })),
    );
    assert!(matches!(state.active_screen(), ScreenId::Cluster { .. }));

    let (state, _) = app::update(state, Action::Navigate(NavigationAction::Pop));
    assert!(matches!(state.active_screen(), ScreenId::Launcher));
}

#[test]
fn cluster_entry_requests_topology() {
    let state = AppState::default();
    let (_state, effects) = app::update(
        state,
        Action::Navigate(NavigationAction::Push(ScreenId::Cluster {
            id: "cluster-1".to_string(),
        })),
    );

    assert!(
        effects.contains(&Effect::Backend(BackendRequest::GetClusterTopology {
            cluster_id: "cluster-1".to_string(),
        }))
    );
}

#[test]
fn pop_from_cluster_unsubscribes_active_streams() {
    let state = AppState::default();
    let (state, _) = app::update(
        state,
        Action::Navigate(NavigationAction::Push(ScreenId::Cluster {
            id: "cluster-1".to_string(),
        })),
    );

    let mut state = state;
    let entry = state.clusters.get_mut("cluster-1").expect("cluster state");
    entry.log_subscription = Some("log-sub".to_string());
    entry.timeline_subscription = Some("timeline-sub".to_string());

    let (state, effects) = app::update(state, Action::Navigate(NavigationAction::Pop));
    assert!(
        effects.contains(&Effect::Backend(BackendRequest::Unsubscribe {
            subscription_id: "log-sub".to_string(),
        }))
    );
    assert!(
        effects.contains(&Effect::Backend(BackendRequest::Unsubscribe {
            subscription_id: "timeline-sub".to_string(),
        }))
    );

    let entry = state.clusters.get("cluster-1").expect("cluster state");
    assert!(entry.log_subscription.is_none());
    assert!(entry.timeline_subscription.is_none());
}

#[test]
fn push_to_agent_unsubscribes_cluster_streams() {
    let state = AppState::default();
    let (state, _) = app::update(
        state,
        Action::Navigate(NavigationAction::Push(ScreenId::Cluster {
            id: "cluster-1".to_string(),
        })),
    );

    let mut state = state;
    let entry = state.clusters.get_mut("cluster-1").expect("cluster state");
    entry.log_subscription = Some("log-sub".to_string());
    entry.timeline_subscription = Some("timeline-sub".to_string());

    let (state, effects) = app::update(
        state,
        Action::Screen(ScreenAction::Cluster {
            id: "cluster-1".to_string(),
            action: cluster::Action::OpenAgent("agent-1".to_string()),
        }),
    );

    assert!(
        effects.contains(&Effect::Backend(BackendRequest::Unsubscribe {
            subscription_id: "log-sub".to_string(),
        }))
    );
    assert!(
        effects.contains(&Effect::Backend(BackendRequest::Unsubscribe {
            subscription_id: "timeline-sub".to_string(),
        }))
    );

    let entry = state.clusters.get("cluster-1").expect("cluster state");
    assert!(entry.log_subscription.is_none());
    assert!(entry.timeline_subscription.is_none());

    assert!(matches!(
        state.active_screen(),
        ScreenId::Agent { cluster_id, agent_id }
            if cluster_id == "cluster-1" && agent_id == "agent-1"
    ));
}
