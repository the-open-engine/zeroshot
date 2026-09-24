use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

fn blocking_child_command() -> Command {
    #[cfg(unix)]
    let (program, arguments) = ("/bin/sh", ["-c", "read ignored"].as_slice());
    #[cfg(windows)]
    let (program, arguments) = ("cmd.exe", ["/D", "/Q", "/C", "set /p ignored="].as_slice());
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    command
}

#[test]
fn coverage_contract_child_command_is_sanitized_and_preserves_the_validated_specification() {
    let directory = tempfile::tempdir().assert_value();
    let workspace = WorkspaceCapability {
        current_dir: directory.path().to_path_buf(),
        mode: crate::execution::WorkspaceAccessMode::ReadOnly,
    };
    let argv = vec!["first".to_owned(), "two words".to_owned()];
    let environment = BTreeMap::from([
        ("LANG".to_owned(), "C".to_owned()),
        ("PRIVATE_VALUE".to_owned(), "exact".to_owned()),
    ]);
    validate_launch_fields("/bin/echo", &argv, &environment).assert_value();

    let command = build_child_command(
        ChildCommandSpec {
            program: "/bin/echo",
            argv: &argv,
            environment: &environment,
            workspace: &workspace,
        },
        ProcessContainment::ProcessGroup,
    );
    let command = command.as_std();
    assert_eq!(command.get_program(), "/bin/echo");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        ["first", "two words"]
    );
    assert_eq!(command.get_current_dir(), Some(directory.path()));
    let mut effective_environment = environment;
    crate::execution::platform::process_environment(&mut effective_environment);
    assert_eq!(
        command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<BTreeMap<_, _>>(),
        effective_environment
            .iter()
            .map(|(key, value)| {
                (
                    std::ffi::OsString::from(key),
                    Some(std::ffi::OsString::from(value)),
                )
            })
            .collect()
    );
}

#[test]
fn launch_validation_counts_c_storage_and_rejects_each_bound_without_large_allocations() {
    let argv = vec!["a".to_owned(), "β".to_owned()];
    let environment = BTreeMap::from([("K".to_owned(), "vv".to_owned())]);
    validate_launch_fields("p", &argv, &environment).assert_value();
    assert_eq!(format_arg_bytes("p", &argv).assert_value(), 7);
    assert_eq!(total_env_bytes(&environment).assert_value(), 5);
    assert_eq!(c_string_storage_bytes("β"), 3);

    let empty = validate_launch_fields("", &[], &BTreeMap::new())
        .assert_error()
        .to_string();
    assert!(empty.contains("program must not be empty"));

    let limit = CollectionLimit::new("fixture", 2, 4);
    validate_collection(limit, 2, 4).assert_value();
    let items = validate_collection(CollectionLimit::new("fixture", 2, 4), 3, 0)
        .assert_error()
        .to_string();
    assert!(items.contains("fixture has 3 items; maximum is 2"));
    let bytes = validate_collection(CollectionLimit::new("fixture", 2, 4), 0, 5)
        .assert_error()
        .to_string();
    assert!(bytes.contains("fixture is 5 bytes; maximum is 4"));
}

#[tokio::test]
async fn empty_spawn_recovery_state_is_explicitly_inert() {
    let mut recovery = SpawnRecovery::registered();
    assert!(recovery.child_mut().is_none());
    assert!(recovery.disarm().is_none());
    assert!(SpawnRecovery::registered().recover().await.is_none());
    drop(SpawnRecovery::registered());
}

#[tokio::test]
async fn coverage_contract_spawn_recovery_terminates_and_reaps_a_captured_child() {
    let mut command = blocking_child_command();
    let child = command.spawn().assert_value();
    let mut recovery = SpawnRecovery::registered();
    recovery.capture(child);
    assert!(recovery.child_mut().is_some());

    let diagnostic = recovery.recover().await;
    assert!(
        diagnostic.is_none(),
        "unexpected cleanup failure: {diagnostic:?}"
    );
}

#[tokio::test]
async fn coverage_contract_spawn_recovery_uses_registered_tree_cleanup_when_available() {
    let registration =
        platform::register_process_tree_for(ProcessContainment::ProcessGroup).assert_value();
    let mut command = blocking_child_command();
    platform::configure_process(&mut command, ProcessContainment::ProcessGroup);
    let mut child = command.spawn().assert_value();
    let process_tree = platform::capture_process_tree(registration, &mut child).assert_value();
    let mut recovery = SpawnRecovery::registered();
    recovery.capture(child);
    recovery.capture_process_tree(process_tree);

    let diagnostic = recovery.recover().await;
    assert!(
        diagnostic.is_none(),
        "unexpected registered-tree cleanup failure: {diagnostic:?}"
    );
}
