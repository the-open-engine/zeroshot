use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

fn current() -> (tempfile::TempDir, native::ProcessIdentity) {
    (
        tempfile::tempdir().assert_value(),
        native::process_identity(std::process::id()).assert_value(),
    )
}

fn record(directory: &Path, pid: u32, born: u64) -> File {
    let mut file = platform::private_file(&directory.join(pid.to_string()), FileAccess::CreateNew)
        .assert_value();
    file.write_all(&u64::from(pid).to_le_bytes()).assert_value();
    file.write_all(&born.to_le_bytes()).assert_value();
    file
}

#[test]
fn provisional_ownership_denies_before_capture_and_until_confirmed_retirement() {
    let (root, identity) = current();
    let file = record(root.path(), identity.pid, 0);
    FileExt::lock_exclusive(&file).assert_value();
    assert!(owned(root.path(), identity).assert_value());
    FileExt::unlock(&file).assert_value();
    assert!(owned(root.path(), identity).assert_value());
}

#[test]
fn unheld_complete_ownership_matches_creation_identity_and_rejects_pid_reuse() {
    let (root, identity) = current();
    let mut file = record(root.path(), identity.pid, identity.born);
    assert!(owned(root.path(), identity).assert_value());
    file.seek(SeekFrom::Start(8)).assert_value();
    file.write_all(&identity.born.wrapping_add(1).to_le_bytes())
        .assert_value();
    assert!(!owned(root.path(), identity).assert_value());
}

#[test]
fn an_active_group_lease_denies_even_before_creation_identity_is_known() {
    let (root, identity) = current();
    let file = record(root.path(), identity.pid, identity.born.wrapping_add(1));
    assert!(!owned_group(root.path(), identity.pid).assert_value());
    FileExt::lock_exclusive(&file).assert_value();
    assert!(owned_group(root.path(), identity.pid).assert_value());
    assert!(!owned_group(root.path(), 0).assert_value());
    FileExt::unlock(&file).assert_value();
    assert!(!owned_group(root.path(), identity.pid).assert_value());
}

#[test]
fn an_incomplete_record_never_establishes_human_authority() {
    let root = tempfile::tempdir().assert_value();
    let mut file =
        platform::private_file(&root.path().join("123"), FileAccess::CreateNew).assert_value();
    file.write_all(&[0; 8]).assert_value();
    assert!(read_record(root.path(), 123).is_err());
}

#[test]
fn a_record_with_the_wrong_pid_never_establishes_human_authority() {
    let (root, identity) = current();
    let mut file = record(root.path(), identity.pid, identity.born);
    file.seek(SeekFrom::Start(0)).assert_value();
    file.write_all(&u64::from(identity.pid.wrapping_add(1)).to_le_bytes())
        .assert_value();
    let error = owned(root.path(), identity).unwrap_err();
    assert_eq!(error.to_string(), "invalid local process ownership");
}

fn registration(pid: u32, born: u64) -> (tempfile::TempDir, Registration) {
    let root = tempfile::tempdir().assert_value();
    let file = record(root.path(), pid, born);
    let registration = Registration {
        directory: root.path().to_owned(),
        pending: ".pending".into(),
        file,
        publication_lock: None,
    };
    (root, registration)
}

#[test]
fn retirement_preserves_a_newer_registration_at_the_same_pid() {
    let (root, mut registration) = registration(123, 456);
    std::fs::rename(root.path().join("123"), root.path().join("old")).assert_value();
    let newer = record(root.path(), 123, 789);
    registration.retire().assert_value();
    let mut retained = read_record(root.path(), 123).assert_value().assert_value();
    assert_eq!(record_born(&mut retained, 123).assert_value(), 789);
    assert_eq!(
        platform::file_identity(&newer).assert_value(),
        platform::file_identity(&platform::open_identity(&root.path().join("123")).assert_value())
            .assert_value()
    );
}

#[test]
fn confirmed_retirement_is_idempotent_and_removes_only_its_own_record() {
    let (root, mut registration) = registration(123, 456);
    registration.retire().assert_value();
    registration.retire().assert_value();
    assert!(read_record(root.path(), 123).assert_value().is_none());
}

#[cfg(windows)]
mod windows;

#[cfg(unix)]
mod unix;
