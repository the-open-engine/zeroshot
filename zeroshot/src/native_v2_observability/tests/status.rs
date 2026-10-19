use super::*;

#[tokio::test]
async fn status_lists_every_parallel_execution_with_opaque_selectors() {
    let (ledger, run_id) = ledger_run("parallel-status").await;
    let left = reference(&run_id, "left", 1);
    let right = reference(&run_id, "right", 2);
    ledger
        .append(
            &run_id,
            vec![RunEvent::RunStarted, started(&left), started(&right)],
        )
        .await
        .assert_value();
    let service = NativeV2Observability::new(ledger);

    let status = service
        .status(RunStatusParams {
            run_id: run_id.clone(),
        })
        .await
        .assert_value();
    let admitted = admitted_run();
    assert_eq!(&status.title, &admitted.title);
    assert_eq!(&status.source, &admitted.source);
    assert_eq!(status.size, admitted.runtime.size());
    let active_executions = match status.status {
        RunStatus::Running { active_executions } => Some(active_executions),
        _ => None,
    };
    let active_executions = active_executions.assert_value_with("run must be running");
    assert_eq!(active_executions.len(), 2);
    assert_eq!(active_executions.assert_at(0).node.as_str(), "left");
    assert_eq!(active_executions.assert_at(1).node.as_str(), "right");
    assert_ne!(
        active_executions.assert_at(0).execution,
        active_executions.assert_at(1).execution
    );
    let encoded = serde_json::to_string(&active_executions).assert_value();
    assert!(!encoded.contains("nodeInstance"));
    assert!(!encoded.contains("executionId"));
    assert!(!encoded.contains(run_id.as_str()));
}

pub(super) async fn cursor_fixture() -> (
    Arc<FakeRunLedger>,
    RunId,
    ExecutionRef,
    ExecutionRef,
    NativeV2Observability,
) {
    let (ledger, run_id) = ledger_run("cursor-resume").await;
    let left = reference(&run_id, "left", 1);
    let right = reference(&run_id, "right", 2);
    ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                started(&left),
                started(&right),
                RunEvent::SafeLog {
                    diagnostic: None,
                    execution: Some(left.execution),
                    timestamp: openengine_cluster_protocol::UnixTimestampMillis::new(
                        1_725_000_000_123,
                    )
                    .assert_value(),
                    stream: SafeLogStream::Output,
                    line: SafeLogLine::new("first").assert_value(),
                },
                completed(&left, Value::Null),
                RunEvent::SafeLog {
                    diagnostic: None,
                    execution: Some(left.execution),
                    timestamp: openengine_cluster_protocol::UnixTimestampMillis::new(
                        1_725_000_000_456,
                    )
                    .assert_value(),
                    stream: SafeLogStream::Output,
                    line: SafeLogLine::new("second").assert_value(),
                },
            ],
        )
        .await
        .assert_value();
    let service = NativeV2Observability::new(ledger.clone());
    (ledger, run_id, left, right, service)
}

use openengine_cluster_testkit::assertions::{AssertAt, AssertValue};

#[tokio::test]
async fn failure_fallback_refreshes_usage_and_yields_to_a_durable_terminal() {
    let (ledger, worker, _initial, service) = tracked_worker_run("failure-refresh").await;
    let run_id = worker.run_id.clone();
    service.runtime_failed(&run_id);

    let updated = ledger
        .append(
            &run_id,
            vec![RunEvent::TokenUsageObserved {
                execution: worker.execution,
                usage: Some(TokenUsageDelta {
                    input_tokens: TokenCount::new(17).assert_value(),
                    output_tokens: TokenCount::new(4).assert_value(),
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                }),
            }],
        )
        .await
        .assert_value();
    service.refresh_runtime(&run_id).await;
    let params = RunStatusParams {
        run_id: run_id.clone(),
    };
    let fallback = service.status(params.clone()).await.assert_value();
    assert_eq!(fallback.at_cursor, updated.snapshot.cursor);
    let RunStatus::Finished {
        terminal_result,
        metadata,
    } = fallback.status
    else {
        panic!("storage refresh must retain the known failure");
    };
    assert!(
        matches!(terminal_result, TerminalResult::Failed { reason, .. }
        if reason.as_str() == "runtime_failed")
    );
    let usage = metadata.token_usage.assert_value();
    assert_eq!(usage.input_tokens.get(), 17);
    assert_eq!(usage.output_tokens.get(), 4);

    finish_worker_run(ledger.as_ref(), &worker).await;
    service.refresh_runtime(&run_id).await;
    let terminal = service.status(params.clone()).await.assert_value();
    assert_eq!(terminal.at_cursor.as_str(), "v2:5");
    assert!(matches!(
        terminal.status,
        RunStatus::Finished {
            terminal_result: TerminalResult::Succeeded { .. },
            ..
        }
    ));
    service.runtime_failed(&run_id);
    assert_eq!(service.status(params).await.assert_value(), terminal);
}

