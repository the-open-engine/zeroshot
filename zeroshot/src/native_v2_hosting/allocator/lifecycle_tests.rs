use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;

fn recovery_document(run_id: &RunId) -> HostedRecoveryDocument {
    HostedRecoveryDocument {
        recoverable: true,
        run_id: Some(run_id.clone()),
        delivery_run_id: Some(run_id.clone()),
        resumed_from: None,
        successor_run_id: None,
    }
}

fn retained_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("workspace")).assert_value();
    std::fs::create_dir_all(root.join("runtime")).assert_value();
    std::fs::write(root.join("workspace/candidate"), b"retained").assert_value();
    std::fs::write(root.join("runtime/private"), b"discard").assert_value();
}

fn cleanup_request<'a>(
    paths: (&'a Path, &'a Path),
    identities: (&'a RunId, &'a RunId),
    recovery_eligible: bool,
    exit: RunRuntimeExit,
) -> CleanupRunRequest<'a> {
    CleanupRunRequest {
        run_root: paths.0,
        recovery_path: paths.1,
        run_id: identities.0,
        delivery_run_id: identities.1,
        recovery_eligible,
        exit,
    }
}

#[test]
fn hosting_source_contract_cleanup_retains_only_failed_candidate_workspaces() {
    let root = TestDirectory::new("host-cleanup-decisions");
    let run_id = RunId::new("cleanup-run");
    let delivery_run_id = RunId::new("delivery-run");
    let run_root = root.child("run");
    let recovery = root.child("recovery.json");
    let paths = (run_root.as_path(), recovery.as_path());
    let identities = (&run_id, &delivery_run_id);

    assert!(matches!(
        failed_run_directory_state(&run_root).assert_value(),
        FailedRunDirectoryState::Absent
    ));
    cleanup_run_directory(cleanup_request(
        paths,
        identities,
        true,
        RunRuntimeExit::Failed,
    ))
    .assert_value();
    assert!(!confirmed_retained_workspace(&run_root, &recovery, &run_id).assert_value());

    std::fs::create_dir(&run_root).assert_value();
    std::fs::write(run_root.join("bootstrap"), b"disposable").assert_value();
    assert!(matches!(
        failed_run_directory_state(&run_root).assert_value(),
        FailedRunDirectoryState::Disposable
    ));
    cleanup_run_directory(cleanup_request(
        paths,
        identities,
        true,
        RunRuntimeExit::RuntimeLost,
    ))
    .assert_value();
    assert!(!run_root.exists());

    retained_workspace(&run_root);
    assert!(matches!(
        failed_run_directory_state(&run_root).assert_value(),
        FailedRunDirectoryState::Retained
    ));
    cleanup_run_directory(cleanup_request(
        paths,
        identities,
        true,
        RunRuntimeExit::Failed,
    ))
    .assert_value();
    assert!(run_root.join("workspace/candidate").is_file());
    assert!(!run_root.join("runtime").exists());
    let document = read_recovery(&recovery).assert_value();
    assert!(document.recoverable);
    assert_eq!(document.run_id, Some(run_id.clone()));
    assert_eq!(document.delivery_run_id, Some(delivery_run_id.clone()));
    assert!(confirmed_retained_workspace(&run_root, &recovery, &run_id).assert_value());

    cleanup_run_directory(cleanup_request(
        paths,
        identities,
        false,
        RunRuntimeExit::Failed,
    ))
    .assert_value();
    assert!(!run_root.exists());
    std::fs::write(&run_root, b"not a directory").assert_value();
    assert!(failed_run_directory_state(&run_root).is_err());
    assert!(remove_run_directory(&run_root).is_err());
}

#[test]
fn hosting_source_contract_interrupted_handoffs_reconcile_exact_lineage() {
    let root = TestDirectory::new("host-handoff-reconcile");
    reconcile_retained_allocations(root.path()).assert_value();

    let source_id = RunId::new("source-run");
    let successor_id = RunId::new("successor-run");
    let source_root = run_directory(root.path(), &source_id);
    let source_path = recovery_path(root.path(), &source_id);
    let successor_path = recovery_path(root.path(), &successor_id);
    retained_workspace(&source_root);
    let mut source = recovery_document(&source_id);
    source.successor_run_id = Some(successor_id.clone());
    write_recovery(&source_path, &source).assert_value();
    write_recovery(&successor_path, &recovery_document(&successor_id)).assert_value();

    reconcile_retained_document(root.path(), &source_path).assert_value();
    let rolled_back = read_recovery(&source_path).assert_value();
    assert!(rolled_back.recoverable);
    assert_eq!(rolled_back.successor_run_id, None);
    assert!(!successor_path.exists());

    std::fs::remove_dir_all(&source_root).assert_value();
    let delivery_id = RunId::new("delivery-root");
    source.delivery_run_id = Some(delivery_id.clone());
    source.successor_run_id = Some(successor_id.clone());
    write_recovery(&source_path, &source).assert_value();
    std::fs::create_dir_all(run_directory(root.path(), &successor_id)).assert_value();
    reconcile_retained_document(root.path(), &source_path).assert_value();
    let completed = read_recovery(&successor_path).assert_value();
    assert_eq!(completed.run_id, Some(successor_id.clone()));
    assert_eq!(completed.delivery_run_id, Some(delivery_id));
    assert_eq!(completed.resumed_from, Some(source_id.clone()));

    let mut mismatched = completed;
    mismatched.resumed_from = Some(RunId::new("another-source"));
    write_recovery(&successor_path, &mismatched).assert_value();
    assert!(reconcile_retained_document(root.path(), &source_path).is_err());

    std::fs::create_dir_all(&source_root).assert_value();
    assert!(reconcile_retained_document(root.path(), &source_path).is_err());
}

#[test]
fn hosting_source_contract_recovery_lineage_and_private_paths_are_bounded() {
    let root = TestDirectory::new("host-recovery-lineage");
    let original = RunId::new("original-run");
    let successor = RunId::new("successor-run");
    let delivery = RunId::new("delivery-run");
    let original_path = recovery_path(root.path(), &original);
    let mut original_document = recovery_document(&original);
    original_document.delivery_run_id = Some(delivery.clone());
    write_recovery(&original_path, &original_document).assert_value();
    let mut successor_document = recovery_document(&successor);
    successor_document.delivery_run_id = None;
    successor_document.resumed_from = Some(original.clone());
    assert_eq!(
        retained_delivery_run_id(root.path(), &successor, &successor_document),
        Some(delivery)
    );

    let missing = RunId::new("missing-predecessor");
    successor_document.resumed_from = Some(missing.clone());
    assert_eq!(
        retained_delivery_run_id(root.path(), &successor, &successor_document),
        Some(missing)
    );
    successor_document.resumed_from = Some(successor.clone());
    write_recovery(
        recovery_path(root.path(), &successor).as_path(),
        &successor_document,
    )
    .assert_value();
    assert_eq!(
        retained_delivery_run_id(root.path(), &successor, &successor_document),
        None
    );

    assert_eq!(
        run_directory(root.path(), &original),
        run_directory(root.path(), &original)
    );
    assert_ne!(
        run_directory(root.path(), &original),
        run_directory(root.path(), &successor)
    );
    assert_ne!(
        run_directory(root.path(), &original),
        controller_lock_path(root.path(), &original)
    );
    let disposable = root.child("disposable");
    std::fs::create_dir(&disposable).assert_value();
    remove_run_directory(&disposable).assert_value();
    remove_run_directory(&disposable).assert_value();
}
