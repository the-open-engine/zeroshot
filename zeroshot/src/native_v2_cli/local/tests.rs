use std::collections::BTreeMap;
use std::io::Write as _;

use openengine_cluster_protocol::{
    Cursor, EnumLabel, RunResumeParams, RunSize, RunStatus, RunStatusResult, RunTitle,
    TerminalResult,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::*;
use crate::native_v2_candidate::test_support::{TestDirectory, full_graph, success_node};
use crate::native_v2_local::PreparedLocalRun;
use crate::native_v2_supervisor::RunEnvironment;
use crate::v2_run_ledger::CreateRun;

fn contract_backend(root: &Path) -> LocalCliBackend {
    LocalCliBackend::new(
        root.to_owned(),
        PathBuf::from("zeroshot"),
        root.to_owned(),
        PathBuf::from("git"),
    )
}

pub(super) fn contract_submission(key: &str) -> RunSubmission {
    serde_json::from_value(json!({
        "title": "Local storage contract",
        "graph": full_graph(vec![success_node()]),
        "initialInput": null,
        "runtime": {
            "harness": "codex",
            "provider": "openai",
            "size": "small",
            "nodes": {}
        },
        "source": {
            "repository": "open-engine/zeroshot",
            "branch": "main",
            "revision": "0123456789abcdef0123456789abcdef01234567"
        },
        "submissionKey": key
    }))
    .assert_value()
}

fn recovery_document(
    submission: RunSubmission,
    workspace: PathBuf,
    resumed_from: Option<RunId>,
) -> LocalRecoveryDocument {
    LocalRecoveryDocument {
        submission,
        workspace,
        resumed_from,
        delivery_run_id: None,
        successor_run_id: None,
    }
}

fn contract_status(submission: &RunSubmission, run_id: RunId, failed: bool) -> RunStatusResult {
    RunStatusResult {
        run_id,
        title: RunTitle::new("Local storage contract").assert_value(),
        source: submission.source.clone(),
        size: RunSize::Small,
        at_cursor: Cursor::new(crate::v2_run_ledger::INITIAL_CURSOR),
        status: if failed {
            RunStatus::Finished {
                terminal_result: TerminalResult::Failed {
                    reason: EnumLabel::new("runtime_failed").assert_value(),
                },
                metadata: Default::default(),
            }
        } else {
            RunStatus::Admitted {}
        },
        workspace_recovery: Default::default(),
    }
}

fn prepared_local_run(
    run_id: RunId,
    submission: RunSubmission,
    workspace: PathBuf,
) -> PreparedLocalRun {
    PreparedLocalRun {
        delivery_run_id: run_id.clone(),
        run_id,
        environment: RunEnvironment::exact(&submission.runtime, BTreeMap::new()).assert_value(),
        submission,
        github_token: None,
        workspace,
        native_environment: BTreeMap::new(),
    }
}

#[test]
fn local_storage_contract_validates_paths_and_private_artifacts() {
    let root = TestDirectory::new("lsc");
    let backend = contract_backend(root.path());
    let first = RunId::new("0199f33f-3b44-7d21-9000-000000000001");
    let second = RunId::new("0199f33f-3b44-7d21-9000-000000000002");
    let invalid = RunId::new("not-a-local-run");

    assert!(matches!(
        backend.existing_run_paths(&first),
        Err(NativeV2CliError::RunNotFound { .. })
    ));
    assert!(backend.run_storage(&invalid).is_err());
    let second_storage = backend.create_run_storage(&second).assert_value();
    let first_storage = backend.create_run_storage(&first).assert_value();
    assert_eq!(
        backend.existing_run_paths(&first).assert_value().storage(),
        first_storage
    );
    std::fs::create_dir_all(root.child("runs").join("not-a-run")).assert_value();
    std::fs::write(
        root.child("runs")
            .join("0199f33f-3b44-7d21-9000-000000000003"),
        b"file",
    )
    .assert_value();
    assert_eq!(backend.local_run_ids().assert_value(), [first, second]);

    let workspace = root.child("workspace");
    let lease = backend.workspace_lease(&workspace).assert_value();
    assert_eq!(lease, backend.workspace_lease(&workspace).assert_value());
    assert_ne!(
        lease,
        backend.workspace_lease(&root.child("other")).assert_value()
    );

    let fresh_bootstrap = second_storage.join(BOOTSTRAP_FILE);
    std::fs::write(&fresh_bootstrap, b"private").assert_value();
    remove_stale_bootstrap(&fresh_bootstrap, Duration::MAX).assert_value();
    assert!(fresh_bootstrap.exists());
    remove_stale_bootstrap(&second_storage.join("missing"), Duration::ZERO).assert_value();

    let ledger = second_storage.join("ledger");
    assert!(!require_existing_ledger(&ledger).assert_value());
    std::fs::write(&ledger, b"sqlite").assert_value();
    assert!(require_existing_ledger(&ledger).assert_value());
    remove_private_bootstrap(&ledger);
    assert!(!ledger.exists());
    std::fs::create_dir(&ledger).assert_value();
    assert!(require_existing_ledger(&ledger).is_err());
    remove_private_bootstrap(&ledger);
    assert!(ledger.is_dir());
}

#[tokio::test]
async fn local_idempotency_contract_finds_matches_and_fails_closed() {
    let root = TestDirectory::new("local-idempotency-contract");
    let backend = contract_backend(root.path());
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000011");
    let wrong_identity = RunId::new("0199f33f-3b44-7d21-9000-000000000012");
    let submission = contract_submission("local-idempotency");
    let admitted = NativeV2Admission
        .admit(submission.clone())
        .await
        .assert_value();
    let digest = submission_digest(&submission).assert_value();
    let storage = backend.create_run_storage(&run_id).assert_value();
    let ledger = SqliteRunLedger::open(storage.join("runs.sqlite3")).assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: submission.submission_key.clone(),
            submission_digest: digest.clone(),
            admitted: admitted.clone(),
        })
        .await
        .assert_value();

    assert_eq!(
        backend
            .matching_submission(run_id.clone(), &submission.submission_key, &digest)
            .await
            .assert_value(),
        Some(run_id.clone())
    );
    assert_eq!(
        backend
            .existing_submission(&submission.submission_key, &digest)
            .await
            .assert_value(),
        Some(run_id.clone())
    );
    let conflicting_digest = submission_digest(&contract_submission("different")).assert_value();
    assert!(matches!(
        backend
            .matching_submission(
                run_id.clone(),
                &submission.submission_key,
                &conflicting_digest,
            )
            .await,
        Err(NativeV2CliError::SubmissionConflict { .. })
    ));

    let mismatched_storage = backend.create_run_storage(&wrong_identity).assert_value();
    SqliteRunLedger::open(mismatched_storage.join("runs.sqlite3"))
        .assert_value()
        .create_or_get(CreateRun {
            run_id,
            submission_key: submission.submission_key.clone(),
            submission_digest: digest.clone(),
            admitted,
        })
        .await
        .assert_value();
    assert!(matches!(
        backend
            .matching_submission(wrong_identity, &submission.submission_key, &digest)
            .await,
        Err(NativeV2CliError::Local(message))
            if message.contains("ledger identity")
    ));
}

