use super::*;

fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    BTreeMap<String, String>,
) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("checkout");
    std::fs::create_dir(&workspace).unwrap();
    let environment =
        BTreeMap::from([(GITHUB_TOKEN_FILE_ENV.to_owned(), "renewed-token".to_owned())]);
    (root, workspace, environment)
}

#[test]
fn token_file_capture_resolves_caller_paths_and_removes_provider_controls() {
    for absolute in [false, true] {
        let (root, workspace, mut environment) = fixture();
        let expected = root.path().canonicalize().unwrap().join("renewed-token");
        if absolute {
            environment.insert(
                GITHUB_TOKEN_FILE_ENV.to_owned(),
                expected.to_str().unwrap().to_owned(),
            );
        }
        let (token, file) =
            take_github_authority(&mut environment, None, root.path(), &workspace).unwrap();
        assert!(token.is_none());
        assert_eq!(file.unwrap().path(), expected);
        assert!(environment.is_empty());
    }
}

#[test]
fn explicit_github_token_takes_precedence_without_exposing_it_to_provider_context() {
    let names = if cfg!(windows) {
        vec!["GH_TOKEN", "gh_token"]
    } else {
        vec!["GH_TOKEN"]
    };
    for name in names {
        let (root, workspace, mut environment) = fixture();
        environment.insert(name.into(), "explicit-token".into());
        let (token, file) =
            take_github_authority(&mut environment, None, root.path(), &workspace).unwrap();
        assert_eq!(token.as_deref(), Some("explicit-token"));
        assert!(file.is_none());
        assert!(environment.is_empty());
    }
}

#[test]
fn invalid_explicit_github_token_never_falls_back_to_the_file() {
    for token in ["", " ", "has space", "line\nbreak", "nul\0byte"] {
        let (root, workspace, mut environment) = fixture();
        environment.insert("GH_TOKEN".into(), token.into());
        assert!(take_github_authority(&mut environment, None, root.path(), &workspace).is_err());
        assert!(environment.is_empty());
    }
}

#[test]
fn no_token_file_keeps_existing_local_authority_behavior() {
    let (root, workspace, _) = fixture();
    let mut environment = BTreeMap::from([("GH_TOKEN".to_owned(), "native-token".to_owned())]);
    let (token, file) = take_github_authority(
        &mut environment,
        Some("trusted-token".into()),
        root.path(),
        &workspace,
    )
    .unwrap();
    assert_eq!(token.as_deref(), Some("trusted-token"));
    assert!(file.is_none());
    assert_eq!(environment.get("GH_TOKEN").unwrap(), "native-token");
}

#[cfg(unix)]
#[test]
fn non_utf8_explicit_github_token_is_rejected_before_token_file_fallback() {
    const CHILD: &str = "ZEROSHOT_TEST_TOKEN_CAPTURE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        assert!(
            crate::native_v2_local::capture_local_native_environment(
                &std::env::current_dir().unwrap()
            )
            .is_err(),
            "a present malformed token must not disappear from the caller snapshot"
        );
        return;
    }
    use std::os::unix::ffi::OsStringExt;
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native_v2_local::token_file::tests::non_utf8_explicit_github_token_is_rejected_before_token_file_fallback"])
        .env_clear()
        .env(CHILD, "1")
        .env(GITHUB_TOKEN_FILE_ENV, "/trusted/renewed-token")
        .env("GH_TOKEN", std::ffi::OsString::from_vec(vec![0xff]))
        .output().unwrap();
    assert!(
        output.status.success(),
        "malformed-token subprocess failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(unix)]
#[test]
fn caller_control_validation_rejects_non_utf8_settings_before_snapshot_filtering() {
    use std::os::unix::ffi::OsStringExt;
    for name in [GITHUB_TOKEN_FILE_ENV, DELIVERY_CHECK_COMMAND_ENV] {
        assert!(
            validate_caller_authority(
                |field| (field == name).then(|| OsString::from_vec(vec![0xff]))
            )
            .is_err()
        );
    }
    assert!(
        validate_caller_authority(
            |field| (field == "GH_TOKEN").then(|| OsString::from_vec(vec![0xff]))
        )
        .is_ok(),
        "without file mode, preserve unrelated native environment behavior"
    );
}

#[cfg(unix)]
#[test]
fn delivery_validator_capture_removes_the_control_and_selects_a_fresh_private_executable() {
    use openengine_cluster_testkit::fixture::write_executable;
    let (root, workspace, _) = fixture();
    let mut paths = Vec::new();
    for name in ["first-validator", "replacement-validator"] {
        let executable = root.path().join(name);
        write_executable(&executable, "#!/bin/sh\nexit 0\n", 0o700).unwrap();
        let mut environment = BTreeMap::from([(
            DELIVERY_CHECK_COMMAND_ENV.to_owned(),
            executable.to_str().unwrap().to_owned(),
        )]);
        let capability = take_delivery_validator(&mut environment, &workspace)
            .unwrap()
            .unwrap();
        assert_eq!(capability.path(), executable);
        assert!(environment.is_empty());
        assert!(!format!("{capability:?}").contains(executable.to_str().unwrap()));
        paths.push(capability.path().to_owned());
    }
    assert_ne!(paths[0], paths[1], "resume uses the current caller setting");
    for invalid in ["relative-validator", "", "/missing-validator --argument"] {
        let mut environment =
            BTreeMap::from([(DELIVERY_CHECK_COMMAND_ENV.to_owned(), invalid.to_owned())]);
        assert!(take_delivery_validator(&mut environment, &workspace).is_err());
        assert!(environment.is_empty());
    }
}

#[test]
fn provider_context_excludes_delivery_controls_without_creating_capabilities() {
    let environment = BTreeMap::from([
        (
            GITHUB_TOKEN_FILE_ENV.to_owned(),
            "/private/token".to_owned(),
        ),
        (
            DELIVERY_CHECK_COMMAND_ENV.to_owned(),
            "/private/validator".to_owned(),
        ),
        ("PUBLIC_SETTING".to_owned(), "retained".to_owned()),
    ]);
    assert_eq!(
        provider_environment(&environment),
        BTreeMap::from([("PUBLIC_SETTING".to_owned(), "retained".to_owned())])
    );
    assert_eq!(
        environment.len(),
        3,
        "the caller snapshot remains owned by the caller"
    );
}

#[test]
fn caller_validation_checks_explicit_credentials_before_file_mode_capture() {
    for (token, accepted) in [
        ("explicit-token", true),
        ("", false),
        ("bad token", false),
        ("line\nbreak", false),
    ] {
        let caller = BTreeMap::from([
            (GITHUB_TOKEN_FILE_ENV, OsString::from("/private/token")),
            ("GH_TOKEN", OsString::from(token)),
        ]);
        assert_eq!(
            validate_caller_authority(|name| caller.get(name).cloned()).is_ok(),
            accepted
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let caller = BTreeMap::from([
            (GITHUB_TOKEN_FILE_ENV, OsString::from("/private/token")),
            ("GH_TOKEN", OsString::from_vec(vec![0xff])),
        ]);
        assert!(validate_caller_authority(|name| caller.get(name).cloned()).is_err());
    }
}

#[test]
fn empty_file_control_fails_closed_without_leaking_to_provider_context() {
    let (root, workspace, mut environment) = fixture();
    environment.insert(GITHUB_TOKEN_FILE_ENV.to_owned(), String::new());
    assert!(take_github_authority(&mut environment, None, root.path(), &workspace).is_err());
    assert!(environment.is_empty());
}
