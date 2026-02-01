use crate::app::{
    Action, CommandAction, CommandContext, NavigationAction, ScreenId, ToastLevel, UiVariant,
};
use crate::commands::types::{ParsedCommand, VALID_PROVIDERS};

pub fn dispatch(parsed: ParsedCommand, context: CommandContext) -> Vec<Action> {
    match parsed.name.as_str() {
        "help" => vec![Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Info,
            message: help_message(),
        })],
        "monitor" => handle_monitor(context),
        "issue" => handle_issue(parsed, context),
        "provider" => handle_provider(parsed),
        "quit" | "exit" => vec![Action::Quit],
        other => vec![Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Error,
            message: format!("Unknown command: {other}. Try /help."),
        })],
    }
}

fn handle_monitor(context: CommandContext) -> Vec<Action> {
    let (target, label) = if matches!(context.ui_variant, UiVariant::Disruptive) {
        (ScreenId::FleetRadar, "Fleet Radar")
    } else {
        (ScreenId::Monitor, "Monitor")
    };

    if context.active_screen == target {
        return vec![Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Info,
            message: format!("Already on {label}."),
        })];
    }

    vec![
        Action::Navigate(NavigationAction::Push(target)),
        Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Success,
            message: format!("Opened {label}."),
        }),
    ]
}

fn handle_issue(parsed: ParsedCommand, context: CommandContext) -> Vec<Action> {
    let Some(reference) = parsed.args.first() else {
        return vec![Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Error,
            message: "Usage: /issue <ref>".to_string(),
        })];
    };

    vec![
        Action::Command(CommandAction::StartClusterFromIssue {
            reference: reference.to_string(),
            provider_override: context.provider_override,
        }),
        Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Info,
            message: format!("Starting cluster from issue {reference}..."),
        }),
    ]
}

fn handle_provider(parsed: ParsedCommand) -> Vec<Action> {
    let Some(name) = parsed.args.first() else {
        return vec![Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Error,
            message: format!("Usage: /provider <{}>", VALID_PROVIDERS.join("|")),
        })];
    };

    let normalized = name.to_lowercase();
    if !VALID_PROVIDERS.contains(&normalized.as_str()) {
        return vec![Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Error,
            message: format!(
                "Unknown provider '{name}'. Use one of: {}",
                VALID_PROVIDERS.join(", ")
            ),
        })];
    }

    vec![
        Action::Command(CommandAction::SetProviderOverride {
            provider: Some(normalized.clone()),
        }),
        Action::Command(CommandAction::ShowToast {
            level: ToastLevel::Success,
            message: format!("Provider override set to {normalized}."),
        }),
    ]
}

fn help_message() -> String {
    let lines = [
        "Commands: /help /monitor /issue <ref> /provider <name> /quit /exit",
        "Keys: / command bar, ? help, Esc back, q quit (not in Launcher), Ctrl+C quit, j/k or arrows move, PgUp/PgDn fast, Tab/Shift+Tab or h/l switch panes",
    ];
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::dispatch;
    use crate::app::{Action, CommandContext, NavigationAction, ScreenId, ToastLevel, UiVariant};
    use crate::commands::types::ParsedCommand;

    fn context() -> CommandContext {
        CommandContext {
            provider_override: None,
            active_screen: ScreenId::Launcher,
            ui_variant: UiVariant::Classic,
        }
    }

    #[test]
    fn unknown_command_returns_error_toast() {
        let parsed = ParsedCommand {
            raw: "/nope".to_string(),
            name: "nope".to_string(),
            args: vec![],
        };
        let actions = dispatch(parsed, context());
        let toast = actions
            .iter()
            .find_map(|action| match action {
                crate::app::Action::Command(crate::app::CommandAction::ShowToast {
                    level,
                    message,
                }) => Some((level, message)),
                _ => None,
            })
            .expect("expected toast action");
        assert_eq!(toast.0, &ToastLevel::Error);
        assert!(toast.1.contains("Unknown command"));
    }

    #[test]
    fn invalid_provider_is_rejected() {
        let parsed = ParsedCommand {
            raw: "/provider nope".to_string(),
            name: "provider".to_string(),
            args: vec!["nope".to_string()],
        };
        let actions = dispatch(parsed, context());
        let toast = actions
            .iter()
            .find_map(|action| match action {
                crate::app::Action::Command(crate::app::CommandAction::ShowToast {
                    level,
                    message,
                }) => Some((level, message)),
                _ => None,
            })
            .expect("expected toast action");
        assert_eq!(toast.0, &ToastLevel::Error);
        assert!(toast.1.contains("Unknown provider"));
    }

    #[test]
    fn monitor_command_targets_fleet_radar_in_disruptive() {
        let parsed = ParsedCommand {
            raw: "/monitor".to_string(),
            name: "monitor".to_string(),
            args: vec![],
        };
        let mut context = context();
        context.ui_variant = UiVariant::Disruptive;
        let actions = dispatch(parsed, context);
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::Navigate(NavigationAction::Push(ScreenId::FleetRadar))
        )));
    }

    #[test]
    fn monitor_command_targets_monitor_in_classic() {
        let parsed = ParsedCommand {
            raw: "/monitor".to_string(),
            name: "monitor".to_string(),
            args: vec![],
        };
        let actions = dispatch(parsed, context());
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::Navigate(NavigationAction::Push(ScreenId::Monitor))
        )));
    }
}
