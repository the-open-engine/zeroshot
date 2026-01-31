use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use zeroshot_tui::app::{Action, NavigationAction, ScreenAction, ScreenId};
use zeroshot_tui::input;
use zeroshot_tui::screens::{cluster, monitor};

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
