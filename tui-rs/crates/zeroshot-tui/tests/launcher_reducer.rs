use zeroshot_tui::app::{
    self, Action, AppState, BackendAction, BackendRequest, CommandContext, CommandRequest, Effect,
    ScreenAction, ScreenId,
};
use zeroshot_tui::screens::launcher;

#[test]
fn submit_text_routes_to_start_cluster_from_text() {
    let mut state = AppState::default();
    state.launcher.input = "123".to_string();
    state.provider_override = Some("claude".to_string());

    let (state, effects) = app::update(
        state,
        Action::Screen(ScreenAction::Launcher(launcher::Action::Submit)),
    );

    assert_eq!(state.last_error, None);
    assert_eq!(
        effects,
        vec![Effect::Backend(BackendRequest::StartClusterFromText {
            text: "123".to_string(),
            provider_override: Some("claude".to_string()),
        })]
    );
}

#[test]
fn submit_command_routes_to_command_effect() {
    let mut state = AppState::default();
    state.launcher.input = "/help".to_string();

    let (state, effects) = app::update(
        state,
        Action::Screen(ScreenAction::Launcher(launcher::Action::Submit)),
    );

    assert_eq!(state.last_error, None);
    assert_eq!(
        effects,
        vec![Effect::Command(CommandRequest::SubmitRaw {
            raw: "/help".to_string(),
            context: CommandContext {
                provider_override: None,
                active_screen: ScreenId::Launcher,
                ui_variant: state.ui_variant,
            },
        })]
    );
}

#[test]
fn backend_error_sets_last_error() {
    let state = AppState::default();
    let (state, _) = app::update(
        state,
        Action::Backend(BackendAction::Error("boom".to_string())),
    );

    assert_eq!(state.last_error.as_deref(), Some("boom"));
}
