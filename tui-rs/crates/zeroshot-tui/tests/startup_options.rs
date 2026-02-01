use zeroshot_tui::app::{AppState, InitialScreen, ScreenId, StartupOptions, UiVariant};

#[test]
fn startup_options_apply_monitor_and_provider_override() {
    let mut state = AppState::default();
    let options = StartupOptions {
        initial_screen: Some(InitialScreen::Monitor),
        provider_override: Some("codex".to_string()),
        ui_variant: Some(UiVariant::Disruptive),
    };

    state.apply_startup_options(options);

    assert_eq!(
        state.screen_stack,
        vec![ScreenId::IntentConsole, ScreenId::FleetRadar]
    );
    assert_eq!(state.provider_override, Some("codex".to_string()));
    assert_eq!(state.ui_variant, UiVariant::Disruptive);
}
