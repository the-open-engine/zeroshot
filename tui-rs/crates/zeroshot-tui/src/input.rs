use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{
    Action, AppState, CommandBarAction, NavigationAction, ScreenAction, ScreenId, SpineAction,
    SpineMode, UiVariant, ZoomStackContext,
};
use crate::screens::{agent, cluster, launcher, monitor};

pub fn route_key(state: &AppState, key: KeyEvent) -> Option<Action> {
    if matches!(state.ui_variant, UiVariant::Disruptive) {
        return route_disruptive(state, key);
    }

    if state.command_bar.active {
        return route_command_bar(key);
    }

    let screen = state.active_screen();
    if let Some(action) = route_global(screen, key) {
        return Some(action);
    }

    if !matches!(screen, ScreenId::Launcher | ScreenId::IntentConsole) {
        match key.code {
            KeyCode::Char('/')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                return Some(Action::CommandBar(CommandBarAction::Open {
                    prefill: "/".to_string(),
                }));
            }
            KeyCode::Char('?')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                return Some(Action::CommandBar(CommandBarAction::Open {
                    prefill: "/help ".to_string(),
                }));
            }
            _ => {}
        }
    }

    match screen {
        ScreenId::Launcher => route_launcher(key),
        ScreenId::Monitor => route_monitor(key),
        ScreenId::Cluster { id } => route_cluster(id, key),
        ScreenId::Agent {
            cluster_id,
            agent_id,
        } => route_agent(cluster_id, agent_id, key),
        ScreenId::IntentConsole
        | ScreenId::FleetRadar
        | ScreenId::ClusterCanvas { .. }
        | ScreenId::AgentMicroscope { .. } => None,
    }
}

fn route_disruptive(state: &AppState, key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if ctrl && matches!(key.code, KeyCode::Char('c')) {
        return Some(Action::Quit);
    }

    match key.code {
        KeyCode::Esc => {
            if spine_active(state) {
                Some(Action::Spine(SpineAction::Cancel))
            } else {
                Some(Action::Navigate(NavigationAction::Pop))
            }
        }
        KeyCode::Enter => {
            if spine_active(state) {
                Some(Action::Spine(SpineAction::Submit))
            } else {
                zoom_in_action(state)
            }
        }
        KeyCode::Char('?') if !ctrl && !alt => Some(Action::Spine(SpineAction::EnterMode {
            mode: SpineMode::Command,
            prefill: "help ".to_string(),
        })),
        KeyCode::Char('/') if !ctrl && !alt => Some(Action::Spine(SpineAction::EnterMode {
            mode: SpineMode::Command,
            prefill: String::new(),
        })),
        KeyCode::Char('i') if !ctrl && !alt => Some(Action::Spine(SpineAction::EnterMode {
            mode: intent_mode_for_context(state),
            prefill: String::new(),
        })),
        KeyCode::Char('u') if ctrl => Some(Action::Spine(SpineAction::Clear)),
        KeyCode::Tab => Some(Action::Spine(SpineAction::Complete)),
        KeyCode::Backspace => Some(Action::Spine(SpineAction::Backspace)),
        KeyCode::Delete => Some(Action::Spine(SpineAction::Delete)),
        KeyCode::Left => Some(Action::Spine(SpineAction::MoveCursorLeft)),
        KeyCode::Right => Some(Action::Spine(SpineAction::MoveCursorRight)),
        KeyCode::Home => Some(Action::Spine(SpineAction::MoveCursorHome)),
        KeyCode::End => Some(Action::Spine(SpineAction::MoveCursorEnd)),
        KeyCode::Char(ch) if !ctrl && !alt => Some(Action::Spine(SpineAction::InsertChar(ch))),
        _ => None,
    }
}

fn spine_active(state: &AppState) -> bool {
    !state.spine.input.input.is_empty()
        || !matches!(state.spine.mode, SpineMode::Intent)
        || state.spine.completion.is_some()
}

fn intent_mode_for_context(state: &AppState) -> SpineMode {
    match state.zoom_stack_context() {
        ZoomStackContext::Agent { .. } => SpineMode::WhisperAgent,
        ZoomStackContext::Cluster { .. } => SpineMode::WhisperCluster,
        ZoomStackContext::FleetRadar => {
            if selected_cluster_id_for_zoom(state).is_some() {
                SpineMode::WhisperCluster
            } else {
                SpineMode::Intent
            }
        }
        ZoomStackContext::Root => SpineMode::Intent,
    }
}

