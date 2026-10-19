use super::*;

#[cfg(unix)]
#[test]
fn rejects_socket_paths_that_cannot_fit_the_platform_address() {
    let capacity = local_socket_path_capacity();
    assert!(validate_local_socket_path(Path::new(&"x".repeat(capacity - 1))).is_ok());
    assert!(
        validate_local_socket_path(Path::new(&"x".repeat(capacity)))
            .is_err_and(|error| error.to_string().contains("ZEROSHOT_STATE_DIR"))
    );
}

#[test]
fn local_controller_receives_only_minimal_non_harness_environment() {
    let mut command = Command::new("true");
    command.env_clear();
    copy_minimal_process_environment(&mut command).expect("copy local environment");
    let environment = command
        .as_std()
        .get_envs()
        .filter_map(|(name, value)| value.map(|value| (name.to_owned(), value.to_owned())))
        .collect::<std::collections::BTreeMap<_, _>>();

    for name in ["HOME", "USERPROFILE", "PATH"] {
        let expected = std::env::var_os(name).filter(|value| !value.is_empty());
        assert_eq!(
            environment.get(std::ffi::OsStr::new(name)),
            expected.as_ref()
        );
    }
    for name in [
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "COPILOT_HOME",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "COPILOT_GITHUB_TOKEN",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_RUNTIME_DIR",
    ] {
        assert!(!environment.contains_key(std::ffi::OsStr::new(name)));
    }
}

#[test]
fn local_controller_receives_an_absolute_restic_selector() {
    let environment = |restic: OsString| {
        let mut command = Command::new("true");
        command.env_clear();
        copy_minimal_environment(&mut command, |name| {
            (name == "ZEROSHOT_RESTIC").then(|| restic.clone())
        })
        .expect("copy local environment");
        command
            .as_std()
            .get_envs()
            .filter_map(|(name, value)| value.map(|value| (name.to_owned(), value.to_owned())))
            .collect::<std::collections::BTreeMap<_, _>>()
    };

    let relative = Path::new("tools").join("restic");
    let forwarded = environment(relative.clone().into_os_string());
    assert_eq!(
        forwarded.get(std::ffi::OsStr::new("ZEROSHOT_RESTIC")),
        Some(
            &std::env::current_dir()
                .expect("current directory")
                .join(relative)
                .into_os_string()
        )
    );
    assert_eq!(
        environment(OsString::new()).get(std::ffi::OsStr::new("ZEROSHOT_RESTIC")),
        Some(&OsString::new())
    );
}
