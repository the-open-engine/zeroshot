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

#[test]
fn local_run_ids_require_canonical_uuid_v7_controller_directories() {
    let valid = RunId::new("01890a5d-ac96-774b-bcce-b302099a8057");
    validate_local_run_id(&valid).expect("canonical v7 run ID");
    for rejected in [
        "run-failed",
        "01890a5d-ac96-174b-bcce-b302099a8057",
        "01890A5D-AC96-774B-BCCE-B302099A8057",
    ] {
        assert!(
            validate_local_run_id(&RunId::new(rejected)).is_err(),
            "{rejected}"
        );
    }

    let directory = tempfile::tempdir().expect("state test directory");
    std::fs::create_dir(directory.path().join(valid.as_str())).expect("valid run directory");
    std::fs::create_dir(directory.path().join("not-a-run")).expect("invalid run directory");
    std::fs::write(directory.path().join("ledger"), b"state").expect("ledger file");
    let mut discovered = std::fs::read_dir(directory.path())
        .expect("read state directory")
        .filter_map(|entry| local_run_id_from_entry(entry.expect("state entry")))
        .map(|run_id| run_id.as_str().to_owned())
        .collect::<Vec<_>>();
    discovered.sort();
    assert_eq!(discovered, vec![valid.as_str().to_owned()]);

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            directory.path().join(valid.as_str()),
            directory
                .path()
                .join("01890a5d-ac96-774b-bcce-b302099a8058"),
        )
        .expect("run directory symlink");
        assert!(
            std::fs::read_dir(directory.path())
                .expect("read state directory")
                .filter_map(|entry| local_run_id_from_entry(entry.expect("state entry")))
                .all(|run_id| run_id != RunId::new("01890a5d-ac96-774b-bcce-b302099a8058"))
        );
    }
}

#[cfg(unix)]
#[test]
fn local_socket_paths_reject_null_bytes_before_binding() {
    use std::os::unix::ffi::OsStringExt as _;

    let error = validate_local_socket_path(&std::path::PathBuf::from(OsString::from_vec(
        b"/tmp/controller\0socket".to_vec(),
    )))
    .expect_err("null bytes cannot be bound");
    assert!(error.to_string().contains("null byte"));
}

#[test]
fn local_ledger_and_bootstrap_inspection_rejects_non_file_entries() {
    let directory = tempfile::tempdir().expect("state test directory");
    let missing = directory.path().join("missing-ledger");
    assert!(!require_existing_ledger(&missing).expect("missing ledger is absent"));

    let ledger = directory.path().join("run-ledger.sqlite3");
    std::fs::write(&ledger, b"sqlite").expect("ledger file");
    assert!(require_existing_ledger(&ledger).expect("regular ledger"));

    for path in [
        directory.path().to_owned(),
        directory.path().join("run-directory"),
    ] {
        if path != directory.path() {
            std::fs::create_dir(&path).expect("run directory");
        }
        assert!(
            require_existing_ledger(&path)
                .expect_err("directories are not ledgers")
                .to_string()
                .contains("not a regular file")
        );
    }

    #[cfg(unix)]
    {
        let target = directory.path().join("target-ledger");
        std::fs::write(&target, b"sqlite").expect("ledger target");
        let symlink = directory.path().join("symlink-ledger");
        std::os::unix::fs::symlink(&target, &symlink).expect("ledger symlink");
        assert!(
            require_existing_ledger(&symlink)
                .expect_err("symlinked ledgers are not regular files")
                .to_string()
                .contains("not a regular file")
        );
        remove_private_bootstrap(&symlink);
        assert!(std::fs::symlink_metadata(&symlink).is_ok());
        remove_private_bootstrap(&directory.path().join("missing-bootstrap"));
        remove_private_bootstrap(directory.path());
        assert!(directory.path().is_dir());
    }

    let bootstrap = directory.path().join("bootstrap.json");
    std::fs::write(&bootstrap, b"{}").expect("bootstrap file");
    remove_private_bootstrap(&bootstrap);
    assert!(!bootstrap.exists());
}
