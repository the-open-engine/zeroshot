mod workspace_storage;

use std::sync::Arc;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::process::CommandExt as _;

use super::*;
use openengine_cluster_protocol::{CheckpointId, NodeName, RunCheckpointsParams, RunCheckpointsResult};
use crate::full_v1_reducer::ExecutionBoundary;
use crate::execution::process::HostedProcessPool;
use crate::native_v2_capsule::CapsuleFilesystem;
use crate::native_v2_cloud::{
    AllocatedCapsule, CapsuleAllocationUnavailable, RetainedAllocationRequest,
    RetainedAllocationUnavailable,
};
use crate::native_v2_delivery::git_auth::encode_basic_credential;
use crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection;
use crate::native_v2_target_authority::OperatorDiagnosticStore;
use super::super::allocator::{
    HostedRecoveryDocument, checkpoint_directory, checkpoint_repository, recovery_path,
    write_recovery,
};

const CHECKOUT_TOKEN: &str = "checkout-secret";

struct CheckoutFixture {
    _repository: RepositoryFixture,
    root: TestDirectory,
    allocator: ProductionCapsuleAllocator,
    admitted: native_v2_contract::AdmittedRun,
    diagnostics: Arc<OperatorDiagnosticStore>,
    run_id: RunId,
}

impl CheckoutFixture {
    async fn new(body: &str) -> Self {
        Self::with_workspace_storage(body, None).await
    }

    async fn with_workspace_storage(
        body: &str,
        storage: Option<Arc<dyn HostedWorkspaceStorage>>,
    ) -> Self {
        let repository = RepositoryFixture::new();
        let root = TestDirectory::new("checkout-recovery");
        prepare_storage_root(&root.path().to_owned()).assert_value();
        let script = format!(
            r#"#!/bin/sh
set -eu
previous=
workspace=/
for argument do
  if [ "$previous" = -C ]; then workspace=$argument; fi
  previous=$argument
done
case " $* " in
  *" fetch "*) /usr/bin/printf x >> "$0.attempts" ;;
esac
{body}
exec /usr/bin/git "$@"
"#
        );
        let program = root.write_executable("git-script", &script);
        let attempts = root.write("git-script.attempts", "");
        fs::set_permissions(attempts, fs::Permissions::from_mode(0o666)).assert_value();
        let external = root.child("git-script.external");
        fs::create_dir(&external).assert_value();
        fs::write(external.join("keep"), "outside checkout").assert_value();
        let mut config = capsule_config(root.path().to_owned());
        config.git_program = program;
        config.workspace_storage = storage;
        let diagnostics = config.operator_diagnostics.clone();
        let allocator = ProductionCapsuleAllocator::new(config)
            .assert_value()
            .with_test_filesystem_and_source(repository.remote.clone(), portable_filesystem);
        let admitted = admit(submission(
            runtime(BTreeSet::new()),
            &repository.main_revision,
            "checkout-recovery",
        ))
        .await;
        Self {
            _repository: repository,
            root,
            allocator,
            admitted,
            diagnostics,
            run_id: RunId::new("checkout-recovery"),
        }
    }

    async fn allocate(&self) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        self.allocator
            .allocate(&self.run_id, &self.admitted, Some(CHECKOUT_TOKEN))
            .await
    }

    async fn retain_untracked_workspace(&self) -> PathBuf {
        let capsule = self.allocate().await.assert_value();
        let workspace = self.allocator.run_path(&self.run_id).join("workspace");
        fs::write(workspace.join("untracked.txt"), "resume me\n").assert_value();
        finish_checkpoints(&capsule).await;
        capsule
            .cleanup
            .destroy_or_confirm_absent(RunRuntimeExit::Failed)
            .await
            .assert_value();
        workspace
    }

    async fn allocate_retained(
        &self,
        successor: &RunId,
    ) -> Result<AllocatedCapsule, RetainedAllocationUnavailable> {
        self.allocator
            .allocate_from_retained(RetainedAllocationRequest {
                selection: CheckpointRestoreSelection::Latest,
                source_run_id: &self.run_id,
                run_id: successor,
                admitted: &self.admitted,
                github_token: Some(CHECKOUT_TOKEN),
            })
            .await
    }

    async fn checkpoints(&self, run_id: &RunId) -> RunCheckpointsResult {
        self.allocator
            .checkpoints(RunCheckpointsParams {
                run_id: run_id.clone(),
                after: None,
                limit: None,
            })
            .await
            .assert_value()
    }

    async fn assert_claimed_by(&self, successor: &RunId) {
        let source_recovery = self.allocator.workspace_recovery(&self.run_id).await;
        assert!(!source_recovery.recoverable);
        assert_eq!(source_recovery.successor_run_id.as_ref(), Some(successor));
    }
}

