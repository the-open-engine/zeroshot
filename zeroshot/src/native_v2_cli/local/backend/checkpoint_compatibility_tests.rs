use super::*;
use super::super::local_contract_tests::contract_submission;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection;
use openengine_cluster_protocol::{CheckpointId, RunResumeFrom};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

fn params(from: Option<RunResumeFrom>) -> openengine_cluster_protocol::RunResumeParams {
    openengine_cluster_protocol::RunResumeParams {
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,

        run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000001"),
        successor_run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000002"),
        from,
        connections: Default::default(),
        connection_resolver: None,
        github_token: None,
    }
}

fn selected_checkpoint() -> Option<RunResumeFrom> {
    Some(RunResumeFrom::Checkpoint {
        checkpoint_id: CheckpointId::new("entry-1").assert_value(),
    })
}

fn recovery(root: &TestDirectory, checkpoints: bool) -> LocalRecoveryDocument {
    let mut submission = contract_submission("checkpoint-compatibility");
    submission.runtime.workspace_checkpoints = checkpoints;
    LocalRecoveryDocument {
        submission,
        workspace: root.child("workspace"),
        resumed_from: None,
        delivery_run_id: None,
        successor_run_id: None,
        pending_restore: None,
    }
}

#[tokio::test]
async fn dead_controller_resume_retains_acknowledged_receipts_without_importing_graph_history() {
    use crate::native_v2_contract::NodeRuntimeBinding;
    use crate::native_v2_candidate::test_support::{full_graph, git_delivery_node, success_node};
    use crate::native_v2_supervisor::checkpoints::{DeliveryCheckpointStore, RunCheckpointStore};
    use crate::native_v2_runner::test_support::{delivery_checkpoint, node_started};
    use crate::v2_run_ledger::RunEvent;
    use openengine_cluster_protocol::{DeclaredConnections, NodeName};
    use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId};
    use serde_json::{Value, json};
    for enabled in [false, true] {
        let root = TestDirectory::new("dead-delivery");
        let backend = backend(&root);
        let mut recovery = recovery(&root, enabled);
        recovery.submission.graph = full_graph(vec![git_delivery_node(), success_node()]);
        recovery.submission.runtime.nodes.insert(
            NodeName::new("deliver").assert_value(),
            NodeRuntimeBinding::GitDelivery {
                connections: DeclaredConnections::empty(),
                pull_request_feedback: Default::default(),
            },
        );
        let run_id = params(None).run_id;
        let ledger = super::super::local_contract_tests::create_contract_run(
            &backend,
            &run_id,
            &recovery.submission,
        )
        .await;
        let repository = backend.checkpoint_repository(&run_id).assert_value();
        let store = DeliveryCheckpointStore::new(repository.clone());
        let old = delivery_checkpoint(json!({"receipts":[]}), 1);
        store.retain_delivery_state(&old).await.assert_value();
        let latest = delivery_checkpoint(json!({"owned":"resolved revision"}), 2);
        let reference = ExecutionRef {
            run_id: run_id.clone(),
            node: NodeName::new("deliver").assert_value(),
            node_instance: NodeInstanceId::new(1).assert_value(),
            execution: ExecutionId::new(1).assert_value(),
        };
        ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    node_started(&reference, Value::Null),
                    RunEvent::DeliveryState {
                        execution: reference.execution,
                        state: latest.clone(),
                    },
                ],
            )
            .await
            .assert_value();
        drop(ledger);
        assert_eq!(store.delivery_state().await.assert_value(), Some(old));
        // Opening an observer reconciles a hard process exit without constructing a runtime.
        let observer = PortableRunController::open_observer(
            backend.paths(&run_id).assert_value(),
            run_id.clone(),
        )
        .await
        .assert_value();
        drop(observer);
        for _ in 0..2 {
            backend
                .retain_predecessor_delivery(&run_id, &recovery)
                .await
                .assert_value();
        }
        assert_eq!(
            store.delivery_state().await.assert_value(),
            Some(latest.clone())
        );
        assert_eq!(std::fs::read_dir(repository).assert_value().count(), 1);
        assert!(
            !backend
                .paths(&run_id)
                .assert_value()
                .storage()
                .join("checkpoints/index.json")
                .exists()
        );
        let predecessor = SqliteRunLedger::open(backend.paths(&run_id).assert_value().ledger())
            .assert_value()
            .get(&run_id)
            .await
            .assert_value()
            .assert_value();
        assert!(predecessor.snapshot.execution_seed.is_empty());
        assert!(
            matches!(predecessor.snapshot.terminal, Some(TerminalResult::Failed { reason, .. })
            if reason.as_str() == "runtime_lost")
        );
        // A same-generation disagreement blocks successor preparation and preserves retained bytes.
        store.discard().await.assert_value();
        let conflict = delivery_checkpoint(json!({"conflicting":"receipt"}), 2);
        store.retain_delivery_state(&conflict).await.assert_value();
        assert!(
            backend
                .retain_predecessor_delivery(&run_id, &recovery)
                .await
                .is_err()
        );
        assert_eq!(store.delivery_state().await.assert_value(), Some(conflict));
    }
}

