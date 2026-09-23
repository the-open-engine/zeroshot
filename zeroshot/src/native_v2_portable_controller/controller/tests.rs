use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::v2_run_ledger::fake::FakeRunLedger;

#[tokio::test]
async fn boundary_contract_single_run_allocator_refuses_foreign_runs_and_confirms_absent_runtime_cleanup()
 {
    let root = tempfile::tempdir().assert_value();
    let run_id = RunId::new("run-single-allocator");
    let foreign = RunId::new("run-foreign");
    let lease =
        Arc::new(ControllerLease::acquire(root.path().join("controller.lock")).assert_value());
    let allocator = SingleRunAllocator::new(run_id.clone(), None, lease);

    assert!(allocator.require_run(&run_id).is_ok());
    assert!(allocator.require_run(&foreign).is_err());
    assert!(allocator.claim_controller(&foreign).await.is_err());
    let claim = allocator.claim_controller(&run_id).await.assert_value();
    drop(claim);
    assert!(
        allocator
            .destroy_or_confirm_absent(&foreign, RunRuntimeExit::Failed)
            .await
            .is_err()
    );
    allocator
        .destroy_or_confirm_absent(&run_id, RunRuntimeExit::Completed)
        .await
        .assert_value();

    let mut loss = allocator.loss_receiver.clone();
    allocator.loss_sender().send_replace(true);
    loss.changed().await.assert_value();
    assert!(*loss.borrow());
}

#[test]
fn boundary_contract_workspace_identity_detects_replacement_and_rejects_non_directories() {
    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let identity = WorkspaceIdentity::capture(&workspace).assert_value();
    assert!(identity.is_current(&workspace));

    let original = root.path().join("original");
    std::fs::rename(&workspace, &original).assert_value();
    std::fs::create_dir(&workspace).assert_value();
    assert!(!identity.is_current(&workspace));
    assert!(!identity.is_current(&root.path().join("absent")));

    let file = root.path().join("file");
    std::fs::write(&file, b"not a workspace").assert_value();
    assert!(matches!(
        WorkspaceIdentity::capture(&file),
        Err(PortableControllerError::Workspace)
    ));
}

#[tokio::test]
async fn boundary_contract_empty_ledger_has_no_existing_run_and_storage_rejects_unsafe_ledger_entries()
 {
    let run_id = RunId::new("run-empty-ledger");
    let ledger = FakeRunLedger::new();
    assert!(!validate_existing_run(&ledger, &run_id).await.assert_value());

    assert!(matches!(
        open_controller_storage(Path::new("relative")),
        Err(PortableControllerError::Path)
    ));

    let root = tempfile::tempdir().assert_value();
    let unsafe_storage = root.path().join("unsafe-state");
    std::fs::create_dir(&unsafe_storage).assert_value();
    std::fs::create_dir(unsafe_storage.join("runs.sqlite3")).assert_value();
    assert!(matches!(
        open_controller_storage(&unsafe_storage),
        Err(PortableControllerError::LedgerPath)
    ));

    let storage = root.path().join("state");
    let (paths, lease, ledger) = open_controller_storage(&storage).assert_value();
    assert_eq!(paths.storage(), storage);
    assert!(lease.is_intact());
    drop((ledger, lease));
}

#[tokio::test]
async fn coverage_contract_observer_refuses_storage_without_the_exact_durable_run() {
    let root = tempfile::tempdir().assert_value();
    let storage = root.path().join("state");
    let (paths, lease, ledger) = open_controller_storage(&storage).assert_value();
    drop((ledger, lease));

    let result = PortableRunController::open_observer(paths, RunId::new("missing-run")).await;
    assert!(matches!(
        result,
        Err(PortableControllerError::DurableIdentity)
    ));
}

#[test]
fn coverage_contract_workspace_loss_requires_all_three_sources_of_positive_evidence() {
    for identity in [false, true] {
        for workspace_lease in [false, true] {
            for controller_lease in [false, true] {
                assert_eq!(
                    workspace_is_lost(identity, workspace_lease, controller_lease),
                    !(identity && workspace_lease && controller_lease)
                );
            }
        }
    }
}