#[tokio::test]
async fn transient_fetch_failure_recovers_in_the_same_allocation_at_the_exact_revision() {
    let fixture = CheckoutFixture::new(
        r#"case " $* " in
  *" fetch "*)
    if [ "$(/usr/bin/cat "$0.attempts")" = x ]; then
      /usr/bin/git "$@"
      /usr/bin/mkdir "$workspace/partial"
      /usr/bin/ln -s "$0.external" "$workspace/partial/external"
      /usr/bin/printf stale > "$workspace/.git/index.lock"
      /usr/bin/printf 'temporary remote disconnect\n' >&2
      exit 42
    fi ;;
esac"#,
    )
    .await;
    let capsule = fixture.allocate().await.assert_value();
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    let head = std::process::Command::new("/usr/bin/git")
        .arg("-C")
        .arg(&workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .assert_value();
    assert!(head.status.success());
    assert_eq!(
        String::from_utf8(head.stdout).assert_value().trim(),
        fixture.admitted.source.revision.as_str()
    );
    assert_eq!(fixture.root.read("git-script.attempts"), "xx");
    assert!(!workspace.join("partial").exists());
    assert!(!workspace.join(".git/index.lock").exists());
    assert_eq!(
        fixture.root.read("git-script.external/keep"),
        "outside checkout"
    );
    assert!(
        fixture
            .diagnostics
            .snapshot(&fixture.run_id)
            .diagnostics
            .is_empty()
    );
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
}

#[tokio::test]
async fn exhausted_checkout_preserves_redacted_operator_diagnostics() {
    let fixture = CheckoutFixture::new(
        r#"case " $* " in
  *" fetch "*)
    /usr/bin/printf 'upstream status body\n'
    /usr/bin/printf '%05000d\n' 0
    /usr/bin/printf 'unfamiliar fetch failure: %s\n%s\n' "$GH_TOKEN" "$*" >&2
    exit 42 ;;
esac"#,
    )
    .await;
    assert!(matches!(
        fixture.allocate().await,
        Err(CapsuleAllocationUnavailable::SourceCheckout)
    ));
    assert_eq!(fixture.root.read("git-script.attempts"), "xxx");
    let snapshot = fixture.diagnostics.snapshot(&fixture.run_id);
    assert_eq!(snapshot.diagnostics.len(), 1);
    let diagnostic = &snapshot.diagnostics[0];
    assert_eq!(diagnostic.operation, "source.checkout");
    assert_eq!(diagnostic.exit_status, Some(42));
    assert!(diagnostic.stdout.starts_with("upstream status body\n"));
    assert!(diagnostic.stdout_truncated);
    assert!(!diagnostic.stderr.contains("upstream status body"));
    assert!(
        diagnostic
            .stderr
            .contains("unfamiliar fetch failure: [REDACTED]")
    );
    assert!(diagnostic.stderr.contains("fetch"));
    assert!(!diagnostic.stderr.contains(CHECKOUT_TOKEN));
    assert!(
        !diagnostic
            .stderr
            .contains(&encode_basic_credential(CHECKOUT_TOKEN))
    );
    assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
    assert_eq!(
        fixture.root.read("git-script.external/keep"),
        "outside checkout"
    );
    assert!(matches!(
        fixture.allocate().await,
        Err(CapsuleAllocationUnavailable::Runtime)
    ));
}

