use std::collections::BTreeMap;
use std::io::Write as _;

use openengine_cluster_protocol::{
    Cursor, EnumLabel, NodeName, RunResumeParams, RunSize, RunStatus, RunStatusResult, RunTitle,
    TerminalResult,
};
#[cfg(unix)]
use openengine_cluster_testkit::assertions::AssertError;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::native_v2_candidate::test_support::{TestDirectory, full_graph, success_node};
#[cfg(unix)]
use crate::native_v2_cli::TargetRunIntent;
use crate::native_v2_local::PreparedLocalRun;
use crate::native_v2_supervisor::RunEnvironment;
use crate::v2_run_ledger::CreateRun;

#[cfg(unix)]
#[path = "preflight_tests.rs"]
mod preflight_tests;

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
                    context: None,
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

pub(super) async fn create_contract_run(
    backend: &LocalCliBackend,
    run_id: &RunId,
    submission: &RunSubmission,
) -> SqliteRunLedger {
    let admitted = NativeV2Admission
        .admit(submission.clone())
        .await
        .assert_value();
    let storage = backend.create_run_storage(run_id).assert_value();
    let ledger = SqliteRunLedger::open(storage.join("runs.sqlite3")).assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: submission.submission_key.clone(),
            submission_digest: submission_digest(submission).assert_value(),
            admitted,
        })
        .await
        .assert_value();
    ledger
}

fn prepared_local_run(
    run_id: RunId,
    submission: RunSubmission,
    workspace: PathBuf,
) -> PreparedLocalRun {
    PreparedLocalRun {
        git_credentials: None,

        delivery_run_id: run_id.clone(),
        run_id,
        environment: RunEnvironment::exact(
            &submission.runtime,
            submission.environment.as_ref(),
            BTreeMap::new(),
        )
        .assert_value(),
        submission,
        github_token: None,
        github_token_file: None,
        delivery_validator: None,
        workspace,
        native_environment: BTreeMap::new(),
    }
}

#[cfg(unix)]
fn prepared_request(run_id: RunId, submission_key: &str) -> PreparedRunRequest {
    let submission = contract_submission(submission_key);
    PreparedRunRequest {
        git_credentials: None,

        cwd: None,
        source_connection: None,

        run_id,
        intent: TargetRunIntent {
            environment: None,
            title: submission.title,
            graph: submission.graph,
            initial_input: submission.initial_input,
            runtime: submission.runtime,
            branch: None,
            submission_key: submission.submission_key,
        },
        connections: BTreeMap::new(),
        github_token: None,
        source: None,
        profile: None,
    }
}

