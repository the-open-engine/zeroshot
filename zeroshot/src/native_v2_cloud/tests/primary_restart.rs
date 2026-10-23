use super::*;
use crate::full_v1_reducer::{ExecutionId, NodeInstanceId};
use crate::native_v2_contract::{ExecutionRef, NodeCompletion, TokenUsageDelta};
use crate::v2_run_ledger::RunEvent;
use openengine_cluster_protocol::TokenCount;

fn outcome(failed: bool) -> TerminalResult {
    if failed {
        TerminalResult::Failed {
            reason: EnumLabel::new("work_rejected").assert_value(),
            context: None,
        }
    } else {
        TerminalResult::Succeeded {
            output: Value::Null,
        }
    }
}

async fn seed_tail(
    ledger: &FakeRunLedger,
    key: &str,
    failed: bool,
    pending: bool,
) -> RunSubmitParams {
    let mut request = request_with_key(Value::Null, key);
    request.submission.source = None;
    request
        .submission
        .runtime
        .nodes_mut()
        .retain(|name, _| name.as_str() == "worker");
    let end = if failed {
        json!({"kind":"fail","name":"done","reason":"work_rejected"})
    } else {
        crate::native_v2_candidate::test_support::success_node()
    };
    request.submission.graph = crate::native_v2_candidate::test_support::full_graph(vec![
        json!({"kind":"step","name":"worker","worker":"worker.test@1",
            "instructions":"Perform the task.","input":{"kind":"null"},"output":{"kind":"null"},
            "inputBindings":[],"writeBindings":[],"attempts":1}),
        end,
    ]);
    request.submission.initial_input = Value::Null;
    persist_request(ledger, &request).await;
    let reference = ExecutionRef {
        run_id: request.run_id.clone(),
        node: NodeName::new("worker").assert_value(),
        node_instance: NodeInstanceId::new(1).assert_value(),
        execution: ExecutionId::new(1).assert_value(),
    };
    let mut events = vec![
        RunEvent::RunStarted,
        crate::native_v2_runner::test_support::node_started(&reference, Value::Null),
        RunEvent::TokenUsageObserved {
            execution: reference.execution,
            usage: Some(tokens(5, 7)),
        },
        RunEvent::NodeCompleted {
            completion: NodeCompletion {
                reference,
                outcome: WorkerOutcome::Verified {
                    output: Value::Null,
                    artifacts: Vec::new(),
                },
            },
        },
        RunEvent::PrimaryGraphCompleted {
            result: outcome(failed),
            auxiliary_pending: pending,
        },
    ];
    if pending {
        events.push(RunEvent::RunTokenUsageObserved {
            usage: Some(tokens(11, 13)),
        });
    }
    ledger.append(&request.run_id, events).await.assert_value();
    request
}

fn tokens(input: u64, output: u64) -> TokenUsageDelta {
    TokenUsageDelta {
        input_tokens: TokenCount::new(input).assert_value(),
        output_tokens: TokenCount::new(output).assert_value(),
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    }
}

async fn stored_primary(ledger: &FakeRunLedger, run_id: &RunId) -> StoredRun {
    ledger.get(run_id).await.assert_value().assert_value()
}

async fn restart(fixture: &Harness) -> NativeV2CloudController {
    NativeV2CloudController::new(fixture.ledger.clone(), fixture.allocator.clone())
        .await
        .assert_value()
}