#[tokio::test]
async fn wrong_checkout_revision_never_exposes_a_runner() {
    let fixture = CheckoutFixture::new(
        r#"case " $* " in
  *" rev-parse HEAD "*)
    /usr/bin/printf '0000000000000000000000000000000000000000\n'
    exit 0 ;;
esac"#,
    )
    .await;
    assert!(matches!(
        fixture.allocate().await,
        Err(CapsuleAllocationUnavailable::SourceCheckout)
    ));
    assert_eq!(fixture.root.read("git-script.attempts"), "x");
    let diagnostics = fixture.diagnostics.snapshot(&fixture.run_id).diagnostics;
    assert!(diagnostics[0].stderr.contains("checkout revision mismatch"));
    assert!(
        diagnostics[0]
            .stderr
            .contains(fixture.admitted.source.revision.as_str())
    );
    assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
}

#[tokio::test]
async fn failed_capsule_retains_workspace_until_discarded() {
    let fixture = CheckoutFixture::new("").await;
    let capsule = fixture.allocate().await.assert_value();
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    fs::write(workspace.join("untracked.txt"), "retained\n").assert_value();

    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();

    let recovery = fixture.allocator.workspace_recovery(&fixture.run_id).await;
    assert!(recovery.recoverable);
    assert_eq!(
        fs::read_to_string(workspace.join("untracked.txt")).assert_value(),
        "retained\n"
    );
    assert!(
        !fixture
            .allocator
            .run_path(&fixture.run_id)
            .join("runtime")
            .exists()
    );
    assert!(
        fixture
            .allocator
            .discard_workspace(&fixture.run_id)
            .await
            .assert_value()
    );
    assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
}

#[tokio::test]
async fn runtime_loss_retains_workspace_after_confirmed_cleanup() {
    let fixture = CheckoutFixture::new("").await;
    let capsule = fixture.allocate().await.assert_value();
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    fs::write(workspace.join("runtime-lost.txt"), "retained\n").assert_value();

    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::RuntimeLost)
        .await
        .assert_value();

    assert!(
        fixture
            .allocator
            .workspace_recovery(&fixture.run_id)
            .await
            .recoverable
    );
    assert_eq!(
        fs::read_to_string(workspace.join("runtime-lost.txt")).assert_value(),
        "retained\n"
    );
}

#[tokio::test]
async fn reconstructed_failed_runs_retain_workspace_without_active_capsules() {
    let fixture = CheckoutFixture::new("").await;
    for (suffix, exit) in [
        ("failed", RunRuntimeExit::Failed),
        ("runtime-lost", RunRuntimeExit::RuntimeLost),
    ] {
        let run_id = RunId::new(format!("checkout-reconstructed-{suffix}"));
        let run_root = fixture.allocator.run_path(&run_id);
        let workspace = run_root.join("workspace");
        let git_metadata = workspace.join(".git");
        fs::create_dir_all(&git_metadata).assert_value();
        fs::write(workspace.join("retained.txt"), "survived restart\n").assert_value();
        fs::write(git_metadata.join("HEAD"), "ref: refs/heads/main\n").assert_value();
        fs::create_dir_all(run_root.join("runtime")).assert_value();
        fs::write(run_root.join("runtime/private"), "remove me\n").assert_value();
        let staging = checkpoint_directory(fixture.root.path(), &run_id).join("staging");
        fs::create_dir_all(staging.join("incomplete")).assert_value();
        fs::write(staging.join("incomplete/workspace"), "temporary full copy").assert_value();

        fixture
            .allocator
            .destroy_or_confirm_absent(&run_id, exit)
            .await
            .assert_value();

        assert!(
            fixture
                .allocator
                .workspace_recovery(&run_id)
                .await
                .recoverable
        );
        assert_eq!(
            fs::read_to_string(workspace.join("retained.txt")).assert_value(),
            "survived restart\n"
        );
        assert_eq!(
            fs::read_to_string(git_metadata.join("HEAD")).assert_value(),
            "ref: refs/heads/main\n"
        );
        assert!(!run_root.join("runtime").exists());
        assert!(!staging.exists());
    }
}

