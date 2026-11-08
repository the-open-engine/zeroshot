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
    recovery.pending_restore = Some(PendingRestore {
        selection: CheckpointRestoreSelection::Checkpoint {
            checkpoint_id: CheckpointId::new("entry-1").assert_value(),
        },
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
            error.contains("partially restored")
                && error.contains("resume with --from-checkpoint entry-1"),
            "{error}"
        );
    }
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
