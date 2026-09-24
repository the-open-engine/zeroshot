use std::fs;

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;

#[test]
fn local_path_and_commit_helpers_preserve_atomic_failure_semantics() {
    let root = TestDirectory::new("cli-support");
    let destination = root.child("committed");
    let temporary = root.child("temporary");
    let file = File::create(&temporary).assert_value();
    write_and_commit(
        file,
        b"committed bytes",
        CommitPaths {
            temporary: &temporary,
            destination: &destination,
            parent: root.path(),
        },
    )
    .assert_value();
    assert_eq!(fs::read(&destination).assert_value(), b"committed bytes");

    let unused = root.child("unused");
    let error = write_and_commit(
        File::open(&destination).assert_value(),
        b"must not be written",
        CommitPaths {
            temporary: &destination,
            destination: &unused,
            parent: root.path(),
        },
    )
    .assert_error();
    assert!(matches!(error, NativeV2CliError::Local(_)));
    assert_eq!(fs::read(&destination).assert_value(), b"committed bytes");

    assert_eq!(
        absolute_user_path(root.path(), "invalid").assert_value(),
        root.path()
    );
    assert!(absolute_user_path("relative", "invalid").is_err());
    assert!(absolute_user_path("", "invalid").is_err());
}

#[test]
fn temporary_cleanup_removes_only_failed_writes() {
    let root = TestDirectory::new("cli-cleanup");
    let failed = root.child("failed");
    fs::write(&failed, b"partial").assert_value();
    let result = cleanup_temporary::<()>(
        Err(NativeV2CliError::Local("write failed".to_owned())),
        &failed,
    );
    assert!(result.is_err());
    assert!(!failed.exists());

    let successful = root.child("successful");
    fs::write(&successful, b"complete").assert_value();
    cleanup_temporary(Ok(()), &successful).assert_value();
    assert_eq!(fs::read(successful).assert_value(), b"complete");
}
