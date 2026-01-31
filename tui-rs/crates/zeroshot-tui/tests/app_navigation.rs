use zeroshot_tui::app::{self, Action, AppState, NavigationAction, ScreenId};

#[test]
fn esc_pops_until_launcher_root() {
    let state = AppState::default();
    let (state, _) = app::update(state, Action::Navigate(NavigationAction::Push(ScreenId::Monitor)));
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
    let (state, _) = app::update(state, Action::Navigate(NavigationAction::Push(ScreenId::Monitor)));
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