#[test]
fn local_recovery_contract_preserves_lineage_and_recoverability() {
    let root = TestDirectory::new("local-recovery-contract");
    let backend = contract_backend(root.path());
    let original = RunId::new("0199f33f-3b44-7d21-9000-000000000021");
    let successor = RunId::new("0199f33f-3b44-7d21-9000-000000000022");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let submission = contract_submission("local-recovery");
    backend.create_run_storage(&original).assert_value();
    backend.create_run_storage(&successor).assert_value();
    let original_document = recovery_document(submission.clone(), workspace.clone(), None);
    backend
        .write_recovery_document(&original, &original_document)
        .assert_value();
    let successor_document = recovery_document(
        submission.clone(),
        workspace.clone(),
        Some(original.clone()),
    );
    backend
        .write_recovery_document(&successor, &successor_document)
        .assert_value();

    assert_eq!(
        backend
            .delivery_run_id(&successor, &successor_document)
            .assert_value(),
        original
    );
    let failed = contract_status(&submission, successor.clone(), true);
    assert!(
        backend
            .workspace_recovery(&failed)
            .assert_value()
            .recoverable
    );
    let admitted = contract_status(&submission, successor.clone(), false);
    assert!(
        !backend
            .workspace_recovery(&admitted)
            .assert_value()
            .recoverable
    );

    let missing_successor = RunId::new("0199f33f-3b44-7d21-9000-000000000023");
    let mut claimed = successor_document;
    claimed.successor_run_id = Some(missing_successor);
    backend
        .write_recovery_document(&successor, &claimed)
        .assert_value();
    assert!(
        backend
            .claimed_local_successor(&successor)
            .assert_value()
            .is_none()
    );
    assert_eq!(
        backend
            .read_recovery_document(&successor)
            .assert_value()
            .successor_run_id,
        None
    );

    std::fs::write(
        backend.recovery_path(&successor).assert_value(),
        b"not json",
    )
    .assert_value();
    assert!(matches!(
        backend.read_recovery_document(&successor),
        Err(NativeV2CliError::Local(message))
            if message == "workspace recovery metadata is invalid"
    ));
}

