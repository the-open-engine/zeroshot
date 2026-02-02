use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use zeroshot_tui::app::{
    self, Action, NavigationAction, ScreenAction, ScreenId, SpineAction, SpineMode, UiVariant,
};
use zeroshot_tui::input;
use zeroshot_tui::protocol::ClusterSummary;
use zeroshot_tui::screens::{cluster, launcher, monitor};

fn state_for(screen: ScreenId) -> app::AppState {
    let mut state = app::AppState::default();
    state.screen_stack = vec![screen];
    state
}

#[test]
fn global_keys_apply_everywhere() {
    let screens = vec![
        ScreenId::Launcher,
        ScreenId::Monitor,
        ScreenId::Cluster {
            id: "c1".to_string(),
        },
        ScreenId::Agent {
            cluster_id: "c1".to_string(),
            agent_id: "a1".to_string(),
        },
    ];

    for screen in screens {
        let state = state_for(screen);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let action = input::route_key(&state, esc);
        assert!(matches!(
            action,
            Some(Action::Navigate(NavigationAction::Pop))
        ));

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let action = input::route_key(&state, ctrl_c);
        assert!(matches!(action, Some(Action::Quit)));
    }
}

#[test]
fn screen_specific_keys_only_apply_to_focused_screen() {
    let launcher = ScreenId::Launcher;
    let monitor_screen = ScreenId::Monitor;
    let cluster_screen = ScreenId::Cluster {
        id: "c1".to_string(),
    };

    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let state = state_for(launcher);
    assert!(input::route_key(&state, down).is_none());

    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let state = state_for(monitor_screen);
    let action = input::route_key(&state, down);
    assert!(matches!(
        action,
        Some(Action::Screen(ScreenAction::Monitor(
            monitor::Action::MoveSelection(1)
        )))
    ));

    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let state = state_for(cluster_screen.clone());
    let action = input::route_key(&state, tab);
    match action {
        Some(Action::Screen(ScreenAction::Cluster { id, action })) => {
            assert_eq!(id, "c1");
            assert!(matches!(
                action,
                cluster::Action::CycleFocus(cluster::FocusDirection::Next)
            ));
        }
        _ => panic!("expected cluster focus action"),
    }

    let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
    let state = state_for(cluster_screen.clone());
    let action = input::route_key(&state, up);
    assert!(matches!(
        action,
        Some(Action::Screen(ScreenAction::Cluster {
            action: cluster::Action::MoveFocused(-1),
            ..
        }))
    ));

    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let state = state_for(cluster_screen.clone());
    let action = input::route_key(&state, down);
    assert!(matches!(
        action,
        Some(Action::Screen(ScreenAction::Cluster {
            action: cluster::Action::MoveFocused(1),
            ..
        }))
    ));

    let k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
    let state = state_for(cluster_screen.clone());
    let action = input::route_key(&state, k);
    assert!(matches!(
        action,
        Some(Action::Screen(ScreenAction::Cluster {
            action: cluster::Action::MoveFocused(-1),
            ..
        }))
    ));

    let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    let state = state_for(cluster_screen.clone());
    let action = input::route_key(&state, j);
    assert!(matches!(
        action,
        Some(Action::Screen(ScreenAction::Cluster {
            action: cluster::Action::MoveFocused(1),
            ..
        }))
    ));

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let state = state_for(cluster_screen);
    let action = input::route_key(&state, enter);
    assert!(matches!(
        action,
        Some(Action::Screen(ScreenAction::Cluster {
            action: cluster::Action::ActivateFocused,
            ..
        }))
    ));

    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let state = state_for(ScreenId::Monitor);
    assert!(input::route_key(&state, tab).is_none());
}

#[test]
fn launcher_keys_edit_input_state() {
    let mut state = app::AppState::default();
    state.screen_stack = vec![ScreenId::Launcher];

    let action = input::route_key(
        &state,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
    )
    .expect("expected insert char");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.input, "a");
    assert_eq!(state.launcher.cursor, 1);

    let action = input::route_key(
        &state,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
    )
    .expect("expected insert char");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.input, "ab");
    assert_eq!(state.launcher.cursor, 2);

    let action = input::route_key(&state, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .expect("expected move left");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.cursor, 1);

    let action = input::route_key(
        &state,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
    )
    .expect("expected backspace");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.input, "b");
    assert_eq!(state.launcher.cursor, 0);

    let action = input::route_key(&state, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE))
        .expect("expected delete");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.input, "");
    assert_eq!(state.launcher.cursor, 0);
}

#[test]
fn q_quits_except_in_launcher_input() {
    let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    let state = state_for(ScreenId::Launcher);
    let action = input::route_key(&state, key);
    assert!(matches!(
        action,
        Some(Action::Screen(ScreenAction::Launcher(
            launcher::Action::InsertChar('q')
        )))
    ));

    let screens = vec![
        ScreenId::Monitor,
        ScreenId::Cluster {
            id: "c1".to_string(),
        },
        ScreenId::Agent {
            cluster_id: "c1".to_string(),
            agent_id: "a1".to_string(),
        },
    ];

    for screen in screens {
        let state = state_for(screen);
        let action = input::route_key(
            &state,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        );
        assert!(matches!(action, Some(Action::Quit)));
    }
}

#[test]
fn disruptive_routes_spine_shortcuts() {
    let mut state = state_for(ScreenId::FleetRadar);
    state.ui_variant = UiVariant::Disruptive;

    let action = input::route_key(
        &state,
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
    );
    match action {
        Some(Action::Spine(SpineAction::EnterMode { mode, prefill })) => {
            assert_eq!(mode, SpineMode::Command);
            assert_eq!(prefill, "");
        }
        _ => panic!("expected command mode for /"),
    }

    let action = input::route_key(
        &state,
        KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
    );
    match action {
        Some(Action::Spine(SpineAction::EnterMode { mode, prefill })) => {
            assert_eq!(mode, SpineMode::Command);
            assert_eq!(prefill, "help ");
        }
        _ => panic!("expected help prefill for ?"),
    }

    let action = input::route_key(
        &state,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    assert!(matches!(action, Some(Action::Spine(SpineAction::Clear))));
}

#[test]
fn disruptive_esc_cancels_or_pops() {
    let mut state = state_for(ScreenId::FleetRadar);
    state.ui_variant = UiVariant::Disruptive;

    let action = input::route_key(&state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(
        action,
        Some(Action::Navigate(NavigationAction::Pop))
    ));

    state.spine.mode = SpineMode::Command;
    let action = input::route_key(&state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(action, Some(Action::Spine(SpineAction::Cancel))));
}

#[test]
fn disruptive_enter_submits_or_zooms() {
    let mut state = state_for(ScreenId::FleetRadar);
    state.ui_variant = UiVariant::Disruptive;
    state.fleet_radar.set_clusters(
        vec![ClusterSummary {
            id: "c1".to_string(),
            state: "running".to_string(),
            provider: None,
            created_at: 0,
            agent_count: 0,
            message_count: 0,
            cwd: None,
        }],
        1000,
    );

    let action = input::route_key(&state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        action,
        Some(Action::Navigate(NavigationAction::Push(ScreenId::ClusterCanvas { id })))
            if id == "c1"
    ));

    state.spine.input.input = "hello".to_string();
    let action = input::route_key(&state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(action, Some(Action::Spine(SpineAction::Submit))));
}

#[test]
fn disruptive_ctrl_c_quits() {
    let mut state = state_for(ScreenId::FleetRadar);
    state.ui_variant = UiVariant::Disruptive;

    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let action = input::route_key(&state, ctrl_c);
    assert!(matches!(action, Some(Action::Quit)));
}
