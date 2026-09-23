use super::*;

#[test]
fn windows_environment_lookup_is_case_insensitive_and_prefers_userprofile() {
    let mut environment = LocalHarnessEnvironment::for_windows(BTreeMap::from([
        ("Path".to_owned(), r"C:\tools".to_owned()),
        ("openai_Api_Key".to_owned(), "provider-secret".to_owned()),
        ("Home".to_owned(), "/msys/home".to_owned()),
        ("UserProfile".to_owned(), r"C:\Users\native".to_owned()),
    ]));

    assert_eq!(
        environment.get("PATH").map(String::as_str),
        Some(r"C:\tools")
    );
    assert_eq!(
        environment.get("OPENAI_API_KEY").map(String::as_str),
        Some("provider-secret")
    );
    assert_eq!(
        environment.user_home().map(String::as_str),
        Some(r"C:\Users\native")
    );
    *environment
        .get_mut("OpenAI_API_Key")
        .expect("mixed-case key") = "updated-secret".to_owned();
    assert_eq!(
        environment.selected(&["PATH", "OPENAI_API_KEY"]),
        BTreeMap::from([
            ("OPENAI_API_KEY".to_owned(), "updated-secret".to_owned()),
            ("PATH".to_owned(), r"C:\tools".to_owned()),
        ])
    );
}