#[test]
fn legacy_resume_uses_the_retained_workspace_but_checkpoint_selection_stays_strict() {
    let root = TestDirectory::new("llr");
    let backend = backend(&root);
    let recovery = recovery(&root, true);

    for from in [None, Some(RunResumeFrom::Restart {})] {
        assert!(
            backend
                .checkpoint_selection(&params(from), &recovery)
                .assert_value()
                .is_none()
        );
    }
    // A selected checkpoint is pinned before launch, so an unavailable one fails without a
    // controller instead of falling back to the retained workspace.
    let error = backend
        .checkpoint_selection(&params(selected_checkpoint()), &recovery)
        .assert_error();
    assert!(
        error
            .to_string()
            .contains("workspace checkpoint is unavailable"),
        "{error}"
    );
}

#[test]
fn interrupted_restore_blocks_a_resume_that_would_not_restore() {
    let root = TestDirectory::new("lir");
    let backend = backend(&root);
    let mut recovery = recovery(&root, true);
    for (selection, hint) in [
        (
            CheckpointRestoreSelection::Latest,
            "resume with plain resume",
        ),
        (
            CheckpointRestoreSelection::Checkpoint {
                checkpoint_id: CheckpointId::new("entry-1").assert_value(),
            },
            "resume with --from-checkpoint entry-1",
        ),
    ] {
        recovery.pending_restore = Some(PendingRestore {
            selection,
            snapshot: serde_json::from_value(serde_json::json!(uuid::Uuid::now_v7().to_string()))
                .assert_value(),
            attempt: RunId::new("0199f33f-3b44-7d21-9000-000000000003"),
        });
        for from in [None, Some(RunResumeFrom::Restart {})] {
            let error = backend
                .checkpoint_selection(&params(from), &recovery)
                .assert_error()
                .to_string();
            assert!(
                error.contains("partially restored") && error.contains(hint),
                "{error}"
            );
        }
    }
}

#[tokio::test]
async fn interrupted_latest_restore_rejects_a_changed_pinned_snapshot() {
    use crate::native_v2_supervisor::checkpoints::{RunCheckpointStore, fake_checkpoint_store};

    let root = TestDirectory::new("restore-pin");
    let backend = backend(&root);
    let mut recovery = recovery(&root, true);
    let workspace = recovery.workspace.clone();
    std::fs::create_dir(&workspace).assert_value();
    let source = workspace.join("source.txt");
    std::fs::write(&source, "original").assert_value();
    let request = params(None);
    let directory = backend
        .paths(&request.run_id)
        .assert_value()
        .storage()
        .join("checkpoints");
    let store = || {
        fake_checkpoint_store(
            root.path(),
            directory.clone(),
            workspace.clone(),
            Default::default(),
        )
    };
    store().finish(&[]).await.assert_value();

    let (restore, pending) = backend
        .checkpoint_selection(&request, &recovery)
        .assert_value()
        .assert_value();
    assert_eq!(restore.selection, CheckpointRestoreSelection::Latest);
    assert_eq!(pending.attempt, request.successor_run_id);
    recovery.pending_restore = Some(pending.clone());
    assert!(
        backend
            .checkpoint_selection(&request, &recovery)
            .assert_value()
            .is_some()
    );

    std::fs::write(&source, "newer workspace").assert_value();
    store().finish(&[]).await.assert_value();
    let error = backend
        .checkpoint_selection(&request, &recovery)
        .assert_error();
    assert!(
        error
            .to_string()
            .contains("no longer resolves to its pinned snapshot"),
        "{error}"
    );
    assert_eq!(
        recovery.pending_restore.as_ref().assert_value().snapshot,
        pending.snapshot
    );
    assert_eq!(
        std::fs::read_to_string(source).assert_value(),
        "newer workspace"
    );
    assert!(
        !backend
            .paths(&request.successor_run_id)
            .assert_value()
            .storage()
            .exists()
    );
}

#[test]
fn disabled_workspace_checkpoints_allow_restart_but_reject_snapshot_restore() {
    let root = TestDirectory::new("checkpoint-policy");
    let backend = backend(&root);
    let recovery = recovery(&root, false);
    for from in [None, Some(RunResumeFrom::Restart {})] {
        assert!(
            backend
                .checkpoint_selection(&params(from), &recovery)
                .assert_value()
                .is_none()
        );
    }
    let error = backend
        .checkpoint_selection(&params(selected_checkpoint()), &recovery)
        .unwrap_err();
    assert!(error.to_string().contains("workspaceCheckpoints is false"));
}

fn backend(root: &TestDirectory) -> LocalCliBackend {
    LocalCliBackend::new(
        root.path().to_owned(),
        PathBuf::from("zeroshot"),
        root.path().to_owned(),
        PathBuf::from("git"),
    )
}
