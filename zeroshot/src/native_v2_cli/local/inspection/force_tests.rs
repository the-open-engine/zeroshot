use super::*;
use crate::native_v2_cli::local::local_contract_tests::contract_submission;
use openengine_cluster_protocol::{NodeName, RunSuspension};

fn recovery(fixture: &Fixture) -> LocalRecoveryDocument {
    let document = LocalRecoveryDocument {
        delivery_receipt: None,
        submission: contract_submission(fixture.id.as_str()),
        workspace: fixture.root.path().to_owned(),
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
    fixture
        .backend
        .write_recovery_document(&fixture.id, &document)
        .unwrap();
    document
}

async fn prepare_child(parent: &Fixture) -> Fixture {
    let backend = &parent.backend;
    let child = Fixture::in_state(false, false, Some(backend.state_root.clone())).await;
    let mut child_doc = recovery(&child);
    child_doc.resumed_from = Some(parent.id.clone());
    backend
        .write_recovery_document(&child.id, &child_doc)
        .unwrap();

    child
}

async fn linked_parts() -> (Fixture, Fixture) {
    let parent = Fixture::new(true, false).await;
    let child = prepare_child(&parent).await;
    let mut document = recovery(&parent);
    document.successor_run_id = Some(child.id.clone());
    parent
        .backend
        .write_recovery_document(&parent.id, &document)
        .unwrap();
    (parent, child)
}

#[tokio::test]
async fn linked_unreadable_part_uses_its_own_controller_for_status_and_force() {
    let (parent, child) = linked_parts().await;
    let backend = &parent.backend;
    let recorded = backend
        .read_recorded_status(RunStatusParams::new(child.id.clone()))
        .await
        .unwrap();
    let failed = failed(&recorded, "runtime_failed");
    let controller = LiveController::new(Some(failed.clone()), &recorded);
    let paths = backend.paths(&child.id).unwrap();
    let _endpoint = ServedEndpoint::start(&paths, controller.clone());
    std::fs::write(paths.ledger(), b"damaged ledger").unwrap();

    // An endpoint without this part's readiness cannot corroborate the handoff.
    let ready = std::fs::read(paths.ready()).unwrap();
    let mut foreign = read_ready(&paths).unwrap();
    foreign.run_id = parent.id.clone();
    std::fs::write(paths.ready(), serde_json::to_vec(&foreign).unwrap()).unwrap();
    assert!(
        backend
            .run_force(None, RunForceParams::new(parent.id.clone()))
            .await
            .is_err()
    );
    assert!(!controller.forced.load(Ordering::SeqCst));
    std::fs::write(paths.ready(), ready).unwrap();

    let status = backend
        .run_status(None, RunStatusParams::new(parent.id.clone()))
        .await
        .unwrap();
    assert_eq!(status.run_id, parent.id);
    assert_eq!(status.current_part_id, child.id);
    assert_eq!(status.part_id, child.id);
    assert_eq!(
        status.status,
        crate::native_v2_cli::CliRunStatus::Target(failed.status)
    );
    let mut stale = RunForceParams::new(parent.id.clone());
    stale.part_id = Some(parent.id.clone());
    assert!(matches!(backend.run_force(None, stale).await,
        Err(NativeV2CliError::Remote { code, .. }) if code == openengine_cluster_protocol::RUN_CONFLICT));
    assert!(!controller.forced.load(Ordering::SeqCst));

    // A storage failure can remove the file entirely. The validated directory and matching
    // readiness still identify the exact controller independently of SQLite.
    std::fs::remove_file(paths.ledger()).unwrap();
    let stopped = backend
        .run_force(None, RunForceParams::new(parent.id.clone()))
        .await
        .unwrap();
    assert_eq!(stopped.run_id, parent.id);
    assert_eq!(stopped.part_id, child.id);
    assert!(controller.forced.load(Ordering::SeqCst));
}

#[tokio::test]
async fn unreadable_ancestor_keeps_the_head_status_with_partial_usage() {
    use crate::native_v2_cli::CliRunStatus;
    let (parent, child) = linked_parts().await;
    let backend = &parent.backend;
    let child_paths = backend.paths(&child.id).unwrap();
    let recorded = backend
        .read_recorded_status(RunStatusParams::new(child.id.clone()))
        .await
        .unwrap();
    let mut live = failed(&recorded, "runtime_failed");
    let RunStatus::Finished { metadata, .. } = &mut live.status else {
        unreachable!()
    };
    metadata.token_usage = Some(
        serde_json::from_value(serde_json::json!({
            "inputTokens": 17, "outputTokens": 8, "cacheReadInputTokens": 3,
            "cacheCreationInputTokens": 5, "complete": true
        }))
        .unwrap(),
    );
    let controller = LiveController::new(Some(live), &recorded);
    let _endpoint = ServedEndpoint::start(&child_paths, controller.clone());
    let source = backend.paths(&parent.id).unwrap();
    std::fs::remove_file(source.ready()).unwrap();
    std::fs::write(source.ledger(), b"damaged ledger").unwrap();

    let status = backend
        .run_status(None, RunStatusParams::new(child.id.clone()))
        .await
        .unwrap();
    std::fs::remove_file(source.ledger()).unwrap();
    let stopped = backend
        .run_force(None, RunForceParams::new(parent.id.clone()))
        .await
        .unwrap();
    assert!(controller.forced.load(Ordering::SeqCst));
    for result in [status, stopped] {
        assert_eq!(result.run_id, parent.id);
        assert_eq!(result.part_id, child.id);
        let CliRunStatus::Target(RunStatus::Finished {
            terminal_result,
            metadata,
        }) = result.status
        else {
            panic!("expected live failure")
        };
        assert!(
            matches!(terminal_result, TerminalResult::Failed { reason, .. } if reason.as_str() == "runtime_failed")
        );
        let usage = metadata.token_usage.unwrap();
        assert_eq!(
            (usage.input_tokens.get(), usage.output_tokens.get()),
            (17, 8)
        );
        assert!(!usage.complete);
        assert_eq!(usage.cache_read_input_tokens, None);
        assert_eq!(usage.cache_creation_input_tokens, None);
    }
    // A readable database with a different run is an identity mismatch.
    std::fs::copy(child_paths.ledger(), source.ledger()).unwrap();
    assert!(
        backend
            .run_status(None, RunStatusParams::new(parent.id))
            .await
            .is_err()
    );
}

async fn suspend(fixture: &Fixture) -> RunStatusResult {
    let reference = test_support::request(fixture.id.as_str(), "worker", (1, 1))
        .invocation
        .reference;
    let paths = fixture.backend.paths(&fixture.id).unwrap();
    let ledger = SqliteRunLedger::open(paths.ledger()).unwrap();
    ledger
        .append(
            &fixture.id,
            vec![
                RunEvent::NodeCompleted {
                    completion: crate::native_v2_contract::NodeCompletion {
                        reference,
                        outcome: WorkerOutcome::Verified {
                            output: serde_json::Value::Null,
                            artifacts: vec![],
                        },
                    },
                },
                RunEvent::Suspended {
                    suspension: RunSuspension::Command {
                        node: NodeName::new("next").unwrap(),
                        created_at: UnixTimestampMillis::MIN,
                    },
                },
            ],
        )
        .await
        .unwrap();
    fixture
        .backend
        .read_recorded_status(RunStatusParams::new(fixture.id.clone()))
        .await
        .unwrap()
}

#[tokio::test]
async fn running_force_fences_continuation_until_the_controller_replies() {
    let fixture = Fixture::new(false, false).await;
    recovery(&fixture);
    let recorded = fixture
        .backend
        .read_recorded_status(RunStatusParams::new(fixture.id.clone()))
        .await
        .unwrap();
    assert!(matches!(recorded.status, RunStatus::Running { .. }));
    let gate = Arc::new(ForceGate::default());
    let controller = Arc::new(LiveController {
        status: Some(recorded.clone()),
        stopped: failed(&recorded, "force_stopped"),
        forced: AtomicBool::new(false),
        force_gate: Some(gate.clone()),
    });
    let _endpoint = ServedEndpoint::start(&fixture.backend.paths(&fixture.id).unwrap(), controller);
    let force = fixture
        .backend
        .force_once(RunForceParams::new(fixture.id.clone()));
    tokio::pin!(force);
    tokio::select! {
        result = &mut force => panic!("force returned before the controller reply: {result:?}"),
        () = gate.entered.notified() => {}
        () = tokio::time::sleep(Duration::from_secs(10)) => panic!("force never reached the controller"),
    }
    // Suspension may commit while force is in flight. Its continuation must not claim the
    // workspace before force finishes, even though force initially observed a running run.
    let suspended = suspend(&fixture).await;
    assert!(
        fixture
            .backend
            .claim_recovery_workspace(&fixture.id)
            .is_err()
    );
    assert!(
        !fixture
            .backend
            .workspace_recovery(&suspended)
            .await
            .unwrap()
            .recoverable
    );
    // Cancellation of this run does not serialize unrelated submissions.
    drop(
        tokio::time::timeout(
            Duration::from_secs(1),
            fixture.backend.acquire_submission_lock(),
        )
        .await
        .unwrap()
        .unwrap(),
    );
    gate.release.notify_one();
    force.await.unwrap();
    drop(
        fixture
            .backend
            .claim_recovery_workspace(&fixture.id)
            .unwrap(),
    );
}

#[tokio::test]
async fn continuation_claim_fences_force_even_while_recorded_status_is_running() {
    let fixture = Fixture::new(false, false).await;
    recovery(&fixture);
    let recorded = fixture
        .backend
        .read_recorded_status(RunStatusParams::new(fixture.id.clone()))
        .await
        .unwrap();
    assert!(matches!(recorded.status, RunStatus::Running { .. }));
    let controller = LiveController::new(Some(recorded.clone()), &recorded);
    let _endpoint = ServedEndpoint::start(
        &fixture.backend.paths(&fixture.id).unwrap(),
        controller.clone(),
    );
    let claim = fixture
        .backend
        .claim_recovery_workspace(&fixture.id)
        .unwrap();
    assert!(
        fixture
            .backend
            .force_once(RunForceParams::new(fixture.id.clone()))
            .await
            .is_err()
    );
    drop(claim);
    assert!(!controller.forced.load(Ordering::SeqCst));
}

#[tokio::test]
async fn public_force_rechecks_its_selected_part_after_waiting_for_a_handoff() {
    let parent = Fixture::new(false, false).await;
    let backend = &parent.backend;
    let child = prepare_child(&parent).await;
    let mut parent_doc = recovery(&parent);
    // Verify the child through its durable admission rather than its readiness marker.
    std::fs::remove_file(backend.paths(&child.id).unwrap().ready()).unwrap();
    suspend(&parent).await;

    let claim = backend.claim_recovery_workspace(&parent.id).unwrap();
    let mut observed = RunForceParams::new(parent.id.clone());
    observed.part_id = Some(parent.id.clone());
    let explicit = backend.run_force(None, observed);
    let implicit = backend.run_force(None, RunForceParams::new(parent.id.clone()));
    tokio::pin!(explicit, implicit);
    // The unlinked root is selected synchronously. Both callers must then wait for the claim.
    assert!(futures_util::poll!(&mut explicit).is_pending());
    assert!(futures_util::poll!(&mut implicit).is_pending());
    parent_doc.successor_run_id = Some(child.id.clone());
    backend
        .write_recovery_document(&parent.id, &parent_doc)
        .unwrap();
    drop(claim);

    let (explicit, implicit) = tokio::join!(explicit, implicit);
    for result in [explicit, implicit] {
        assert!(matches!(result,
            Err(NativeV2CliError::Remote { code, details: Some(details), .. })
                if code == openengine_cluster_protocol::RUN_CONFLICT && details["currentPartId"] == child.id.as_str()));
    }
    let child_status = backend
        .read_recorded_status(RunStatusParams::new(child.id))
        .await
        .unwrap();
    assert!(matches!(child_status.status, RunStatus::Running { .. }));
}

#[tokio::test]
async fn unconfirmed_successor_preserves_the_source_and_refuses_logical_force() {
    let fixture = Fixture::new(false, false).await;
    let mut document = recovery(&fixture);
    let suspended = suspend(&fixture).await;
    let controller = LiveController::new(Some(suspended.clone()), &suspended);
    let _endpoint = ServedEndpoint::start(
        &fixture.backend.paths(&fixture.id).unwrap(),
        controller.clone(),
    );
    let claim = fixture
        .backend
        .claim_recovery_workspace(&fixture.id)
        .unwrap();
    let successor = RunId::new(uuid::Uuid::now_v7().to_string());
    document.successor_run_id = Some(successor);
    fixture
        .backend
        .write_recovery_document(&fixture.id, &document)
        .unwrap();
    drop(claim);
    let before = tree(fixture.root.path());
    let error = fixture
        .backend
        .run_force(None, RunForceParams::new(fixture.id.clone()))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unconfirmed successor"),
        "{error}"
    );
    let forced = fixture
        .backend
        .force_once(RunForceParams::new(fixture.id.clone()))
        .await
        .unwrap();
    assert!(matches!(forced.status, RunStatus::Suspended { .. }));
    assert!(!controller.forced.load(Ordering::SeqCst));
    assert_eq!(before, tree(fixture.root.path()));
}
