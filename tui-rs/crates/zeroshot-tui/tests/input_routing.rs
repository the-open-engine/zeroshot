use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use zeroshot_tui::app::{self, Action, NavigationAction, ScreenAction, ScreenId};
use zeroshot_tui::input;
use zeroshot_tui::screens::{cluster, launcher, monitor};

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
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let action = input::route_key(&screen, esc);
        assert!(matches!(
            action,
            Some(Action::Navigate(NavigationAction::Pop))
        ));

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let action = input::route_key(&screen, ctrl_c);
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
    assert!(input::route_key(&launcher, down).is_none());

    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let action = input::route_key(&monitor_screen, down);
    assert!(matches!(
        action,
        Some(Action::Screen(ScreenAction::Monitor(monitor::Action::MoveSelection(1))))
    ));

    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let action = input::route_key(&cluster_screen, tab);
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

    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    assert!(input::route_key(&monitor_screen, tab).is_none());
}

#[test]
fn launcher_keys_edit_input_state() {
    let screen = ScreenId::Launcher;
    let mut state = app::AppState::default();

    let action = input::route_key(
        &screen,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
    )
    .expect("expected insert char");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.input, "a");
    assert_eq!(state.launcher.cursor, 1);

    let action = input::route_key(
        &screen,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
    )
    .expect("expected insert char");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.input, "ab");
    assert_eq!(state.launcher.cursor, 2);

    let action = input::route_key(&screen, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .expect("expected move left");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.cursor, 1);

    let action =
        input::route_key(&screen, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .expect("expected backspace");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.input, "b");
    assert_eq!(state.launcher.cursor, 0);

    let action = input::route_key(&screen, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE))
        .expect("expected delete");
    let (next_state, _) = app::update(state, action);
    state = next_state;
    assert_eq!(state.launcher.input, "");
    assert_eq!(state.launcher.cursor, 0);
}

#[test]
fn q_quits_except_in_launcher_input() {
    let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    let action = input::route_key(&ScreenId::Launcher, key);
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
        let action = input::route_key(&screen, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(matches!(action, Some(Action::Quit)));
    }
}
