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
        pending_restore: None,
    };
    fixture
        .backend
        .write_recovery_document(&fixture.id, &document)
        .unwrap();
    document
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
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn running_force_fences_continuation_until_the_controller_replies() {
    let fixture = Fixture::new(false, false).await;
    recovery(&fixture);
    let recorded = fixture
        .backend
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
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
    let force = fixture.backend.force_once(RunForceParams {
        run_id: fixture.id.clone(),
    });
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
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
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
            .force_once(RunForceParams {
                run_id: fixture.id.clone()
            })
            .await
            .is_err()
    );
    drop(claim);
    assert!(!controller.forced.load(Ordering::SeqCst));
}

#[tokio::test]
async fn accepted_continuation_preserves_suspended_source_and_bypasses_live_controller() {
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
    let forced = fixture
        .backend
        .force_once(RunForceParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(forced.status, RunStatus::Suspended { .. }));
    assert!(!controller.forced.load(Ordering::SeqCst));
    assert_eq!(before, tree(fixture.root.path()));
}
