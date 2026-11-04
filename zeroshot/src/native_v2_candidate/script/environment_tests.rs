use super::*;

fn captured_windows_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Path".to_owned(), r"C:\tools".to_owned()),
        ("UserProfile".to_owned(), r"C:\Users\native".to_owned()),
        ("HOME".to_owned(), "/msys/home".to_owned()),
        ("tMp".to_owned(), r"C:\temporary".to_owned()),
        ("SystemRoot".to_owned(), r"C:\Windows".to_owned()),
        ("OPENAI_API_KEY".to_owned(), "provider-secret".to_owned()),
        ("ZEROSHOT_API_KEY".to_owned(), "transport-secret".to_owned()),
    ])
}

#[test]
fn script_context_uses_windows_lookup_and_home_selection_without_ambient_credentials() {
    let captured = LocalHarnessEnvironment::for_windows(captured_windows_environment());
    let selected = script_environment(&captured);
    assert_eq!(selected["PATH"], r"C:\tools");
    assert_eq!(selected["HOME"], r"C:\Users\native");
    assert_eq!(selected["USERPROFILE"], selected["HOME"]);
    assert_eq!(selected["TMP"], r"C:\temporary");
    assert_eq!(selected["SYSTEMROOT"], r"C:\Windows");
    assert!(!selected.contains_key("OPENAI_API_KEY"));
    assert!(!selected.contains_key("ZEROSHOT_API_KEY"));
}

#[cfg(windows)]
#[test]
fn windows_script_adapter_keeps_the_captured_search_path_and_profile() {
    let captured = captured_windows_environment();
    let adapter = ScriptAdapter::new(PathBuf::from("."), Some(&captured));
    assert_eq!(
        adapter.environment,
        script_environment(&LocalHarnessEnvironment::for_windows(captured))
    );
}