#[tokio::test]
async fn reconstructed_completed_successor_deletes_the_lineage_repository() {
    let fixture = CheckoutFixture::new("").await;
    let successor = RunId::new("checkout-reconstructed-completed-successor");
    let run_root = fixture.allocator.run_path(&successor);
    fs::create_dir_all(&run_root).assert_value();
    let repository = checkpoint_repository(fixture.root.path(), &fixture.run_id);
    fs::create_dir_all(repository.join("repository/data")).assert_value();
    fs::write(repository.join("repository/config"), "restic").assert_value();
    write_recovery(
        &recovery_path(fixture.root.path(), &successor),
        &HostedRecoveryDocument {
            recoverable: false,
            run_id: Some(successor.clone()),
            delivery_run_id: Some(fixture.run_id.clone()),
            resumed_from: Some(fixture.run_id.clone()),
            successor_run_id: None,
        },
    )
    .assert_value();

    fixture
        .allocator
        .destroy_or_confirm_absent(&successor, RunRuntimeExit::Completed)
        .await
        .assert_value();

    assert!(!run_root.exists());
    assert!(!repository.exists());
}

#[tokio::test]
async fn failed_retained_allocation_commits_recovery_to_successor() {
    let mut fixture = CheckoutFixture::new("").await;
    fixture.retain_untracked_workspace().await;
    fixture
        .allocator
        .set_test_filesystem(|_, _, _| Err(CapsuleAllocationUnavailable::Runtime));

    let successor = RunId::new("checkout-recovery-failed-successor");
    assert!(matches!(
        fixture.allocate_retained(&successor).await,
        Err(RetainedAllocationUnavailable::Settled(
            CapsuleAllocationUnavailable::Runtime
        ))
    ));

    fixture.assert_claimed_by(&successor).await;
    let successor_recovery = fixture.allocator.workspace_recovery(&successor).await;
    assert!(successor_recovery.recoverable);
    assert_eq!(
        successor_recovery.resumed_from.as_ref(),
        Some(&fixture.run_id)
    );
    let successor_workspace = fixture.allocator.run_path(&successor).join("workspace");
    assert_eq!(
        fs::read_to_string(successor_workspace.join("untracked.txt")).assert_value(),
        "resume me\n"
    );
    assert!(
        !fixture
            .allocator
            .run_path(&successor)
            .join("runtime")
            .exists()
    );
}

#[tokio::test]
async fn unconfirmed_retained_cleanup_keeps_successor_for_reconciliation() {
    let mut fixture = CheckoutFixture::new("").await;
    fixture.retain_untracked_workspace().await;
    fixture
        .allocator
        .set_test_filesystem(block_retained_cleanup);

    let successor = RunId::new("checkout-recovery-unconfirmed-successor");
    assert!(matches!(
        fixture.allocate_retained(&successor).await,
        Err(RetainedAllocationUnavailable::CleanupUnconfirmed(_))
    ));

    fixture.assert_claimed_by(&successor).await;
    let successor_root = fixture.allocator.run_path(&successor);
    assert_eq!(
        fs::read_to_string(successor_root.join("workspace/untracked.txt")).assert_value(),
        "resume me\n"
    );
    assert!(successor_root.join("runtime").is_file());

    fs::remove_file(successor_root.join("runtime")).assert_value();
    fixture.allocator.reconcile_test_recovery();
    fixture
        .allocator
        .destroy_or_confirm_absent(&successor, RunRuntimeExit::RuntimeLost)
        .await
        .assert_value();
    let successor_recovery = fixture.allocator.workspace_recovery(&successor).await;
    assert!(successor_recovery.recoverable);
    assert_eq!(
        successor_recovery.resumed_from.as_ref(),
        Some(&fixture.run_id)
    );
}

