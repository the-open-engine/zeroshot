use super::*;

#[test]
fn target_api_keys_never_enter_the_local_harness_snapshot() {
    let environment = LocalHarnessEnvironment::for_windows(BTreeMap::from([
        ("ZEROSHOT_API_KEY".to_owned(), "transport-secret".to_owned()),
        (
            "zeroshot_api_key".to_owned(),
            "other-transport-secret".to_owned(),
        ),
        ("OPENAI_API_KEY".to_owned(), "provider-secret".to_owned()),
    ]));
    assert!(environment.get("ZEROSHOT_API_KEY").is_none());
    assert_eq!(
        environment.into_values(),
        BTreeMap::from([("OPENAI_API_KEY".to_owned(), "provider-secret".to_owned()),])
    );
}

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