#[tokio::test]
async fn wave5_cli_contract_bootstrap_and_resume_failures_leave_recoverable_state() {
    let root = TestDirectory::new("lbr");
    let backend = LocalCliBackend::new(
        root.path().to_owned(),
        root.child("missing-controller"),
        root.path().to_owned(),
        PathBuf::from("git"),
    )
    .with_ready_timeout(Duration::ZERO);
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let original = RunId::new("0199f33f-3b44-7d21-9000-000000000041");
    let successor = RunId::new("0199f33f-3b44-7d21-9000-000000000042");
    let submission = contract_submission("controller-bootstrap-rollback");

    let error = backend
        .start_prepared_controller(prepared_local_run(
            original.clone(),
            submission.clone(),
            workspace.clone(),
        ))
        .await
        .unwrap_err();
    assert!(matches!(error, NativeV2CliError::Local(_)));
    let original_storage = backend.run_storage(&original).assert_value();
    assert!(original_storage.is_dir());
    assert!(!original_storage.join(BOOTSTRAP_FILE).exists());
    assert_eq!(
        backend
            .read_recovery_document(&original)
            .assert_value()
            .delivery_run_id,
        Some(original.clone())
    );

    let recovery = recovery_document(submission, workspace, None);
    backend
        .write_recovery_document(&original, &recovery)
        .assert_value();
    let error = backend
        .start_local_successor(
            RunResumeParams {
                run_id: original.clone(),
                successor_run_id: successor.clone(),
                connections: BTreeMap::new(),
                connection_resolver: None,
                github_token: None,
            },
            recovery,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, NativeV2CliError::Local(_)));
    assert!(!backend.run_storage(&successor).assert_value().exists());
    assert_eq!(
        backend
            .read_recovery_document(&original)
            .assert_value()
            .successor_run_id,
        None
    );
}

#[tokio::test]
async fn wave5_cli_contract_enumeration_and_readiness_fail_closed_without_controller() {
    let root = TestDirectory::new("ler");
    let backend = contract_backend(root.path()).with_ready_timeout(Duration::ZERO);
    std::fs::create_dir_all(root.child("runs/not-a-run")).assert_value();
    std::fs::write(root.child("runs/also-not-a-run"), b"ignored").assert_value();
    assert!(backend.list_local().await.assert_value().runs.is_empty());
    assert!(
        backend
            .list_entry(Err(std::io::Error::other("unreadable entry")))
            .await
            .assert_value()
            .is_none()
    );

    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000051");
    let other = RunId::new("0199f33f-3b44-7d21-9000-000000000052");
    assert!(matches!(
        backend.existing_run_paths(&run_id),
        Err(NativeV2CliError::RunNotFound { .. })
    ));
    let storage = backend.create_run_storage(&run_id).assert_value();
    let paths = PortableControllerPaths::new(storage);
    let ready = serde_json::to_vec(&serde_json::json!({
        "kind": "zeroshot.portable-controller-ready/v1",
        "runId": other,
        "socket": paths.socket(),
        "pid": 1
    }))
    .assert_value();
    private_file(&paths.ready(), FileAccess::CreateNew)
        .assert_value()
        .write_all(&ready)
        .assert_value();
    assert!(matches!(
        backend.connect_run(&run_id).await,
        Err(NativeV2CliError::Local(message))
            if message.contains("different run identity")
    ));
}

#[test]
fn wave5_cli_contract_recovery_lineage_is_bounded_and_honors_root_delivery() {
    let root = TestDirectory::new("local-bounded-lineage");
    let backend = contract_backend(root.path());
    let first = RunId::new("0199f33f-3b44-7d21-9000-000000000061");
    let second = RunId::new("0199f33f-3b44-7d21-9000-000000000062");
    let delivery = RunId::new("0199f33f-3b44-7d21-9000-000000000063");
    let submission = contract_submission("bounded-lineage");
    for run_id in [&first, &second] {
        backend.create_run_storage(run_id).assert_value();
    }
    let direct = LocalRecoveryDocument {
        submission: submission.clone(),
        workspace: root.child("workspace"),
        resumed_from: None,
        delivery_run_id: Some(delivery.clone()),
        successor_run_id: None,
    };
    assert_eq!(
        backend.delivery_run_id(&first, &direct).assert_value(),
        delivery
    );

    let first_document = recovery_document(
        submission.clone(),
        root.child("workspace"),
        Some(second.clone()),
    );
    let second_document =
        recovery_document(submission, root.child("workspace"), Some(first.clone()));
    backend
        .write_recovery_document(&first, &first_document)
        .assert_value();
    backend
        .write_recovery_document(&second, &second_document)
        .assert_value();
    assert!(backend.delivery_run_id(&first, &first_document).is_err());
}

#[tokio::test]
async fn wave7_cli_contract_local_stale_state_and_readiness_fail_closed() {
    let root = TestDirectory::new("local-wave7-boundaries");
    let backend = contract_backend(root.path());
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000071");
    let storage = backend.create_run_storage(&run_id).assert_value();
    let bootstrap = storage.join(BOOTSTRAP_FILE);
    std::fs::write(&bootstrap, b"stale").assert_value();
    remove_stale_bootstrap(&bootstrap, Duration::ZERO).assert_value();
    assert!(!bootstrap.exists());

    std::fs::write(&bootstrap, b"stale").assert_value();
    backend
        .remove_stale_bootstraps(Duration::ZERO)
        .assert_value();
    assert!(!bootstrap.exists());
    assert!(require_local(None).is_ok());
    assert!(require_local(Some("prod")).is_err());

    let paths = PortableControllerPaths::new(storage);
    let mut exited = tokio::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .assert_value();
    exited.wait().await.assert_value();
    assert!(
        wait_for_controller(&mut exited, &paths, &run_id, Duration::ZERO)
            .await
            .assert_error()
            .to_string()
            .contains("exited before becoming ready")
    );
}