#[tokio::test]
async fn retained_capsule_moves_exclusively_to_successor_and_keeps_lineage() {
    let fixture = CheckoutFixture::new("").await;
    fixture.retain_untracked_workspace().await;

    let successor = RunId::new("checkout-recovery-successor");
    let resumed = fixture.allocate_retained(&successor).await.assert_value();
    let successor_workspace = fixture.allocator.run_path(&successor).join("workspace");
    assert_eq!(
        fs::read_to_string(successor_workspace.join("untracked.txt")).assert_value(),
        "resume me\n"
    );
    let source_recovery = fixture.allocator.workspace_recovery(&fixture.run_id).await;
    assert!(!source_recovery.recoverable);
    assert_eq!(source_recovery.successor_run_id.as_ref(), Some(&successor));
    assert_eq!(
        fixture
            .allocator
            .workspace_recovery(&successor)
            .await
            .resumed_from
            .as_ref(),
        Some(&fixture.run_id)
    );

    resumed
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
    assert!(!fixture.allocator.run_path(&successor).exists());
    assert_eq!(
        fixture
            .allocator
            .workspace_recovery(&successor)
            .await
            .resumed_from
            .as_ref(),
        Some(&fixture.run_id)
    );
}

async fn capture_checkpoint(capsule: &AllocatedCapsule) {
    let boundary = ExecutionBoundary {
        node: NodeName::new("work").assert_value(),
        map_indices: Vec::new(),
        loop_iterations: Vec::new(),
        attempt: 1,
    };
    let captured = capsule
        .checkpoints
        .as_ref()
        .assert_value()
        .enter(&boundary, &[])
        .await;
    assert!(captured.is_ok(), "snapshot capture failed: {captured:?}");
}

async fn finish_checkpoints(capsule: &AllocatedCapsule) {
    capsule
        .checkpoints
        .as_ref()
        .assert_value()
        .finish(&[])
        .await
        .assert_value();
}

#[tokio::test]
async fn post_terminal_success_and_force_stop_remove_checkpoint_copies() {
    for exit in [RunRuntimeExit::Completed, RunRuntimeExit::ForceStopped] {
        let fixture = CheckoutFixture::new("").await;
        let capsule = fixture.allocate().await.assert_value();
        capture_checkpoint(&capsule).await;
        assert_eq!(
            fixture.checkpoints(&fixture.run_id).await.checkpoints.len(),
            1
        );
        capsule
            .cleanup
            .destroy_or_confirm_absent(exit)
            .await
            .assert_value();
        if matches!(exit, RunRuntimeExit::Completed) {
            capsule
                .checkpoints
                .as_ref()
                .assert_value()
                .discard()
                .await
                .assert_value();
        }
        assert!(
            fixture
                .checkpoints(&fixture.run_id)
                .await
                .checkpoints
                .is_empty()
        );
        assert_eq!(
            fs::read_dir(fixture.root.child("checkpoints"))
                .assert_value()
                .count(),
            0
        );
        assert_eq!(
            fs::read_dir(fixture.root.child("checkpoint-repositories"))
                .assert_value()
                .count(),
            0
        );
        assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
    }
}

