use super::*;

#[test]
fn primary_completion_requires_quiescence_is_immutable_and_prevents_new_dispatch() {
    let (mut snapshot, active) = active_snapshot("primary-latch");
    let primary = RunEvent::PrimaryGraphCompleted {
        result: TerminalResult::Succeeded {
            output: Value::Null,
        },
        auxiliary_pending: true,
        delivery_execution: None,
    };
    assert_invalid_projection(
        &mut snapshot,
        primary.clone(),
        "primary graph still has active executions",
    );
    apply_event(&mut snapshot, &completed(active, Value::Null), 3).assert_value();
    apply_event(&mut snapshot, &primary, 4).assert_value();
    assert!(snapshot.terminal.is_none());
    assert_eq!(snapshot.phase, RunPhase::Running);
    assert!(
        snapshot
            .primary_graph
            .as_ref()
            .assert_value()
            .auxiliary_pending
    );
    assert_invalid_projection(&mut snapshot, primary, "primary graph already completed");
    let next = reference(&snapshot.run_id, 2);
    assert_invalid_projection(&mut snapshot, started(next), "run is not dispatchable");
    assert!(snapshot.replay_seed().primary_graph.is_none());
    apply_event(&mut snapshot, &RunEvent::ForceStopRequested, 5).assert_value();
    apply_event(
        &mut snapshot,
        &RunEvent::Terminal {
            result: TerminalResult::Succeeded {
                output: Value::Null,
            },
        },
        6,
    )
    .assert_value();
    assert!(
        !snapshot
            .primary_graph
            .as_ref()
            .assert_value()
            .auxiliary_pending
    );
}

#[test]
fn primary_completion_is_optional_in_retained_legacy_snapshot_json() {
    let snapshot = projected_snapshot("legacy-primary");
    let mut encoded = serde_json::to_value(&snapshot).assert_value();
    encoded
        .as_object_mut()
        .assert_value()
        .remove("primaryGraph");
    let decoded: RunSnapshot = serde_json::from_value(encoded).assert_value();
    assert!(decoded.primary_graph.is_none());
    assert_eq!(decoded, snapshot);
}

#[tokio::test]
async fn sqlite_reopen_retains_checked_delivery_reference_and_native_receipt() {
    let path = unique_database_path();
    let request = create("delivery-reopen", "delivery-reopen", 'd');
    let run_id = request.run_id.clone();
    let reference = reference(&run_id, 1);
    let receipt = json!({"version":"v1","mode":"push","outcome":"pushed"});
    let result = TerminalResult::Succeeded {
        output: json!({"summary":"authored"}),
    };
    {
        let ledger = SqliteRunLedger::open(&path).assert_value();
        ledger.create_or_get(request).await.assert_value();
        ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    started(reference.clone()),
                    RunEvent::NodeCompleted {
                        completion: NodeCompletion {
                            reference: reference.clone(),
                            outcome: WorkerOutcome::Verifier {
                                output: receipt.clone(),
                                signals: Default::default(),
                                diagnostic: Value::Null,
                                artifacts: Vec::new(),
                            },
                        },
                    },
                    RunEvent::PrimaryGraphCompleted {
                        result: result.clone(),
                        auxiliary_pending: true,
                        delivery_execution: Some(reference.execution),
                    },
                ],
            )
            .await
            .assert_value();
    }
    {
        let ledger = SqliteRunLedger::open(&path).assert_value();
        let stored = ledger.get(&run_id).await.assert_value().assert_value();
        assert!(stored.snapshot.terminal.is_none());
        assert_eq!(stored.snapshot.delivery_receipt(), Some(receipt.clone()));
        ledger
            .append(
                &run_id,
                vec![RunEvent::Terminal {
                    result: result.clone(),
                }],
            )
            .await
            .assert_value();
    }
    {
        let ledger = SqliteRunLedger::open(&path).assert_value();
        let stored = ledger.get(&run_id).await.assert_value().assert_value();
        assert_eq!(stored.snapshot.terminal, Some(result));
        assert_eq!(stored.snapshot.delivery_receipt(), Some(receipt));
    }
    std::fs::remove_file(path).assert_value();
}
