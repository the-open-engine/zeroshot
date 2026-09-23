use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

#[test]
fn coverage_contract_lease_rejects_unsafe_entries_and_detects_path_replacement() {
    assert!(matches!(
        ControllerLease::acquire(PathBuf::from("/")),
        Err(ControllerLeaseError::StateDirectory)
    ));

    let root = tempfile::tempdir().assert_value();
    let directory = root.path().join("directory");
    std::fs::create_dir(&directory).assert_value();
    assert!(matches!(
        ControllerLease::acquire(&directory),
        Err(ControllerLeaseError::InvalidPath)
    ));

    let target = root.path().join("target");
    std::fs::write(&target, b"target").assert_value();
    let symlink = root.path().join("symlink");
    std::os::unix::fs::symlink(&target, &symlink).assert_value();
    assert!(matches!(
        ControllerLease::acquire(&symlink),
        Err(ControllerLeaseError::InvalidPath)
    ));

    let path = root.path().join("controller.lock");
    let lease = ControllerLease::acquire(&path).assert_value();
    assert_eq!(lease.path(), path);
    assert!(format!("{lease:?}").contains("controller.lock"));
    assert!(lease.is_intact());
    assert!(matches!(
        ControllerLease::acquire(&path).assert_error(),
        ControllerLeaseError::Held
    ));

    std::fs::remove_file(&path).assert_value();
    assert!(!lease.is_intact());
    std::fs::write(&path, b"replacement").assert_value();
    assert!(!lease.is_intact());
}