#[cfg(unix)]
fn initialize_local_repository(root: &Path) {
    fn git(root: &Path, arguments: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(arguments)
            .status()
            .assert_value();
        assert!(status.success(), "git {arguments:?} failed");
    }

    git(root, &["init", "--quiet"]);
    git(root, &["config", "user.name", "Zeroshot Test"]);
    git(root, &["config", "user.email", "zeroshot@example.invalid"]);

    std::fs::write(root.join("README.md"), "fixture\n").assert_value();

    git(root, &["add", "README.md"]);
    git(root, &["commit", "--quiet", "-m", "fixture"]);
    git(root, &["branch", "-M", "main"]);

    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/open-engine/zeroshot.git",
        ],
    );
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
    let digest = submission_digest(&submission).assert_value();
    create_contract_run(&backend, &run_id, &submission).await;
    backend
        .write_recovery_document(
            &run_id,
            &recovery_document(submission.clone(), root.path().to_owned(), None),
        )
        .assert_value();

    let admitted = NativeV2Admission
        .admit(submission.clone())
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
            .existing_submission(&submission.submission_key, &digest, root.path())
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
async fn lane_preflight_checks_the_retained_workspace_only_without_checkpoint_restore() {
    for restoring in [false, true] {
        // Windows also searches the parent PATH and executable directory, so an empty captured
        // PATH cannot establish absence. Keep the checkpoint launch check portable.
        if !restoring && cfg!(windows) {
            continue;
        }
        let root = TestDirectory::new("lane-preflight");
        let backend = LocalCliBackend::new(
            root.path().to_owned(),
            root.child("missing-controller"),
            root.path().to_owned(),
            PathBuf::from("git"),
        )
        .with_ready_timeout(Duration::ZERO);
        let workspace = root.child("workspace");
        let empty_bin = root.child("bin");
        for directory in [&workspace, &empty_bin] {
            std::fs::create_dir(directory).assert_value();
        }
        let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000061");
        let mut submission = contract_submission("lane-preflight");
        submission.runtime.nodes_mut().insert(
            NodeName::new("worker").assert_value(),
            serde_json::from_value(json!({
                "kind": "agent",
                "lane": { "harness": "claude", "provider": "anthropic" },
                "model": "provider-owned-model"
            }))
            .assert_value(),
        );
        let mut prepared = prepared_local_run(run_id.clone(), submission, workspace);
        prepared.native_environment =
            BTreeMap::from([("PATH".to_owned(), empty_bin.to_string_lossy().into_owned())]);

        let checkpoint = restoring.then(|| {
            crate::native_v2_supervisor::checkpoints::CheckpointRestore {
                directory: root.child("selected-snapshot"),
                selection:
                    crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection::Latest,
            }
        });
        let error = backend
            .start_prepared_controller_with_lineage(prepared, None, checkpoint)
            .await
            .unwrap_err();

        if restoring {
            assert!(matches!(error, NativeV2CliError::Local(_)), "{error}");
            let storage = backend.run_storage(&run_id).assert_value();
            assert!(
                storage.exists(),
                "checkpoint resume must reach controller launch"
            );
            assert!(!storage.join(BOOTSTRAP_FILE).exists());
            continue;
        }
        assert!(matches!(error, NativeV2CliError::Usage(_)));
        assert_eq!(
            error.to_string(),
            "lane claude/anthropic needs the `claude` executable on PATH"
        );
        assert_eq!(
            serde_json::to_value(error.diagnostic()).assert_value()["code"],
            "request.invalid"
        );
        assert!(!backend.run_storage(&run_id).assert_value().exists());
    }
}

// Windows also searches the parent PATH and executable directory, so an empty captured PATH
// cannot establish that gh is absent.
#[cfg(unix)]
#[tokio::test]
async fn github_delivery_preflight_rejects_a_missing_gh_before_launch() {
    let root = TestDirectory::new("delivery-preflight");
    let backend = LocalCliBackend::new(
        root.path().to_owned(),
        root.child("missing-controller"),
        root.path().to_owned(),
        PathBuf::from("git"),
    )
    .with_ready_timeout(Duration::ZERO);
    let workspace = root.child("workspace");
    let bin = root.child("bin");
    for directory in [&workspace, &bin] {
        std::fs::create_dir(directory).assert_value();
    }
    openengine_cluster_testkit::fixture::write_executable(
        &bin.join("git"),
        "#!/bin/sh\nexit 0\n",
        0o755,
    )
    .assert_value();
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000062");
    let mut submission = contract_submission("delivery-preflight");
    submission.graph = full_graph(vec![
        crate::native_v2_candidate::test_support::git_delivery_node(),
        success_node(),
    ]);
    let mut prepared = prepared_local_run(run_id.clone(), submission, workspace);
    prepared.native_environment =
        BTreeMap::from([("PATH".to_owned(), bin.to_string_lossy().into_owned())]);

    let error = backend
        .start_prepared_controller_with_lineage(prepared, None, None)
        .await
        .unwrap_err();

    assert!(matches!(error, NativeV2CliError::Usage(_)));
    assert_eq!(
        error.to_string(),
        "Git delivery needs the `gh` executable on PATH"
    );
    assert!(!backend.run_storage(&run_id).assert_value().exists());
}