fn zoom_in_action(state: &AppState) -> Option<Action> {
    match state.zoom_stack_context() {
        ZoomStackContext::FleetRadar => selected_cluster_id_for_zoom(state).map(|cluster_id| {
            Action::Navigate(NavigationAction::Push(ScreenId::ClusterCanvas { id: cluster_id }))
        }),
        ZoomStackContext::Cluster { id } => selected_agent_id(state, &id).map(|agent_id| {
            Action::Navigate(NavigationAction::Push(ScreenId::AgentMicroscope {
                cluster_id: id,
                agent_id,
            }))
        }),
        ZoomStackContext::Agent { .. } | ZoomStackContext::Root => None,
    }
}

fn selected_cluster_id_for_zoom(state: &AppState) -> Option<String> {
    match state.active_screen() {
        ScreenId::Monitor => state.monitor.selected_cluster_id(),
        _ => state.fleet_radar.selected_cluster_id(),
    }
}

fn selected_agent_id(state: &AppState, cluster_id: &str) -> Option<String> {
    let cluster_state = state.clusters.get(cluster_id)?;
    let agent = cluster_state.agents.get(cluster_state.selected_agent)?;
    Some(agent.id.clone())
}

fn route_command_bar(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::CommandBar(CommandBarAction::Close)),
        KeyCode::Enter => Some(Action::CommandBar(CommandBarAction::Submit)),
        KeyCode::Backspace => Some(Action::CommandBar(CommandBarAction::Backspace)),
        KeyCode::Delete => Some(Action::CommandBar(CommandBarAction::Delete)),
        KeyCode::Left => Some(Action::CommandBar(CommandBarAction::MoveCursorLeft)),
        KeyCode::Right => Some(Action::CommandBar(CommandBarAction::MoveCursorRight)),
        KeyCode::Home => Some(Action::CommandBar(CommandBarAction::MoveCursorHome)),
        KeyCode::End => Some(Action::CommandBar(CommandBarAction::MoveCursorEnd)),
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(Action::CommandBar(CommandBarAction::InsertChar(ch)))
        }
        _ => None,
    }
}

fn route_global(screen: &ScreenId, key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::Navigate(NavigationAction::Pop)),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::Quit)
        }
        KeyCode::Char('q') => match screen {
            ScreenId::Launcher | ScreenId::IntentConsole => None,
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
        KeyCode::PageUp => Some(Action::Screen(ScreenAction::Monitor(
            monitor::Action::MoveSelection(-5),
        ))),
        KeyCode::PageDown => Some(Action::Screen(ScreenAction::Monitor(
            monitor::Action::MoveSelection(5),
        ))),
        KeyCode::Enter => Some(Action::Screen(ScreenAction::Monitor(
            monitor::Action::OpenSelected,
        ))),
        _ => None,
    }
}

fn route_cluster(id: &str, key: KeyEvent) -> Option<Action> {
    let action = match key.code {
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
            cluster::Action::CycleFocus(cluster::FocusDirection::Next)
        }
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
            cluster::Action::CycleFocus(cluster::FocusDirection::Prev)
        }
        KeyCode::Up | KeyCode::Char('k') => cluster::Action::MoveFocused(-1),
        KeyCode::Down | KeyCode::Char('j') => cluster::Action::MoveFocused(1),
        KeyCode::PageUp => cluster::Action::MoveFocused(-5),
        KeyCode::PageDown => cluster::Action::MoveFocused(5),
        KeyCode::Enter => cluster::Action::ActivateFocused,
        _ => return None,
    };

    Some(Action::Screen(ScreenAction::Cluster {
        id: id.to_string(),
        action,
    }))
}

fn route_agent(cluster_id: &str, agent_id: &str, key: KeyEvent) -> Option<Action> {
    let action = match key.code {
        KeyCode::Enter => agent::Action::SubmitGuidance,
        KeyCode::Backspace => agent::Action::Backspace,
        KeyCode::Delete => agent::Action::Delete,
        KeyCode::Left => agent::Action::MoveCursorLeft,
        KeyCode::Right => agent::Action::MoveCursorRight,
        KeyCode::Home => agent::Action::MoveCursorHome,
        KeyCode::End => agent::Action::MoveCursorEnd,
        KeyCode::Up | KeyCode::Char('k') => agent::Action::ScrollLogs(-1),
        KeyCode::Down | KeyCode::Char('j') => agent::Action::ScrollLogs(1),
        KeyCode::PageUp => agent::Action::ScrollLogs(-5),
        KeyCode::PageDown => agent::Action::ScrollLogs(5),
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            agent::Action::InsertChar(ch)
        }
        _ => return None,
    };

    Some(Action::Screen(ScreenAction::Agent {
        cluster_id: cluster_id.to_string(),
        agent_id: agent_id.to_string(),
        action,
    }))
}
