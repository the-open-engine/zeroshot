#[cfg(unix)]
use openengine_cluster_testkit::assertions::AssertError;
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[cfg(unix)]
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

fn workspace_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let root = tempfile::tempdir().assert_value();
    let leases = root.path().join("leases");
    let parent = root.path().join("work");
    let child = parent.join("child");
    (root, leases, parent, child)
}

#[test]
fn overlapping_workspaces_are_excluded_in_either_order_and_siblings_remain_independent() {
    let (_root, leases, parent, child) = workspace_fixture();
    let sibling = parent.join("sibling");
    let claim = |workspace: &Path| {
        ControllerLease::acquire_workspace(&workspace_lease_path(&leases, workspace), workspace)
    };
    for (first, second) in [(&parent, &child), (&child, &parent)] {
        let owned = claim(first).assert_value();
        assert!(matches!(claim(second), Err(ControllerLeaseError::Held)));
        assert!(owned.is_intact());
        drop(owned);
        assert!(claim(second).assert_value().is_intact());
    }
    let child_lease = claim(&child).assert_value();
    let sibling_lease = claim(&sibling).assert_value();
    assert!(child_lease.is_intact());
    assert!(sibling_lease.is_intact());
    assert!(matches!(claim(&child), Err(ControllerLeaseError::Held)));
    assert!(matches!(claim(&parent), Err(ControllerLeaseError::Held)));
}

#[cfg(unix)]
struct ForkedLeaseChild(libc::pid_t);

#[cfg(unix)]
impl Drop for ForkedLeaseChild {
    fn drop(&mut self) {
        // SAFETY: this is our unreaped child, which only waits for a signal.
        unsafe { libc::kill(self.0, libc::SIGKILL) };
        loop {
            // SAFETY: waitpid writes no status and reaps only this owned child.
            let result = unsafe { libc::waitpid(self.0, std::ptr::null_mut(), 0) };
            if result >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn released_workspace_locks_do_not_survive_in_a_forked_child() {
    let (_root, leases, parent, child) = workspace_fixture();
    let claim = |workspace: &Path| {
        ControllerLease::acquire_workspace(&workspace_lease_path(&leases, workspace), workspace)
    };
    let owned = claim(&child).assert_value();
    // SAFETY: the child executes only async-signal-safe pause calls until its guard kills it.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed: {}", io::Error::last_os_error());
    if pid == 0 {
        loop {
            // SAFETY: pause neither allocates nor runs inherited Rust destructors.
            unsafe { libc::pause() };
        }
    }
    let _child = ForkedLeaseChild(pid);
    assert!(matches!(claim(&parent), Err(ControllerLeaseError::Held)));
    drop(owned);
    assert!(claim(&child).assert_value().is_intact());
    assert!(claim(&parent).assert_value().is_intact());
}

#[test]
fn replacing_an_ancestor_lock_invalidates_the_workspace_lease() {
    let (_root, leases, parent, child) = workspace_fixture();
    let lease = ControllerLease::acquire_workspace(&workspace_lease_path(&leases, &child), &child)
        .assert_value();
    let ancestor_lock = workspace_lease_path(&leases, &parent);
    std::fs::remove_file(&ancestor_lock).assert_value();
    std::fs::write(&ancestor_lock, b"replacement").assert_value();
    assert!(!lease.is_intact());
}
