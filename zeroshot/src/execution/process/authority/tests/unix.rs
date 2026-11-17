use super::*;

fn missing_pid() -> u32 {
    let pid = i32::MAX as u32;
    assert_eq!(
        native::process_identity(pid).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    pid
}

#[test]
fn a_live_parent_retains_its_native_creation_identity() {
    let (_, caller) = current();
    let parent = native::process_identity(caller.parent).assert_value();
    assert_eq!(
        parent_or_boundary(caller, caller).assert_value(),
        Some(parent)
    );
}

#[test]
fn a_departed_parent_never_establishes_a_unix_human_boundary() {
    let (_, caller) = current();
    let current = native::ProcessIdentity {
        parent: missing_pid(),
        ..caller
    };
    assert_eq!(
        parent_or_boundary(caller, current).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn a_reused_parent_never_establishes_a_unix_human_boundary() {
    let (_, caller) = current();
    let current = native::ProcessIdentity {
        parent: caller.pid,
        born: caller.born.checked_sub(1).assert_value(),
        ..caller
    };
    assert_eq!(
        parent_or_boundary(caller, current).unwrap_err().to_string(),
        "local steering caller ancestry changed"
    );
}

#[test]
fn an_invalid_parent_error_is_not_a_departed_parent_boundary() {
    let (_, caller) = current();
    let current = native::ProcessIdentity {
        parent: 0,
        ..caller
    };
    assert_eq!(
        parent_or_boundary(caller, current).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[test]
fn an_unretired_group_record_denies_after_its_native_leader_departs() {
    let (root, caller) = current();
    let group = missing_pid();
    let _file = record(root.path(), group, caller.born);
    assert!(owned_group(root.path(), group).assert_value());
}

fn departed_registration() -> (tempfile::TempDir, Registration, u32) {
    let pid = missing_pid();
    let (root, mut registration) = registration(pid, 0);
    registration.publication_lock = Some(lock_publication(root.path()).assert_value());
    FileExt::lock_exclusive(&registration.file).assert_value();
    (root, registration, pid)
}

#[test]
fn capture_uses_known_creation_identity_after_the_native_process_departs() {
    let (root, mut registration, pid) = departed_registration();
    let born = native::process_identity(std::process::id())
        .assert_value()
        .born;
    registration.capture(pid, Some(born)).assert_value();
    assert!(registration.publication_lock.is_none());
    let mut retained = read_record(root.path(), pid).assert_value().assert_value();
    assert_eq!(record_born(&mut retained, pid).assert_value(), born);
    registration.retire().assert_value();
    assert!(read_record(root.path(), pid).assert_value().is_none());
}

#[test]
fn capture_without_a_native_identity_keeps_provisional_ownership_until_retirement() {
    let (root, mut registration, pid) = departed_registration();
    registration.capture(pid, None).assert_value();
    assert!(registration.publication_lock.is_none());
    FileExt::unlock(&registration.file).assert_value();
    let mut retained = read_record(root.path(), pid).assert_value().assert_value();
    assert_eq!(record_born(&mut retained, pid).assert_value(), 0);
    assert!(owned_group(root.path(), pid).assert_value());
    registration.retire().assert_value();
    assert!(!owned_group(root.path(), pid).assert_value());
}
