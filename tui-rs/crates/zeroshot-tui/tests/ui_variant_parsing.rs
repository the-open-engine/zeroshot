use zeroshot_tui::app::{resolve_ui_variant, UiVariant};

#[test]
fn ui_variant_defaults_to_none() {
    let result = resolve_ui_variant(None, None).expect("resolve");
    assert_eq!(result, None);
}

#[test]
fn ui_variant_parses_case_insensitive() {
    let result = resolve_ui_variant(None, Some("Disruptive")).expect("resolve");
    assert_eq!(result, Some(UiVariant::Disruptive));
}

#[test]
fn ui_variant_cli_overrides_env() {
    let result = resolve_ui_variant(Some("classic"), Some("disruptive")).expect("resolve");
    assert_eq!(result, Some(UiVariant::Classic));
}

#[test]
fn ui_variant_rejects_unknown() {
    let err = resolve_ui_variant(Some("weird"), None).expect_err("expected error");
    assert!(err.contains("Unknown UI variant"));
}
