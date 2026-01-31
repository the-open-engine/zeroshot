use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, NavigationAction, ScreenAction, ScreenId};
use crate::screens::{cluster, launcher, monitor};

pub fn route_key(screen: &ScreenId, key: KeyEvent) -> Option<Action> {
    if let Some(action) = route_global(key) {
        return Some(action);
    }

    match screen {
        ScreenId::Launcher => route_launcher(key),
        ScreenId::Monitor => route_monitor(key),
        ScreenId::Cluster { id } => route_cluster(id, key),
        ScreenId::Agent { .. } => None,
    }
}

fn route_global(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::Navigate(NavigationAction::Pop)),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::Quit)
        }
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

fn route_launcher(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Enter => Some(Action::Screen(ScreenAction::Launcher(launcher::Action::Submit))),
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
    let direction = match key.code {
        KeyCode::Tab | KeyCode::Right => Some(cluster::FocusDirection::Next),
        KeyCode::Left => Some(cluster::FocusDirection::Prev),
        _ => None,
    }?;

    Some(Action::Screen(ScreenAction::Cluster {
        id: id.to_string(),
        action: cluster::Action::CycleFocus(direction),
    }))
}
