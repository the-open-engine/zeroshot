use super::*;

#[test]
fn a_departed_parent_can_end_an_uncontained_human_callers_ancestry() {
    let (_root, caller) = current();
    let outside = native::outside_jobs(caller).assert_value();
    let departed = native::ProcessIdentity {
        parent: u32::MAX,
        ..caller
    };
    let boundary = parent_or_boundary(caller, departed);
    if outside {
        assert!(boundary.assert_value().is_none());
    } else {
        assert_eq!(boundary.unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}

#[test]
fn a_reused_parent_can_end_only_the_original_uncontained_callers_ancestry() {
    let (_root, caller) = current();
    let reused = native::ProcessIdentity {
        parent: caller.pid,
        born: caller.born.checked_sub(1).assert_value(),
        ..caller
    };
    let boundary = parent_or_boundary(caller, reused);
    if native::outside_jobs(caller).assert_value() {
        assert!(boundary.assert_value().is_none());
    } else {
        assert!(boundary.is_err());
    }
    let stale = native::ProcessIdentity {
        born: caller.born.wrapping_add(1),
        ..caller
    };
    assert!(parent_or_boundary(stale, reused).is_err());
    let missing_caller = native::ProcessIdentity {
        pid: u32::MAX,
        ..caller
    };
    assert!(native::outside_jobs(missing_caller).is_err());
}

#[test]
fn an_invalid_parent_is_not_a_departure_boundary() {
    let (_root, caller) = current();
    let invalid = native::ProcessIdentity {
        parent: 0,
        ..caller
    };
    assert_eq!(
        parent_or_boundary(caller, invalid).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[tokio::test]
async fn owned_job_members_cannot_use_departed_or_reused_parents_as_a_human_boundary() {
    use crate::execution::process::platform::{
        ProcessCleanupEvidence, ProcessContainment, capture_process_tree, configure_process,
        prepare_supervisor, register_process_tree_for, terminate_process_tree,
    };
    let root = tempfile::tempdir().assert_value();
    let program = PathBuf::from(std::env::var_os("SystemRoot").assert_value())
        .join("System32")
        .join("ping.exe");
    let mut command = tokio::process::Command::new(program);
    command
        .args(["-n", "31", "127.0.0.1"])
        .current_dir(root.path())
        .kill_on_drop(true);
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut registration =
        register_process_tree_for(ProcessContainment::ProcessGroup).assert_value();
    configure_process(&mut command, ProcessContainment::ProcessGroup);
    prepare_supervisor(&mut command, &mut registration, root.path()).assert_value();
    let mut child = command.spawn().assert_value();
    let caller = native::process_identity(child.id().assert_value()).assert_value();
    let tree = capture_process_tree(registration, &mut child).assert_value();
    assert!(!native::outside_jobs(caller).assert_value());
    let departed = native::ProcessIdentity {
        parent: u32::MAX,
        ..caller
    };
    assert_eq!(
        parent_or_boundary(caller, departed).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    let reused = native::ProcessIdentity {
        parent: caller.pid,
        born: caller.born.checked_sub(1).assert_value(),
        ..caller
    };
    assert!(parent_or_boundary(caller, reused).is_err());
    assert_eq!(
        require_human(caller.pid).unwrap_err().to_string(),
        MANDATE_REQUIRED
    );
    let cleanup = terminate_process_tree(&tree, &mut child).await;
    assert_eq!(cleanup.cleanup, ProcessCleanupEvidence::Reaped);
}

mod departed;
