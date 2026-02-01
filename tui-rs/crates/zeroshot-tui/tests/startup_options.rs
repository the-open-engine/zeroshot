use zeroshot_tui::app::{AppState, InitialScreen, ScreenId, StartupOptions};

#[test]
fn startup_options_apply_monitor_and_provider_override() {
    let mut state = AppState::default();
    let options = StartupOptions {
        initial_screen: Some(InitialScreen::Monitor),
        provider_override: Some("codex".to_string()),
    };

    state.apply_startup_options(options);

    assert_eq!(
        state.screen_stack,
        vec![ScreenId::Launcher, ScreenId::Monitor]
    );
    assert_eq!(state.provider_override, Some("codex".to_string()));
}