#[tokio::test]
async fn checkpoint_restore_survives_workspace_move_and_discard_removes_lineage_catalogs() {
    let fixture = CheckoutFixture::new("").await;
    let original = fixture.allocate().await.assert_value();
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    fs::write(workspace.join("README.md"), "saved tracked edit\n").assert_value();
    fs::write(workspace.join("untracked.txt"), "saved untracked edit\n").assert_value();
    capture_checkpoint(&original).await;
    let page = fixture.checkpoints(&fixture.run_id).await;
    assert_eq!(page.checkpoints.len(), 1);
    let checkpoint_id = &page.checkpoints[0].checkpoint_id;
    fs::write(workspace.join("README.md"), "failed write\n").assert_value();
    fs::write(workspace.join("untracked.txt"), "failed write\n").assert_value();
    fs::write(workspace.join("failed-only.txt"), "remove on restore\n").assert_value();
    original
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    assert_eq!(
        fs::read_dir(fixture.root.child("checkpoint-repositories"))
            .assert_value()
            .count(),
        1
    );

    let successor = RunId::new("checkpoint-successor");
    let resumed = fixture
        .allocator
        .allocate_from_retained(RetainedAllocationRequest {
            selection: CheckpointRestoreSelection::Checkpoint {
                checkpoint_id: checkpoint_id.clone(),
            },
            source_run_id: &fixture.run_id,
            run_id: &successor,
            admitted: &fixture.admitted,
            github_token: Some(CHECKOUT_TOKEN),
        })
        .await
        .assert_value();
    let workspace = fixture.allocator.run_path(&successor).join("workspace");
    assert_eq!(
        fs::read_to_string(workspace.join("README.md")).assert_value(),
        "saved tracked edit\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("untracked.txt")).assert_value(),
        "saved untracked edit\n"
    );
    assert!(!workspace.join("failed-only.txt").exists());
    assert!(workspace.join(".git").is_dir());
    assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
    assert!(resumed.execution_seed.is_empty());
    assert_eq!(fixture.checkpoints(&fixture.run_id).await, page);

    capture_checkpoint(&resumed).await;
    resumed
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    assert_eq!(fixture.checkpoints(&successor).await.checkpoints.len(), 1);
    assert!(
        fixture
            .allocator
            .discard_workspace(&successor)
            .await
            .assert_value()
    );
    assert!(fixture.checkpoints(&successor).await.checkpoints.is_empty());
    assert!(
        fixture
            .checkpoints(&fixture.run_id)
            .await
            .checkpoints
            .is_empty()
    );
}

#[tokio::test]
async fn missing_checkpoint_leaves_source_workspace_available_for_resume() {
    let fixture = CheckoutFixture::new("").await;
    let workspace = fixture.retain_untracked_workspace().await;
    let recovery = fixture.allocator.workspace_recovery(&fixture.run_id).await;
    assert!(recovery.recoverable);
    let successor = RunId::new("invalid-checkpoint-successor");
    let checkpoint_id = CheckpointId::new("missing-checkpoint").assert_value();
    let result = fixture
        .allocator
        .allocate_from_retained(RetainedAllocationRequest {
            selection: CheckpointRestoreSelection::Checkpoint { checkpoint_id },
            source_run_id: &fixture.run_id,
            run_id: &successor,
            admitted: &fixture.admitted,
            github_token: Some(CHECKOUT_TOKEN),
        })
        .await;
    assert!(matches!(
        result,
        Err(RetainedAllocationUnavailable::Settled(
            CapsuleAllocationUnavailable::Runtime,
        ))
    ));
    assert_eq!(
        fixture.allocator.workspace_recovery(&fixture.run_id).await,
        recovery
    );
    assert_eq!(
        fixture.allocator.workspace_recovery(&successor).await,
        Default::default()
    );
    assert!(!fixture.allocator.run_path(&successor).exists());
    assert_eq!(
        fs::read_to_string(workspace.join("untracked.txt")).assert_value(),
        "resume me\n"
    );

    let retry = RunId::new("retry-after-invalid-checkpoint");
    let resumed = fixture.allocate_retained(&retry).await.assert_value();
    fixture.assert_claimed_by(&retry).await;
    resumed
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
}

