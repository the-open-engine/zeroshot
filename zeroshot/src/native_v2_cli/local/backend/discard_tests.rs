use super::*;
use super::super::local_contract_tests::{contract_submission, create_contract_run};
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_runner::test_support::delivery_checkpoint;
use crate::native_v2_supervisor::checkpoints::{DeliveryCheckpointStore, RunCheckpointStore};
use crate::v2_run_ledger::RunEvent;
use openengine_cluster_protocol::{EnumLabel, RunDiscardWorkspaceParams};
use serde_json::json;
use std::fs;

struct Fixture {
    _root: TestDirectory,
    backend: LocalCliBackend,
    id: RunId,
    repository: PathBuf,
}

fn failed() -> RunEvent {
    RunEvent::Terminal {
        result: TerminalResult::Failed {
            reason: EnumLabel::new("worker_failed").unwrap(),
            context: None,
        },
    }
}

fn assert_refusal(error: NativeV2CliError, message: &str) {
    assert!(error.to_string().contains(message), "{error}");
    let diagnostic = serde_json::to_value(error.diagnostic()).unwrap();
    assert_eq!(diagnostic["kind"], "invalid_request");
    assert_eq!(diagnostic["code"], "request.invalid");
}

impl Fixture {
    async fn new(end: Option<RunEvent>) -> Self {
        let root = TestDirectory::new("discard-local");
        let state = root.child("state");
        crate::execution::platform::create_private_directory(&state).unwrap();
        let mut backend =
            LocalCliBackend::new(state, "unused".into(), root.path().into(), "git".into());
        backend.ready_timeout = Duration::from_millis(30);
        let id = RunId::new(uuid::Uuid::now_v7().to_string());
        let repository = backend.checkpoint_repository(&id).unwrap();
        let fixture = Self {
            _root: root,
            backend,
            id,
            repository,
        };
        let recovery = LocalRecoveryDocument {
            submission: contract_submission(fixture.id.as_str()),
            // Discard must succeed even when the user-owned checkout is unavailable.
            workspace: fixture._root.child("missing-workspace"),
            delivery_receipt: None,
            local_branch: None,
            invoking_workspace: None,
            request_digest: None,
            workspace_revision: None,
            resumed_from: None,
            delivery_run_id: None,
            successor_run_id: None,
            workspace_checkpoints_discarded: false,
            pending_restore: None,
        };
        fixture.record(&fixture.id, recovery, end).await;
        fixture
    }