#[tokio::test]
async fn restart_preserves_primary_success_and_failure_with_incomplete_auxiliary_usage() {
    for failed in [false, true] {
        let fixture = harness(Behavior::Complete).await;
        let request = seed_tail(&fixture.ledger, "primary-restart", failed, true).await;
        let controller = restart(&fixture).await;
        assert_eq!(
            terminal(&controller, &request.run_id).await,
            outcome(failed)
        );
        let stored = stored_primary(&fixture.ledger, &request.run_id).await;
        let usage = stored.snapshot.token_usage.as_ref().assert_value();
        assert_eq!(usage.input_tokens.get(), 16);
        assert_eq!(usage.output_tokens.get(), 20);
        assert!(!usage.complete);
        assert!(
            !stored
                .snapshot
                .primary_graph
                .as_ref()
                .assert_value()
                .auxiliary_pending
        );
        assert_eq!(stored.snapshot.executions.len(), 1);
        assert_eq!(fixture.cleanup.terminal_seen(), vec![false]);
        assert_eq!(fixture.driver.starts.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.allocator.allocation_count(), 0);
        drop(controller);
        let _again = restart(&fixture).await;
        let replayed = stored_primary(&fixture.ledger, &request.run_id).await;
        assert_eq!(replayed.snapshot, stored.snapshot);
    }
}

#[tokio::test]
async fn restart_without_auxiliary_work_preserves_complete_usage() {
    let fixture = harness(Behavior::Complete).await;
    let request = seed_tail(&fixture.ledger, "primary-no-auxiliary", false, false).await;
    let controller = restart(&fixture).await;
    assert_eq!(terminal(&controller, &request.run_id).await, outcome(false));
    let stored = stored_primary(&fixture.ledger, &request.run_id).await;
    let usage = stored.snapshot.token_usage.assert_value();
    assert_eq!(usage.input_tokens.get(), 5);
    assert_eq!(usage.output_tokens.get(), 7);
    assert!(usage.complete);
}

#[tokio::test]
async fn primary_restart_waits_for_confirmed_cleanup_before_terminal_publication() {
    let fixture = harness(Behavior::Complete).await;
    let request = seed_tail(&fixture.ledger, "primary-cleanup", false, true).await;
    fixture.cleanup.fail_next();
    assert!(
        NativeV2CloudController::new(fixture.ledger.clone(), fixture.allocator.clone())
            .await
            .is_err()
    );
    let waiting = stored_primary(&fixture.ledger, &request.run_id).await;
    assert!(waiting.snapshot.terminal.is_none());
    assert!(
        waiting
            .snapshot
            .primary_graph
            .as_ref()
            .assert_value()
            .auxiliary_pending
    );
    let controller = restart(&fixture).await;
    assert_eq!(terminal(&controller, &request.run_id).await, outcome(false));
    assert_eq!(fixture.cleanup.terminal_seen(), vec![false, false]);
    assert_eq!(fixture.driver.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exact_resubmission_preserves_orphaned_primary_result_without_dispatch() {
    let fixture = harness(Behavior::Complete).await;
    // The seeded graph has no delivery node, matching a local controller's admission policy.
    let controller = NativeV2CloudController::new_with_delivery_policy(
        fixture.ledger.clone(),
        fixture.allocator.clone(),
        DeliveryPolicy::Optional,
    )
    .await
    .assert_value();
    let request = seed_tail(&fixture.ledger, "primary-orphan", true, true).await;
    let receipt = submit_test_request(&controller, request.clone())
        .await
        .assert_value();
    assert!(receipt.deduped);
    assert_eq!(terminal(&controller, &request.run_id).await, outcome(true));
    assert_eq!(fixture.driver.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.allocator.allocation_count(), 0);
}

#[tokio::test]
async fn force_of_orphaned_primary_result_preserves_main_outcome_after_cleanup() {
    let fixture = harness(Behavior::Complete).await;
    let request = seed_tail(&fixture.ledger, "primary-force", false, true).await;
    let result = fixture
        .controller
        .force(RunForceParams {
            run_id: request.run_id.clone(),
        })
        .await
        .assert_value();
    assert!(
        matches!(result.status, RunStatus::Finished { terminal_result, .. }
        if terminal_result == outcome(false))
    );
    let stored = stored_primary(&fixture.ledger, &request.run_id).await;
    assert!(!stored.snapshot.token_usage.as_ref().assert_value().complete);
    assert_eq!(fixture.cleanup.exits(), vec![RunRuntimeExit::ForceStopped]);
    assert_eq!(fixture.cleanup.terminal_seen(), vec![false]);
}