#[tokio::test]
async fn repeated_retained_successors_keep_the_root_delivery_identity() {
    let fixture = CheckoutFixture::new("").await;
    let original = fixture.allocate().await.assert_value();
    finish_checkpoints(&original).await;
    original
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();

    let first_successor = RunId::new("checkout-recovery-first-successor");
    let first = fixture
        .allocator
        .allocate_from_retained(RetainedAllocationRequest {
            selection: CheckpointRestoreSelection::Latest,
            source_run_id: &fixture.run_id,
            run_id: &first_successor,
            admitted: &fixture.admitted,
            github_token: Some(CHECKOUT_TOKEN),
        })
        .await
        .assert_value();
    assert_eq!(
        fixture
            .allocator
            .recovery_delivery_run_id(&first_successor)
            .as_ref(),
        Some(&fixture.run_id)
    );
    finish_checkpoints(&first).await;
    first
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    fixture
        .allocator
        .destroy_or_confirm_absent(&first_successor, RunRuntimeExit::Failed)
        .await
        .assert_value();
    assert_eq!(
        fixture
            .allocator
            .recovery_delivery_run_id(&first_successor)
            .as_ref(),
        Some(&fixture.run_id)
    );

    let second_successor = RunId::new("checkout-recovery-second-successor");
    let second = fixture
        .allocator
        .allocate_from_retained(RetainedAllocationRequest {
            selection: CheckpointRestoreSelection::Latest,
            source_run_id: &first_successor,
            run_id: &second_successor,
            admitted: &fixture.admitted,
            github_token: Some(CHECKOUT_TOKEN),
        })
        .await
        .assert_value();
    assert_eq!(
        fixture
            .allocator
            .recovery_delivery_run_id(&second_successor)
            .as_ref(),
        Some(&fixture.run_id)
    );
    second
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
}

#[tokio::test]
async fn retained_claim_reconciliation_rolls_back_before_workspace_move() {
    let fixture = CheckoutFixture::new("").await;
    let capsule = fixture.allocate().await.assert_value();
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    let successor = RunId::new("checkout-recovery-interrupted-before-move");
    fixture
        .allocator
        .write_test_recovery(&fixture.run_id, (false, None, Some(successor.clone())));

    fixture.allocator.reconcile_test_recovery();

    let source = fixture.allocator.workspace_recovery(&fixture.run_id).await;
    assert!(source.recoverable);
    assert!(source.successor_run_id.is_none());
    assert!(
        fixture
            .allocator
            .workspace_recovery(&successor)
            .await
            .resumed_from
            .is_none()
    );
}

#[tokio::test]
async fn retained_claim_reconciliation_removes_successor_metadata_before_workspace_move() {
    let fixture = CheckoutFixture::new("").await;
    let capsule = fixture.allocate().await.assert_value();
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    let successor = RunId::new("checkout-recovery-interrupted-after-successor-metadata");
    fixture
        .allocator
        .write_test_recovery(&fixture.run_id, (false, None, Some(successor.clone())));
    fixture
        .allocator
        .write_test_recovery(&successor, (false, Some(fixture.run_id.clone()), None));

    fixture.allocator.reconcile_test_recovery();

    let source = fixture.allocator.workspace_recovery(&fixture.run_id).await;
    assert!(source.recoverable);
    assert!(source.successor_run_id.is_none());
    assert!(
        fixture
            .allocator
            .workspace_recovery(&successor)
            .await
            .resumed_from
            .is_none()
    );
}

#[tokio::test]
async fn retained_claim_reconciliation_completes_after_workspace_move() {
    let fixture = CheckoutFixture::new("").await;
    let capsule = fixture.allocate().await.assert_value();
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    let successor = RunId::new("checkout-recovery-interrupted-after-move");
    fixture
        .allocator
        .write_test_recovery(&fixture.run_id, (false, None, Some(successor.clone())));
    fixture
        .allocator
        .write_test_recovery(&successor, (false, Some(fixture.run_id.clone()), None));
    std::fs::rename(
        fixture.allocator.run_path(&fixture.run_id),
        fixture.allocator.run_path(&successor),
    )
    .assert_value();

    fixture.allocator.reconcile_test_recovery();

    let source = fixture.allocator.workspace_recovery(&fixture.run_id).await;
    assert!(!source.recoverable);
    assert_eq!(source.successor_run_id.as_ref(), Some(&successor));
    assert_eq!(
        fixture
            .allocator
            .workspace_recovery(&successor)
            .await
            .resumed_from
            .as_ref(),
        Some(&fixture.run_id)
    );
}