    async fn record(&self, id: &RunId, recovery: LocalRecoveryDocument, end: Option<RunEvent>) {
        let ledger = create_contract_run(&self.backend, id, &recovery.submission).await;
        let mut events = vec![RunEvent::RunStarted];
        events.extend(end);
        ledger.append(id, events).await.unwrap();
        self.backend.write_recovery_document(id, &recovery).unwrap();
        let directory = self
            .backend
            .paths(id)
            .unwrap()
            .storage()
            .join("checkpoints");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("storage.json"),
            serde_json::to_vec(&json!({
                "format":1,"repository":self.repository,"program":{"executable":"unavailable"}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(directory.join("index.json"), "[]").unwrap();
        crate::execution::platform::private_directory(&self.repository).unwrap();
        fs::create_dir_all(self.repository.join("repository/data")).unwrap();
        fs::write(
            self.repository.join("repository/data/pack"),
            "checkpoint bytes",
        )
        .unwrap();
    }

    async fn successor(&self, end: Option<RunEvent>) -> RunId {
        let id = RunId::new(uuid::Uuid::now_v7().to_string());
        let mut source = self.backend.read_recovery_document(&self.id).unwrap();
        source.successor_run_id = Some(id.clone());
        self.backend
            .write_recovery_document(&self.id, &source)
            .unwrap();
        source.successor_run_id = None;
        source.resumed_from = Some(self.id.clone());
        source.delivery_run_id = Some(self.id.clone());
        source.submission.submission_key = IdempotencyKey::new(id.as_str()).unwrap();
        self.record(&id, source, end).await;
        id
    }

    async fn discard(&self, id: &RunId) -> Result<bool, NativeV2CliError> {
        Ok(self
            .backend
            .run_discard_workspace(
                None,
                RunDiscardWorkspaceParams {
                    part_id: None,
                    run_id: id.clone(),
                },
            )
            .await?
            .discarded)
    }

    fn pending_restore(&self, attempt: &RunId) {
        let mut recovery = self.backend.read_recovery_document(&self.id).unwrap();
        recovery.pending_restore = Some(PendingRestore {
            selection: CheckpointRestoreSelection::Latest,
            snapshot: serde_json::from_value(json!(uuid::Uuid::now_v7().to_string())).unwrap(),
            attempt: attempt.clone(),
        });
        self.backend
            .write_recovery_document(&self.id, &recovery)
            .unwrap();
    }

    fn retained(&self) {
        assert_eq!(
            fs::read_to_string(self.repository.join("repository/data/pack")).unwrap(),
            "checkpoint bytes"
        );
        assert!(
            self.backend
                .paths(&self.id)
                .unwrap()
                .storage()
                .join("checkpoints/storage.json")
                .exists()
        );
    }
}

#[tokio::test]
async fn local_discard_keeps_delivery_generations_and_other_lineages_without_a_checkout() {
    let fixture = Fixture::new(Some(failed())).await;
    let successor = fixture.successor(Some(failed())).await;
    let state = delivery_checkpoint(json!({"receipts":[{"owned":"acknowledged"}]}), 3);
    let store = DeliveryCheckpointStore::new(fixture.repository.clone());
    store.retain_delivery_state(&state).await.unwrap();
    let delivery = fs::read(fixture.repository.join("delivery-state.json")).unwrap();
    let unrelated = fixture
        .backend
        .checkpoint_repository(&RunId::new(uuid::Uuid::now_v7().to_string()))
        .unwrap();
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("sentinel"), "other lineage").unwrap();
    assert!(fixture.discard(&successor).await.unwrap());
    assert_eq!(store.delivery_state().await.unwrap(), Some(state));
    assert_eq!(
        fs::read(fixture.repository.join("delivery-state.json")).unwrap(),
        delivery
    );
    assert_eq!(fs::read_dir(&fixture.repository).unwrap().count(), 1);
    assert_eq!(
        fs::read_to_string(unrelated.join("sentinel")).unwrap(),
        "other lineage"
    );
    for id in [&fixture.id, &successor] {
        let recovery = fixture.backend.read_recovery_document(id).unwrap();
        assert!(recovery.submission.runtime.workspace_checkpoints);
        assert!(!recovery.workspace_checkpoints_enabled());
        assert!(
            !fixture
                .backend
                .paths(id)
                .unwrap()
                .storage()
                .join("checkpoints")
                .exists()
        );
        assert!(!fixture.discard(id).await.unwrap());
    }
}

#[tokio::test]
async fn local_discard_refuses_suspension_pending_restore_and_active_lineage() {
    let suspended = RunEvent::Suspended {
        suspension: serde_json::from_value(
            json!({"reason":"command","node":"pause","createdAt":1000}),
        )
        .unwrap(),
    };
    for (end, message) in [(None, "force-stop"), (Some(suspended), "run is suspended")] {
        let fixture = Fixture::new(end).await;
        assert_refusal(fixture.discard(&fixture.id).await.unwrap_err(), message);
        fixture.retained();
    }
    let fixture = Fixture::new(Some(failed())).await;
    let successor = fixture.successor(Some(failed())).await;
    let mut recovery = fixture.backend.read_recovery_document(&successor).unwrap();
    recovery.pending_restore = Some(PendingRestore {
        selection: CheckpointRestoreSelection::Latest,
        snapshot: serde_json::from_value(json!(uuid::Uuid::now_v7().to_string())).unwrap(),
        attempt: RunId::new(uuid::Uuid::now_v7().to_string()),
    });
    fixture
        .backend
        .write_recovery_document(&successor, &recovery)
        .unwrap();
    let error = fixture.discard(&fixture.id).await.unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("interrupted checkpoint restore")
            && message.contains("repeat `zeroshot resume"),
        "{error}"
    );
    assert_refusal(error, "interrupted checkpoint restore");
    fixture.retained();
}

#[tokio::test]
async fn local_discard_excludes_preparing_resumes_and_unsettled_controllers_in_both_directions() {
    let fixture = Fixture::new(Some(failed())).await;
    let successor = fixture.successor(Some(failed())).await;
    for (claimed, requested) in [(&fixture.id, &successor), (&successor, &fixture.id)] {
        let _claim = fixture.backend.claim_recovery_workspace(claimed).unwrap();
        assert_refusal(fixture.discard(requested).await.unwrap_err(), "force-stop");
        assert_eq!(
            fixture
                .backend
                .cleanup_checkpoints(
                    true,
                    openengine_cluster_protocol::UnixTimestampMillis::new(1).unwrap()
                )
                .await
                .unwrap(),
            0
        );
        fixture.retained();
    }
    let _lease =
        ControllerLease::acquire(fixture.backend.paths(&successor).unwrap().lease()).unwrap();
    assert_refusal(
        fixture.discard(&fixture.id).await.unwrap_err(),
        "force-stop",
    );
    assert_eq!(
        fixture
            .backend
            .cleanup_checkpoints(
                true,
                openengine_cluster_protocol::UnixTimestampMillis::new(1).unwrap()
            )
            .await
            .unwrap(),
        0
    );
    fixture.retained();
}

#[tokio::test]
async fn local_discard_allows_a_suspended_predecessor_with_a_finished_successor() {
    let fixture = Fixture::new(Some(RunEvent::Suspended {
        suspension: serde_json::from_value::<openengine_cluster_protocol::RunSuspension>(
            json!({"reason":"command","node":"pause","createdAt":1000}),
        )
        .unwrap(),
    }))
    .await;
    let mut recovery = fixture.backend.read_recovery_document(&fixture.id).unwrap();
    recovery.successor_run_id = Some(RunId::new(uuid::Uuid::now_v7().to_string()));
    fixture
        .backend
        .write_recovery_document(&fixture.id, &recovery)
        .unwrap();
    assert!(
        fixture
            .backend
            .protected_file_runs()
            .await
            .unwrap()
            .contains(&fixture.id)
    );
    fixture.successor(Some(failed())).await;
    assert!(
        fixture
            .backend
            .protected_file_runs()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(fixture.discard(&fixture.id).await.unwrap());
}

#[tokio::test]
async fn local_discard_checks_shared_catalogs_even_without_a_successor_link() {
    let fixture = Fixture::new(Some(failed())).await;
    let shared = RunId::new(uuid::Uuid::now_v7().to_string());
    let mut recovery = fixture.backend.read_recovery_document(&fixture.id).unwrap();
    recovery.delivery_run_id = Some(fixture.id.clone());
    recovery.resumed_from = Some(fixture.id.clone());
    recovery.submission.submission_key = IdempotencyKey::new(shared.as_str()).unwrap();
    fixture.record(&shared, recovery, None).await;
    let error = fixture.discard(&fixture.id).await.unwrap_err().to_string();
    assert!(
        error.contains(shared.as_str()) && error.contains("force-stop"),
        "{error}"
    );
    fixture.retained();
}

#[tokio::test]
async fn local_discard_reconciles_a_completed_restore_after_its_launcher_exits() {
    for request_predecessor in [false, true] {
        let mut fixture = Fixture::new(Some(failed())).await;
        let successor = fixture.successor(Some(failed())).await;
        fixture.pending_restore(&successor);
        let requested = if request_predecessor {
            &fixture.id
        } else {
            &successor
        };
        fixture.backend.fail_recovery_write = Some(fixture.id.clone());
        assert!(
            fixture
                .discard(requested)
                .await
                .unwrap_err()
                .to_string()
                .contains("injected recovery write failure")
        );
        fixture.retained();
        assert!(
            fixture
                .backend
                .read_recovery_document(&fixture.id)
                .unwrap()
                .pending_restore
                .is_some()
        );
        fixture.backend.fail_recovery_write = None;
        assert!(fixture.discard(requested).await.unwrap());
        let predecessor = fixture.backend.read_recovery_document(&fixture.id).unwrap();
        assert!(predecessor.pending_restore.is_none());
        assert_eq!(predecessor.successor_run_id, Some(successor.clone()));
        assert_eq!(
            fixture
                .backend
                .read_recovery_document(&successor)
                .unwrap()
                .resumed_from,
            Some(fixture.id.clone())
        );
        assert!(!fixture.discard(&successor).await.unwrap());
    }
}

#[tokio::test]
async fn local_discard_preserves_unmatched_and_unfinished_restore_markers() {
    for wrong_attempt in [false, true] {
        let fixture = Fixture::new(Some(failed())).await;
        let successor = fixture.successor(Some(failed())).await;
        if wrong_attempt {
            fixture.pending_restore(&RunId::new(uuid::Uuid::now_v7().to_string()));
        } else {
            fixture.pending_restore(&successor);
            let mut recovery = fixture.backend.read_recovery_document(&successor).unwrap();
            recovery.resumed_from = None;
            fixture
                .backend
                .write_recovery_document(&successor, &recovery)
                .unwrap();
        }
        assert!(
            fixture
                .discard(&fixture.id)
                .await
                .unwrap_err()
                .to_string()
                .contains("interrupted checkpoint restore")
        );
        assert!(
            fixture
                .backend
                .read_recovery_document(&fixture.id)
                .unwrap()
                .pending_restore
                .is_some()
        );
        fixture.retained();
    }
    let fixture = Fixture::new(Some(failed())).await;
    let successor = fixture.successor(Some(failed())).await;
    fixture.pending_restore(&successor);
    let paths = fixture.backend.paths(&successor).unwrap();
    fs::remove_file(paths.ledger()).unwrap();
    drop(SqliteRunLedger::open(paths.ledger()).unwrap());
    // A controller can still own an unfinished restore without having admitted the successor.
    let _lease = ControllerLease::acquire(paths.lease()).unwrap();
    let _submission = fixture.backend.acquire_submission_lock().await.unwrap();
    let _claim = fixture
        .backend
        .claim_recovery_workspace(&fixture.id)
        .unwrap();
    fixture
        .backend
        .reconcile_local_resume_claim(&fixture.id, Duration::ZERO)
        .await
        .unwrap();
    assert!(
        fixture
            .backend
            .read_recovery_document(&fixture.id)
            .unwrap()
            .pending_restore
            .is_some()
    );
    fixture.retained();
}

#[tokio::test]
async fn local_discard_recovery_write_failure_preserves_all_checkpoint_data() {
    for fail_root in [false, true] {
        let mut fixture = Fixture::new(Some(failed())).await;
        let successor = fixture.successor(Some(failed())).await;
        let submissions = [&fixture.id, &successor].map(|id| {
            serde_json::to_value(
                fixture
                    .backend
                    .read_recovery_document(id)
                    .unwrap()
                    .submission,
            )
            .unwrap()
        });
        fixture.backend.fail_recovery_write = Some(if fail_root {
            fixture.id.clone()
        } else {
            successor.clone()
        });
        let error = fixture.discard(&successor).await.unwrap_err().to_string();
        assert!(error.contains("injected recovery write failure"), "{error}");
        fixture.retained();
        assert!(
            fixture
                .backend
                .paths(&successor)
                .unwrap()
                .storage()
                .join("checkpoints/storage.json")
                .exists()
        );
        fixture.backend.fail_recovery_write = None;
        assert!(fixture.discard(&fixture.id).await.unwrap());
        for (id, submission) in [&fixture.id, &successor].into_iter().zip(submissions) {
            let recovery = fixture.backend.read_recovery_document(id).unwrap();
            assert!(!recovery.workspace_checkpoints_enabled());
            assert_eq!(
                serde_json::to_value(recovery.submission).unwrap(),
                submission
            );
        }
    }
}

#[tokio::test]
async fn local_discard_retry_after_last_deletion_retains_disabled_policy() {
    for already_disabled in [false, true] {
        let fixture = Fixture::new(Some(failed())).await;
        let mut recovery = fixture.backend.read_recovery_document(&fixture.id).unwrap();
        recovery.workspace_checkpoints_discarded = already_disabled;
        fixture
            .backend
            .write_recovery_document(&fixture.id, &recovery)
            .unwrap();
        // Simulate interruption after the final deletion, including a legacy attempt that
        // removed data before persisting the disabled policy.
        let directory = fixture
            .backend
            .paths(&fixture.id)
            .unwrap()
            .storage()
            .join("checkpoints");
        assert!(
            crate::native_v2_supervisor::checkpoints::discard_workspace_lineage(
                &directory,
                &fixture.repository,
            )
            .unwrap()
        );
        assert!(!fixture.discard(&fixture.id).await.unwrap());
        let recovery = fixture.backend.read_recovery_document(&fixture.id).unwrap();
        assert!(!recovery.workspace_checkpoints_enabled());
        assert!(
            local_restore_selection(&fixture.id, None, &recovery, false)
                .unwrap()
                .is_none()
        );
        let error = local_restore_selection(&fixture.id, None, &recovery, true).unwrap_err();
        assert!(error.to_string().contains("no checkpoints"));
    }
}

#[tokio::test]
async fn cleanup_selects_terminal_checkpoint_lineages_at_sixty_days_and_is_explicit() {
    const DAYS_60: u64 = 60 * 24 * 60 * 60 * 1000;
    let fixture = Fixture::new(Some(failed())).await;
    let ledger =
        SqliteRunLedger::open_read_only(fixture.backend.paths(&fixture.id).unwrap().ledger())
            .unwrap();
    let finished = ledger
        .get(&fixture.id)
        .await
        .unwrap()
        .unwrap()
        .snapshot
        .terminal_recorded_at
        .unwrap()
        .get();
    let at = |value| openengine_cluster_protocol::UnixTimestampMillis::new(value).unwrap();
    fixture.retained(); // Startup and lookup never trigger cleanup.
    assert_eq!(
        fixture
            .backend
            .cleanup_checkpoints(false, at(finished + DAYS_60 - 1))
            .await
            .unwrap(),
        0
    );
    fixture.retained();
    assert_eq!(
        fixture
            .backend
            .cleanup_checkpoints(false, at(finished + DAYS_60))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        fixture
            .backend
            .cleanup_checkpoints(false, at(finished + DAYS_60))
            .await
            .unwrap(),
        0
    );
    assert!(!fixture.repository.join("repository").exists());
    assert!(
        fixture
            .backend
            .paths(&fixture.id)
            .unwrap()
            .ledger()
            .exists()
    );
    let fixture = Fixture::new(Some(failed())).await;
    assert_eq!(
        fixture
            .backend
            .cleanup_checkpoints(true, at(1))
            .await
            .unwrap(),
        1
    );
    let live = Fixture::new(None).await;
    assert_eq!(
        live.backend
            .cleanup_checkpoints(true, at(openengine_cluster_protocol::MAX_SAFE_GENERATION))
            .await
            .unwrap(),
        0
    );
    live.retained();
}