#[tokio::test]
async fn local_bootstrap_and_resume_failures_leave_recoverable_state() {
    let root = TestDirectory::new("lbr");
    let backend = LocalCliBackend::new(
        root.child("state"),
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
                git_credentials: None,

                run_id: original.clone(),
                successor_run_id: successor.clone(),
                from: None,
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
async fn local_enumeration_and_readiness_fail_closed_without_controller() {
    let root = TestDirectory::new("ler");
    let backend = contract_backend(root.path()).with_ready_timeout(Duration::ZERO);
    let runs = root.child("runs");
    crate::execution::platform::private_directory(&runs).assert_value();
    std::fs::create_dir(runs.join("not-a-run")).assert_value();
    std::fs::write(runs.join("also-not-a-run"), b"ignored").assert_value();
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
fn local_recovery_lineage_is_bounded_and_honors_root_delivery() {
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
async fn local_stale_state_and_readiness_fail_closed() {
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

    let claim = backend.claim_recovery_workspace(&run_id).assert_value();
    assert!(
        !backend
            .recovery_workspace_is_unclaimed(&run_id)
            .assert_value()
    );
    drop(claim);
    assert!(
        backend
            .recovery_workspace_is_unclaimed(&run_id)
            .assert_value()
    );

    let no_ledger = RunId::new("0199f33f-3b44-7d21-9000-000000000072");
    backend.create_run_storage(&no_ledger).assert_value();
    let submission = contract_submission("missing-ledger");
    let digest = submission_digest(&submission).assert_value();
    assert!(
        backend
            .matching_submission(no_ledger, &submission.submission_key, &digest)
            .await
            .assert_value()
            .is_none()
    );

    let unreadable_root = TestDirectory::new("local-wave8-unreadable-runs");
    std::fs::write(unreadable_root.child("runs"), b"not a directory").assert_value();
    assert!(
        contract_backend(unreadable_root.path())
            .list_local()
            .await
            .is_err()
    );
    assert!(require_local(None).is_ok());
    assert!(require_local(Some("prod")).is_err());

    #[cfg(unix)]
    {
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
        crate::execution::process::write_new_file(
            &paths.startup_failure(),
            b"portable controller I/O failed: restic is unavailable",
            0o600,
        )
        .assert_value();
        assert_eq!(
            wait_for_controller(&mut exited, &paths, &run_id, Duration::ZERO)
                .await
                .assert_error()
                .to_string(),
            "local controller operation failed: controller exited before becoming ready: \
             portable controller I/O failed: restic is unavailable"
        );
        assert!(!paths.startup_failure().exists());

        let mut waiting = tokio::process::Command::new("sh")
            .args(["-c", "read _"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .assert_value();
        assert!(
            wait_for_controller(&mut waiting, &paths, &run_id, Duration::ZERO)
                .await
                .assert_error()
                .to_string()
                .contains("before the deadline")
        );
        waiting.kill().await.assert_value();
    }
}

async fn observable_local_run(
    label: &str,
    run_id: &str,
) -> (TestDirectory, LocalCliBackend, RunId, SqliteRunLedger) {
    // Local controller sockets have a small Unix path budget, so callers use short unique labels.
    let root = TestDirectory::new(label);
    let backend = contract_backend(root.path()).with_ready_timeout(Duration::ZERO);
    let run_id = RunId::new(run_id);
    let submission = contract_submission("observer-recovery");
    let ledger = create_contract_run(&backend, &run_id, &submission).await;
    ledger
        .append(
            &run_id,
            vec![
                crate::v2_run_ledger::RunEvent::RunStarted,
                crate::v2_run_ledger::RunEvent::Terminal {
                    result: TerminalResult::Succeeded {
                        output: serde_json::Value::Null,
                    },
                },
            ],
        )
        .await
        .assert_value();
    (root, backend, run_id, ledger)
}

#[tokio::test]
async fn local_status_observes_a_durable_run_without_a_controller_process() {
    let (_root, backend, run_id, _ledger) =
        observable_local_run("l9s", "0199f33f-3b44-7d21-9000-000000000091").await;
    let status = backend
        .status_local(RunStatusParams {
            run_id: run_id.clone(),
        })
        .await;
    assert!(status.is_ok(), "observer status failed: {status:?}");
    let status = status.assert_value();
    assert_eq!(status.run_id, run_id);
}

#[tokio::test]
async fn local_list_observes_a_durable_run_without_a_controller_process() {
    let (_root, backend, run_id, _ledger) =
        observable_local_run("l9l", "0199f33f-3b44-7d21-9000-000000000092").await;
    let listed = backend.list_local().await.assert_value();
    assert_eq!(listed.runs.len(), 1);
    assert_eq!(listed.runs[0].run_id, run_id);

    let invalid_root = TestDirectory::new("local-wave9-invalid-runs");
    std::fs::write(invalid_root.child("runs"), b"not a directory").assert_value();
    assert!(
        contract_backend(invalid_root.path())
            .local_run_ids()
            .is_err()
    );
    assert!(remove_stale_bootstrap(&invalid_root.child("bootstrap\0"), Duration::ZERO).is_err());
}

#[cfg(unix)]
async fn assert_local_start_deduplicates_before_controller_launch() {
    let root = TestDirectory::new("local-wave11-deduplicate");
    let repository = root.child("repository");
    std::fs::create_dir(&repository).assert_value();
    initialize_local_repository(&repository);

    let state = root.child("state");
    let backend = LocalCliBackend::new(
        state,
        PathBuf::from("/bin/true"),
        repository.clone(),
        PathBuf::from("git"),
    )
    .with_ready_timeout(Duration::ZERO);
    let existing = RunId::new("0199f33f-3b44-7d21-9000-0000000000a1");
    let duplicate = RunId::new("0199f33f-3b44-7d21-9000-0000000000a2");
    let prepared = prepare_local_run(
        prepared_request(existing.clone(), "wave11-deduplicate"),
        &repository,
        Path::new("git"),
    )
    .assert_value();
    create_contract_run(&backend, &existing, &prepared.submission).await;
    backend
        .write_recovery_document(
            &existing,
            &recovery_document(
                prepared.submission.clone(),
                prepared.workspace.clone(),
                None,
            ),
        )
        .assert_value();

    assert_eq!(
        backend
            .start_controller(prepared_request(duplicate, "wave11-deduplicate"))
            .await
            .assert_value(),
        existing
    );

    let absent = contract_submission("wave11-absent-key");
    let absent_digest = submission_digest(&absent).assert_value();
    assert!(
        backend
            .existing_submission(&absent.submission_key, &absent_digest, root.path())
            .await
            .assert_value()
            .is_none()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn slow_remote_preflight_does_not_block_unrelated_submission() {
    let root = TestDirectory::new("local-slow-remote-preflight");
    let remote_workspace = root.child("remote-workspace");
    let unrelated_workspace = root.child("unrelated-workspace");
    std::fs::create_dir(&remote_workspace).assert_value();
    std::fs::create_dir(&unrelated_workspace).assert_value();
    initialize_local_repository(&remote_workspace);
    initialize_local_repository(&unrelated_workspace);

    let state = root.child("shared-state");
    std::fs::create_dir(&state).assert_value();
    let git_program = root.child("blocking-git");
    let remote_check_started = root.child("remote-check-started");
    let release_remote_check = root.child("release-remote-check");
    openengine_cluster_testkit::fixture::write_executable(
        &git_program,
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"ls-remote\" ]; then\n    : > '{}'\n    while [ ! -e '{}' ]; do sleep 0.01; done\n    exit 0\n  fi\ndone\nexec /usr/bin/git \"$@\"\n",
            remote_check_started.display(),
            release_remote_check.display(),
        ),
        0o700,
    )
    .assert_value();

    let slow_backend = LocalCliBackend::new(
        state.clone(),
        PathBuf::from("/bin/true"),
        remote_workspace,
        git_program,
    )
    .with_ready_timeout(Duration::from_millis(50));
    let mut slow_request = prepared_request(
        RunId::new("0199f33f-3b44-7d21-9000-0000000000b1"),
        "slow-remote-preflight",
    );
    slow_request.intent.graph = full_graph(vec![
        crate::native_v2_candidate::test_support::git_delivery_node(),
        success_node(),
    ]);
    slow_request.intent.runtime.nodes_mut().insert(
        NodeName::new("deliver").assert_value(),
        crate::native_v2_contract::NodeRuntimeBinding::GitDelivery {
            connections: Default::default(),
            pull_request_feedback: Default::default(),
        },
    );

    let slow_submission =
        tokio::spawn(async move { slow_backend.start_controller(slow_request).await });
    let reached_remote_check = tokio::time::timeout(Duration::from_secs(5), async {
        while !remote_check_started.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if reached_remote_check.is_err() {
        std::fs::write(&release_remote_check, b"release").assert_value();

        let slow_result_after_release =
            tokio::time::timeout(Duration::from_secs(5), slow_submission).await;

        panic!(
            "PR submission never reached the blocked remote-ref check; \
            result after releasing fixture: {slow_result_after_release:?}"
        );
    }

    let unrelated_backend = LocalCliBackend::new(
        state,
        PathBuf::from("/bin/true"),
        unrelated_workspace,
        PathBuf::from("git"),
    )
    .with_ready_timeout(Duration::from_millis(50));
    let unrelated_request = prepared_request(
        RunId::new("0199f33f-3b44-7d21-9000-0000000000b2"),
        "unrelated-submission",
    );
    let unrelated_result = tokio::time::timeout(
        Duration::from_secs(2),
        unrelated_backend.start_controller(unrelated_request),
    )
    .await;
    let remote_check_was_still_blocked = remote_check_started.exists()
        && !release_remote_check.exists()
        && !slow_submission.is_finished();

    // Always unblock the child before asserting, so a failed regression test cannot leave it
    // waiting on a fixture file until its ten-minute Git transport deadline.
    std::fs::write(&release_remote_check, b"release").assert_value();
    let slow_result = tokio::time::timeout(Duration::from_secs(5), slow_submission)
        .await
        .assert_value()
        .assert_value();

    assert!(
        remote_check_was_still_blocked,
        "the slow remote check unexpectedly finished before the unrelated submission"
    );
    assert!(
        matches!(unrelated_result, Ok(Err(NativeV2CliError::Local(_)))),
        "unrelated submission did not progress to controller startup while remote validation was blocked: {unrelated_result:?}"
    );
    assert!(
        matches!(slow_result, Err(NativeV2CliError::Usage(ref message)) if message.contains("remote does not have target branch `main`")),
        "the blocked remote fixture should report a missing target branch: {slow_result:?}"
    );
}

#[cfg(unix)]
async fn assert_successor_claim_and_submission_lock_edges(
    backend: &LocalCliBackend,
    root: &TestDirectory,
) {
    let successor = RunId::new("0199f33f-3b44-7d21-9000-0000000000a5");
    let successor_storage = backend.create_run_storage(&successor).assert_value();
    let successor_paths = PortableControllerPaths::new(successor_storage.clone());
    let lease = ControllerLease::acquire(successor_paths.lease()).assert_value();
    assert!(
        backend
            .successor_claim_is_live_or_admitted(&successor, &successor_storage,)
            .await
            .assert_value()
    );
    drop(lease);

    let blocked_state = root.child("blocked-state");
    std::fs::write(&blocked_state, b"not a directory").assert_value();
    assert!(
        LocalCliBackend::new(
            blocked_state,
            PathBuf::from("/bin/true"),
            root.path().to_owned(),
            PathBuf::from("git"),
        )
        .acquire_submission_lock()
        .await
        .is_err()
    );

    let absent_run = RunId::new("0199f33f-3b44-7d21-9000-0000000000a6");
    assert!(backend.claim_recovery_workspace(&absent_run).is_err());
    assert!(
        backend
            .recovery_workspace_is_unclaimed(&RunId::new("invalid-local-run"))
            .is_err()
    );

    let retry_backend = contract_backend(root.path()).with_ready_timeout(Duration::from_millis(25));
    let retry_run = RunId::new("0199f33f-3b44-7d21-9000-0000000000a7");
    let retry_storage = retry_backend.create_run_storage(&retry_run).assert_value();
    assert!(
        !retry_backend
            .successor_claim_is_live_or_admitted(&retry_run, &retry_storage)
            .await
            .assert_value()
    );

    let submission_lock =
        ControllerLease::acquire(root.path().join(SUBMISSION_LOCK_FILE)).assert_value();
    let waiting_backend = contract_backend(root.path());
    let waiting = tokio::spawn(async move { waiting_backend.acquire_submission_lock().await });
    tokio::time::sleep(Duration::from_millis(25)).await;
    drop(submission_lock);
    drop(
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .assert_value()
            .assert_value()
            .assert_value(),
    );

    assert!(retryable_controller_handoff(
        &PortableControllerError::Lease(ControllerLeaseError::Held)
    ));
    assert!(retryable_controller_handoff(&PortableControllerError::Io(
        std::io::Error::other("retryable handoff")
    )));
    assert!(!retryable_controller_handoff(
        &PortableControllerError::Readiness
    ));
}

#[cfg(unix)]
async fn assert_zero_deadline_cleanup_and_recovery_claim_edges() {
    let root = TestDirectory::new("l11c");
    let backend = LocalCliBackend::new(
        root.child("state"),
        PathBuf::from("/bin/true"),
        root.path().to_owned(),
        PathBuf::from("git"),
    )
    .with_ready_timeout(Duration::ZERO);
    let no_ready = RunId::new("0199f33f-3b44-7d21-9000-0000000000a3");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let error = backend
        .start_prepared_controller(prepared_local_run(
            no_ready.clone(),
            contract_submission("wave11-no-readiness"),
            workspace.clone(),
        ))
        .await
        .assert_error();
    assert!(matches!(error, NativeV2CliError::Local(_)));
    assert!(
        !backend
            .run_storage(&no_ready)
            .assert_value()
            .join(BOOTSTRAP_FILE)
            .exists()
    );

    let recovery_run = RunId::new("0199f33f-3b44-7d21-9000-0000000000a4");
    backend.create_run_storage(&recovery_run).assert_value();
    backend
        .write_recovery_document(
            &recovery_run,
            &recovery_document(contract_submission("wave11-unclaimed"), workspace, None),
        )
        .assert_value();
    assert!(
        backend
            .claimed_local_successor(&recovery_run)
            .assert_value()
            .is_none()
    );
    backend
        .reconcile_local_resume_claim(&recovery_run)
        .await
        .assert_value();
    assert_successor_claim_and_submission_lock_edges(&backend, &root).await;
}

#[cfg(unix)]
#[tokio::test]
async fn wave11_cli_contract_local_deduplication_and_zero_deadline_cleanup_are_exact() {
    assert_local_start_deduplicates_before_controller_launch().await;
    assert_zero_deadline_cleanup_and_recovery_claim_edges().await;
}

#[test]
fn local_review_tokens_are_bounded_and_lease_paths_stay_private() {
    assert!(checked_review_token(String::new()).is_err());
    assert!(checked_review_token("token\0suffix".to_owned()).is_err());
    assert!(checked_review_token("t".repeat(4097)).is_err());
    assert_eq!(
        checked_review_token("t".repeat(4096)).assert_value(),
        "t".repeat(4096)
    );

    let root = TestDirectory::new("local-review-token");
    let state = root.child("state");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let lease = local_workspace_lease_path(&state, &workspace).assert_value();
    assert!(lease.starts_with(state.join("workspaces")));
    assert_eq!(
        local_workspace_lease_path(&state, &workspace).assert_value(),
        lease,
        "one workspace must always map to the same lease path"
    );

    let run_id = RunId::new("0199f33f-3b44-7d21-9000-0000000000ff");
    let storage = create_local_run_storage(&state, &run_id).assert_value();
    assert_eq!(storage, state.join("runs").join(run_id.as_str()));
    assert!(storage.is_dir());
    assert!(create_local_run_storage(&state, &RunId::new("invalid-local-run")).is_err());
}

#[test]
fn local_bootstrap_scavenging_only_removes_stale_regular_files() {
    let root = TestDirectory::new("local-bootstrap-scavenge");
    let missing = root.child("missing-bootstrap");
    remove_stale_bootstrap(&missing, Duration::ZERO).assert_value();

    let fresh = root.child("fresh-bootstrap");
    std::fs::write(&fresh, b"token").assert_value();
    remove_stale_bootstrap(&fresh, Duration::from_secs(3600)).assert_value();
    assert!(fresh.is_file(), "a recent bootstrap is still in use");
    remove_stale_bootstrap(&fresh, Duration::ZERO).assert_value();
    assert!(!fresh.exists(), "an aged bootstrap is reclaimed");

    let directory = root.child("bootstrap-directory");
    std::fs::create_dir(&directory).assert_value();
    remove_stale_bootstrap(&directory, Duration::ZERO).assert_value();
    assert!(directory.is_dir(), "directories are never reclaimed");

    #[cfg(unix)]
    {
        let target = root.child("bootstrap-target");
        std::fs::write(&target, b"token").assert_value();
        let link = root.child("bootstrap-link");
        std::os::unix::fs::symlink(&target, &link).assert_value();
        remove_stale_bootstrap(&link, Duration::ZERO).assert_value();
        assert!(link.symlink_metadata().is_ok(), "links are never followed");
        assert!(target.is_file());
    }
}

#[test]
fn local_backend_guards_named_targets_and_controller_handoff_errors() {
    require_local(None).assert_value();
    assert!(require_local(Some("named-target")).is_err());

    assert!(retryable_controller_handoff(&PortableControllerError::Io(
        std::io::Error::other("busy")
    )));
    assert!(retryable_controller_handoff(
        &PortableControllerError::Lease(ControllerLeaseError::Held)
    ));
    assert!(!retryable_controller_handoff(
        &PortableControllerError::Readiness
    ));
    assert!(local_error("detail").to_string().contains("detail"));
    assert!(
        local_io(std::io::Error::other("io"))
            .to_string()
            .contains("io")
    );
}

#[test]
fn local_recovery_claims_are_exclusive_until_released() {
    let root = TestDirectory::new("local-recovery-claim-contract");
    let backend = contract_backend(root.path());
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-0000000000aa");
    backend.create_run_storage(&run_id).assert_value();
    assert_eq!(
        backend.recovery_path(&run_id).assert_value(),
        root.path()
            .join("runs")
            .join(run_id.as_str())
            .join(RECOVERY_FILE)
    );
    assert!(
        backend
            .recovery_workspace_is_unclaimed(&run_id)
            .assert_value()
    );

    let claim = backend.claim_recovery_workspace(&run_id).assert_value();
    assert!(
        !backend
            .recovery_workspace_is_unclaimed(&run_id)
            .assert_value()
    );
    drop(claim);
    assert!(
        backend
            .recovery_workspace_is_unclaimed(&run_id)
            .assert_value()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn local_controller_readiness_reports_exit_and_deadline() {
    let root = TestDirectory::new("local-controller-readiness");
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-0000000000bb");

    let exited = PortableControllerPaths::new(root.child("exited"));
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("exit 7")
        .spawn()
        .assert_value();
    let error = wait_for_controller(&mut child, &exited, &run_id, Duration::from_secs(30))
        .await
        .expect_err("an exited controller never becomes ready");
    assert!(error.to_string().contains("exited before becoming ready"));

    let waiting = PortableControllerPaths::new(root.child("waiting"));
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 60")
        .spawn()
        .assert_value();
    let error = wait_for_controller(&mut child, &waiting, &run_id, Duration::from_millis(50))
        .await
        .expect_err("a live controller must publish readiness before the deadline");
    assert!(error.to_string().contains("before the deadline"));
    child.kill().await.assert_value();
    child.wait().await.assert_value();
}