#[tokio::test]
async fn retained_claim_reconciliation_preserves_a_failed_successor() {
    let fixture = CheckoutFixture::new("").await;
    let capsule = fixture.allocate().await.assert_value();
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    let successor = RunId::new("checkout-recovery-failed-successor");
    fixture
        .allocator
        .write_test_recovery(&fixture.run_id, (false, None, Some(successor.clone())));
    fixture
        .allocator
        .write_test_recovery(&successor, (true, Some(fixture.run_id.clone()), None));
    std::fs::rename(
        fixture.allocator.run_path(&fixture.run_id),
        fixture.allocator.run_path(&successor),
    )
    .assert_value();

    fixture.allocator.reconcile_test_recovery();

    let recovery = fixture.allocator.workspace_recovery(&successor).await;
    assert!(recovery.recoverable);
    assert_eq!(recovery.resumed_from.as_ref(), Some(&fixture.run_id));
}

#[tokio::test]
async fn retained_workspace_transfers_to_a_different_concurrent_writer_identity() {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let mut fixture = CheckoutFixture::new("").await;
    fixture.allocator.set_test_filesystem(production_filesystem);
    let capsule = fixture.allocate().await.assert_value();
    let source_workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    let source_metadata = fs::metadata(&source_workspace).assert_value();
    let retained = source_workspace.join("retained.txt");
    let create = std::process::Command::new("/bin/sh")
        .args(["-c", "umask 022; printf retained > retained.txt"])
        .current_dir(&source_workspace)
        .uid(source_metadata.uid())
        .gid(source_metadata.gid())
        .output()
        .assert_value();
    assert!(create.status.success());
    finish_checkpoints(&capsule).await;
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    assert_eq!(fs::symlink_metadata(&retained).assert_value().uid(), 0);

    let blocker_run_id = RunId::new("checkout-recovery-identity-blocker");
    let blocker = fixture
        .allocator
        .allocate(&blocker_run_id, &fixture.admitted, Some(CHECKOUT_TOKEN))
        .await
        .assert_value();
    let blocker_workspace = fixture
        .allocator
        .run_path(&blocker_run_id)
        .join("workspace");
    let blocker_metadata = fs::metadata(&blocker_workspace).assert_value();
    assert_eq!(blocker_metadata.uid(), source_metadata.uid());

    let successor = RunId::new("checkout-recovery-different-identity-successor");
    let resumed = fixture.allocate_retained(&successor).await.assert_value();
    let successor_workspace = fixture.allocator.run_path(&successor).join("workspace");
    assert_ne!(source_workspace, successor_workspace);
    assert!(!source_workspace.exists());
    let successor_retained = successor_workspace.join("retained.txt");
    let successor_metadata = fs::metadata(&successor_workspace).assert_value();
    assert_ne!(successor_metadata.uid(), blocker_metadata.uid());
    assert_eq!(
        fs::symlink_metadata(&successor_retained)
            .assert_value()
            .uid(),
        successor_metadata.uid()
    );

    let stale_writer = std::process::Command::new("/bin/sh")
        .args(["-c", "printf stale >> retained.txt"])
        .current_dir(&successor_workspace)
        .uid(blocker_metadata.uid())
        .gid(blocker_metadata.gid())
        .output()
        .assert_value();
    assert!(!stale_writer.status.success());
    let successor_writer = std::process::Command::new("/bin/sh")
        .args(["-c", "printf resumed >> retained.txt"])
        .current_dir(&successor_workspace)
        .uid(successor_metadata.uid())
        .gid(successor_metadata.gid())
        .output()
        .assert_value();
    assert!(successor_writer.status.success());
    assert_eq!(
        fs::read_to_string(&successor_retained).assert_value(),
        "retainedresumed"
    );

    resumed
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
    blocker
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
}

fn block_retained_cleanup(
    _workspace: &Path,
    runtime_home: &Path,
    _process_pool: HostedProcessPool,
) -> Result<CapsuleFilesystem, CapsuleAllocationUnavailable> {
    fs::write(runtime_home, "cleanup blocker")
        .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
    Err(CapsuleAllocationUnavailable::Runtime)
}
