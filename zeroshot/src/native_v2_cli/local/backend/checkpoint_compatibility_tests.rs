use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection;
use openengine_cluster_protocol::{CheckpointId, RunResumeFrom};
use openengine_cluster_testkit::assertions::AssertValue;

fn params(from: Option<RunResumeFrom>) -> openengine_cluster_protocol::RunResumeParams {
    openengine_cluster_protocol::RunResumeParams {
        run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000001"),
        successor_run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000002"),
        from,
        connections: Default::default(),
        connection_resolver: None,
        github_token: None,
    }
}

#[test]
fn legacy_resume_uses_the_retained_workspace_but_checkpoint_selection_stays_strict() {
    let root = TestDirectory::new("llr");
    let backend = LocalCliBackend::new(
        root.path().to_owned(),
        PathBuf::from("zeroshot"),
        root.path().to_owned(),
        PathBuf::from("git"),
    );

    for from in [None, Some(RunResumeFrom::Restart {})] {
        assert!(
            backend
                .checkpoint_selection(&params(from))
                .assert_value()
                .is_none()
        );
    }
    let selected = backend
        .checkpoint_selection(&params(Some(RunResumeFrom::Checkpoint {
            checkpoint_id: CheckpointId::new("entry-1").assert_value(),
        })))
        .assert_value()
        .assert_value();
    assert!(matches!(
        selected.selection,
        CheckpointRestoreSelection::Checkpoint { .. }
    ));
}
