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
    let (ledger, run_id) = ledger_run("failure-refresh").await;
    let worker = reference(&run_id, "worker", 1);
    let initial = ledger
        .append(&run_id, vec![RunEvent::RunStarted, started(&worker)])
        .await
        .assert_value();
    let service = NativeV2Observability::new(ledger.clone());
    service.track_runtime(&initial.snapshot).assert_value();
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
    assert!(matches!(terminal_result, TerminalResult::Failed { reason }
        if reason.as_str() == "runtime_failed"));
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