#[tokio::test]
async fn unpersisted_failure_preserves_first_safe_cause_without_fabricating_history() {
    let (ledger, run_id) = ledger_run("unpersisted-context").await;
    let initial = ledger
        .append(&run_id, vec![RunEvent::RunStarted])
        .await
        .assert_value();
    let service = NativeV2Observability::new(ledger.clone());
    service.track_runtime(&initial.snapshot).assert_value();
    service.runtime_failed_with_cause(&run_id, "could not persist run history");
    service.runtime_failed_with_cause(&run_id, "secondary cleanup error");
    let status = service
        .status(RunStatusParams {
            run_id: run_id.clone(),
        })
        .await
        .assert_value();
    assert_eq!(status.at_cursor, initial.snapshot.cursor);
    let RunStatus::Finished {
        terminal_result:
            TerminalResult::Failed {
                context: Some(context),
                ..
            },
        ..
    } = status.status
    else {
        panic!("missing fallback context");
    };
    let primary = context.primary.assert_value();
    assert!(
        primary
            .message
            .as_str()
            .contains("could not persist run history")
    );
    assert!(primary.cursor.is_none() && context.incomplete);
    let stored = ledger.get(&run_id).await.assert_value().assert_value();
    assert!(stored.snapshot.terminal.is_none());
    assert_eq!(stored.snapshot.cursor, initial.snapshot.cursor);
}

async fn tracked_worker_run(
    run: &str,
) -> (
    Arc<FakeRunLedger>,
    ExecutionRef,
    RunSnapshot,
    NativeV2Observability,
) {
    let (ledger, run_id) = ledger_run(run).await;
    let worker = reference(&run_id, "worker", 1);
    let initial = ledger
        .append(&run_id, vec![RunEvent::RunStarted, started(&worker)])
        .await
        .assert_value();
    let service = NativeV2Observability::new(ledger.clone());
    service.track_runtime(&initial.snapshot).assert_value();
    (ledger, worker, initial.snapshot, service)
}

fn recorded_failure(execution: Option<ExecutionId>, message: impl Into<String>) -> RunEvent {
    RunEvent::SafeLog {
        diagnostic: Some(openengine_cluster_protocol::FailureDiagnosticKind::Failure),
        execution,
        timestamp: openengine_cluster_protocol::UnixTimestampMillis::new(1).assert_value(),
        stream: SafeLogStream::Error,
        line: SafeLogLine::new(message).assert_value(),
    }
}

fn fallback_context(status: &RunStatusResult) -> &openengine_cluster_protocol::FailureContext {
    let RunStatus::Finished {
        terminal_result: TerminalResult::Failed { reason, context },
        ..
    } = &status.status
    else {
        panic!("expected runtime failure fallback");
    };
    assert_eq!(reason.as_str(), "runtime_failed");
    context.as_deref().assert_value()
}

#[tokio::test]
async fn failure_fallback_refreshes_recorded_evidence_without_replacing_its_first_cause() {
    let (ledger, worker, initial, service) = tracked_worker_run("failure-evidence-refresh").await;
    let run_id = worker.run_id.clone();
    service.runtime_failed_with_cause(&run_id, "first runtime failure");
    let params = RunStatusParams {
        run_id: run_id.clone(),
    };
    let original = service.status(params.clone()).await.assert_value();
    let primary = fallback_context(&original).primary.clone().assert_value();
    assert!(primary.cursor.is_none());

    let first = ledger
        .append(
            &run_id,
            vec![recorded_failure(Some(worker.execution), "worker failure 0")],
        )
        .await
        .assert_value();
    service.runtime.observe(&first.snapshot).assert_value();
    let first_status = service.status(params.clone()).await.assert_value();
    let first_context = fallback_context(&first_status);
    assert_eq!(first_context.recorded.len(), 1);
    assert!(first_context.incomplete);
    assert!(
        !first
            .snapshot
            .failure_context
            .as_deref()
            .assert_value()
            .incomplete
    );

    let mut logs = (1..7)
        .map(|index| recorded_failure(Some(worker.execution), format!("worker failure {index}")))
        .collect::<Vec<_>>();
    logs.push(recorded_failure(None, "later durable runtime explanation"));
    let updated = ledger.append(&run_id, logs).await.assert_value();
    let unchanged = updated.snapshot.clone();
    service.runtime.observe(&updated.snapshot).assert_value();
    service.runtime_failed_with_cause(&run_id, "secondary cleanup failure");
    let refreshed = service.status(params.clone()).await.assert_value();
    assert_eq!(refreshed.at_cursor, updated.snapshot.cursor);
    let context = fallback_context(&refreshed);
    assert_eq!(context.primary.as_ref(), Some(&primary));
    assert!(
        context.incomplete,
        "unpersisted cause must keep incomplete evidence explicit"
    );
    let durable_context = updated.snapshot.failure_context.as_deref().assert_value();
    assert_eq!(context.recorded, durable_context.recorded);
    assert_eq!(context.recorded.len(), 5);
    assert_ne!(context.primary, durable_context.primary);
    assert_eq!(
        updated.snapshot, unchanged,
        "observation must not rewrite source evidence"
    );
    let stored = ledger.get(&run_id).await.assert_value().assert_value();
    assert_eq!(stored.snapshot, unchanged);
    assert!(stored.snapshot.terminal.is_none());

    assert_eq!(service.runtime.observe(&initial).assert_value(), refreshed);
    let mut legacy = updated.snapshot.clone();
    legacy.failure_context = None;
    assert_eq!(service.runtime.observe(&legacy).assert_value(), refreshed);
    finish_worker_run(ledger.as_ref(), &worker).await;
    service.refresh_runtime(&run_id).await;
    let terminal = service.status(params.clone()).await.assert_value();
    assert!(matches!(
        terminal.status,
        RunStatus::Finished {
            terminal_result: TerminalResult::Succeeded { .. },
            ..
        }
    ));
    service.runtime_failed_with_cause(&run_id, "still later failure");
    assert_eq!(service.status(params).await.assert_value(), terminal);
}
