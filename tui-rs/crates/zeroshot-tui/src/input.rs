use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, NavigationAction, ScreenAction, ScreenId};
use crate::screens::{cluster, launcher, monitor};

pub fn route_key(screen: &ScreenId, key: KeyEvent) -> Option<Action> {
    if let Some(action) = route_global(screen, key) {
        return Some(action);
    }

    match screen {
        ScreenId::Launcher => route_launcher(key),
        ScreenId::Monitor => route_monitor(key),
        ScreenId::Cluster { id } => route_cluster(id, key),
        ScreenId::Agent { .. } => None,
    }
}

fn route_global(screen: &ScreenId, key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::Navigate(NavigationAction::Pop)),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::Quit)
        }
        KeyCode::Char('q') => match screen {
            ScreenId::Launcher => None,
            _ => Some(Action::Quit),
        },
        _ => None,
    }
}

fn route_launcher(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Enter => Some(Action::Screen(ScreenAction::Launcher(launcher::Action::Submit))),
        KeyCode::Backspace => Some(Action::Screen(ScreenAction::Launcher(
            launcher::Action::Backspace,
        ))),
        KeyCode::Delete => Some(Action::Screen(ScreenAction::Launcher(
            launcher::Action::Delete,
        ))),
        KeyCode::Left => Some(Action::Screen(ScreenAction::Launcher(
            launcher::Action::MoveCursorLeft,
        ))),
        KeyCode::Right => Some(Action::Screen(ScreenAction::Launcher(
            launcher::Action::MoveCursorRight,
        ))),
        KeyCode::Home => Some(Action::Screen(ScreenAction::Launcher(
            launcher::Action::MoveCursorHome,
        ))),
        KeyCode::End => Some(Action::Screen(ScreenAction::Launcher(
            launcher::Action::MoveCursorEnd,
        ))),
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(Action::Screen(ScreenAction::Launcher(
                launcher::Action::InsertChar(ch),
            )))
        }
        _ => None,
    }
}

fn route_monitor(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(Action::Screen(ScreenAction::Monitor(
            monitor::Action::MoveSelection(-1),
        ))),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::Screen(ScreenAction::Monitor(
            monitor::Action::MoveSelection(1),
        ))),
        KeyCode::Enter => Some(Action::Screen(ScreenAction::Monitor(
            monitor::Action::OpenSelected,
        ))),
        _ => None,
    }
}

fn route_cluster(id: &str, key: KeyEvent) -> Option<Action> {
    let action = match key.code {
        KeyCode::Tab | KeyCode::Right => {
            cluster::Action::CycleFocus(cluster::FocusDirection::Next)
        }
        KeyCode::Left => cluster::Action::CycleFocus(cluster::FocusDirection::Prev),
        KeyCode::Up | KeyCode::Char('k') => cluster::Action::MoveFocused(-1),
        KeyCode::Down | KeyCode::Char('j') => cluster::Action::MoveFocused(1),
        KeyCode::Enter => cluster::Action::ActivateFocused,
        _ => return None,
    };

    Some(Action::Screen(ScreenAction::Cluster {
        id: id.to_string(),
        action,
    }))
}
