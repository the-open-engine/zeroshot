use super::*;
use openengine_cluster_protocol::{FailureDiagnosticKind, WorkerErrorCode};

fn diagnostic(execution: Option<ExecutionId>, text: &str, kind: FailureDiagnosticKind) -> RunEvent {
    RunEvent::SafeLog {
        execution,
        timestamp: UnixTimestampMillis::MIN,
        stream: SafeLogStream::Error,
        line: SafeLogLine::new(text).assert_value(),
        diagnostic: Some(kind),
    }
}

fn failed(reason: &str) -> RunEvent {
    RunEvent::Terminal {
        result: TerminalResult::Failed {
            reason: reason.parse().assert_value(),
            context: None,
        },
    }
}

fn ledgers() -> Vec<Box<dyn RunLedger>> {
    vec![
        Box::new(FakeRunLedger::new()),
        Box::new(SqliteRunLedger::open_in_memory().assert_value()),
    ]
}

#[tokio::test]
async fn terminal_event_and_status_keep_same_evidence_even_when_the_worker_is_voided() {
    for ledger in ledgers() {
        let request = create("failure-context", "failure-key", 'd');
        let run_id = request.run_id.clone();
        ledger.create_or_get(request).await.assert_value();
        let worker = reference(&run_id, 1);
        let appended = ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    started(worker.clone()),
                    diagnostic(
                        Some(worker.execution),
                        "GitHub API rejected delivery: 404",
                        FailureDiagnosticKind::Failure,
                    ),
                    RunEvent::ExecutionVoided {
                        reference: worker.clone(),
                        reason: ExecutionVoidReason::ParallelJoin,
                    },
                    diagnostic(
                        None,
                        "Runtime failed: snapshot unavailable",
                        FailureDiagnosticKind::Failure,
                    ),
                    diagnostic(None, "Cleanup also failed", FailureDiagnosticKind::Failure),
                    failed("runtime_failed"),
                ],
            )
            .await
            .assert_value();
        let TerminalResult::Failed {
            context: Some(context),
            ..
        } = appended.snapshot.terminal.as_ref().assert_value()
        else {
            panic!("missing failure context");
        };
        assert_eq!(
            context.primary.as_ref().assert_value().message.as_str(),
            "Runtime failed: snapshot unavailable"
        );
        assert_eq!(
            context
                .primary
                .as_ref()
                .assert_value()
                .cursor
                .as_ref()
                .assert_value()
                .as_str(),
            "v2:5"
        );
        assert_eq!(context.recorded.len(), 2);
        assert_eq!(
            context.recorded[0]
                .execution
                .as_ref()
                .assert_value()
                .node
                .as_str(),
            "worker"
        );
        assert_eq!(
            context.recorded[0].cursor.as_ref().assert_value().as_str(),
            "v2:3"
        );
        assert!(matches!(
            appended.snapshot.executions[&worker.execution].state,
            NodeState::Voided { .. }
        ));
        let RunEvent::Terminal { result } = &appended.events.last().assert_value().event else {
            panic!("terminal event missing");
        };
        assert_eq!(Some(result), appended.snapshot.terminal.as_ref());
        assert_eq!(
            ledger
                .get(&run_id)
                .await
                .assert_value()
                .assert_value()
                .snapshot,
            appended.snapshot
        );
    }
}

#[tokio::test]
async fn recovered_evidence_and_diagnostic_loss_do_not_change_success_or_usage() {
    for ledger in ledgers() {
        let request = create("recovered-context", "recovered-key", 'e');
        let run_id = request.run_id.clone();
        ledger.create_or_get(request).await.assert_value();
        let worker = reference(&run_id, 1);
        let mut events = vec![RunEvent::RunStarted, started(worker.clone())];
        for index in 0..7 {
            events.push(diagnostic(
                Some(worker.execution),
                &format!("recovered {index}"),
                FailureDiagnosticKind::Failure,
            ));
        }
        events.extend([
            diagnostic(
                Some(worker.execution),
                "some diagnostic evidence was not retained",
                FailureDiagnosticKind::Incomplete,
            ),
            completed(worker.clone(), Value::Null),
            RunEvent::Terminal {
                result: TerminalResult::Succeeded {
                    output: Value::Null,
                },
            },
        ]);
        let appended = ledger.append(&run_id, events).await.assert_value();
        assert!(matches!(
            appended.snapshot.terminal,
            Some(TerminalResult::Succeeded { .. })
        ));
        assert!(appended.snapshot.token_usage.is_none());
        let context = appended.snapshot.failure_context.assert_value();
        assert!(context.primary.is_none() && context.incomplete);
        assert_eq!(context.recorded.len(), 5);
        assert_eq!(context.recorded[0].message.as_str(), "recovered 2");
    }
}

#[tokio::test]
async fn force_stop_does_not_invent_causality_and_invalid_diagnostic_batch_is_atomic() {
    let ledger = SqliteRunLedger::open_in_memory().assert_value();
    let request = create("stop-context", "stop-key", 'f');
    let run_id = request.run_id.clone();
    ledger.create_or_get(request).await.assert_value();
    let worker = reference(&run_id, 1);
    let initial = ledger
        .append(&run_id, vec![RunEvent::RunStarted, started(worker.clone())])
        .await
        .assert_value();
    let bad = RunEvent::SafeLog {
        execution: None,
        timestamp: UnixTimestampMillis::MIN,
        stream: SafeLogStream::System,
        line: SafeLogLine::new("not an Error record").assert_value(),
        diagnostic: Some(FailureDiagnosticKind::Failure),
    };
    assert!(
        ledger
            .append(
                &run_id,
                vec![
                    diagnostic(
                        Some(worker.execution),
                        "must roll back",
                        FailureDiagnosticKind::Failure
                    ),
                    bad
                ]
            )
            .await
            .is_err()
    );
    assert_eq!(
        ledger
            .get(&run_id)
            .await
            .assert_value()
            .assert_value()
            .snapshot,
        initial.snapshot
    );
    let result = ledger
        .append(
            &run_id,
            vec![
                diagnostic(
                    Some(worker.execution),
                    "an earlier worker failure",
                    FailureDiagnosticKind::Failure,
                ),
                RunEvent::NodeCompleted {
                    completion: NodeCompletion {
                        reference: worker,
                        outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
                    },
                },
                RunEvent::Terminal {
                    result: TerminalResult::Failed {
                        reason: "force_stopped".parse().assert_value(),
                        context: None,
                    },
                },
            ],
        )
        .await
        .assert_value();
    assert!(matches!(
        result.snapshot.terminal,
        Some(TerminalResult::Failed { context: None, .. })
    ));
}

#[tokio::test]
async fn escaped_failure_text_is_bounded_without_losing_the_explanation() {
    for ledger in ledgers() {
        let ledger: std::sync::Arc<dyn RunLedger> = std::sync::Arc::from(ledger);
        let request = create("control-context", "control-key", 'c');
        let run_id = request.run_id.clone();
        ledger.create_or_get(request).await.assert_value();
        let text = format!(
            "provider failed: {} actual cause at tail",
            "\u{1b}".repeat(8_000)
        );
        let stored = ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    diagnostic(None, &text, FailureDiagnosticKind::Failure),
                    failed("runtime_failed"),
                ],
            )
            .await
            .assert_value();
        let message = &stored
            .snapshot
            .failure_context
            .as_ref()
            .assert_value()
            .primary
            .as_ref()
            .assert_value()
            .message;
        assert!(message.as_str().starts_with("provider failed:"));
        assert!(message.as_str().ends_with("actual cause at tail"));
        assert!(!message.as_str().chars().any(char::is_control));
        let service = crate::native_v2_observability::NativeV2Observability::new(ledger);
        let (_, mut logs) = service
            .logs(openengine_cluster_protocol::RunLogsParams {
                run_id,
                from_cursor: None,
                execution: None,
            })
            .await
            .assert_value();
        assert_eq!(
            &logs
                .recv()
                .await
                .assert_value()
                .assert_value()
                .record
                .message,
            message
        );
    }
}

#[tokio::test]
async fn primary_failure_retains_its_diagnostics_across_auxiliary_interruption() {
    for ledger in ledgers() {
        let request = create("primary-context", "primary-context-key", 'a');
        let run_id = request.run_id.clone();
        ledger.create_or_get(request).await.assert_value();
        let worker = reference(&run_id, 1);
        let latched = ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    started(worker.clone()),
                    diagnostic(
                        Some(worker.execution),
                        "primary failure",
                        FailureDiagnosticKind::Failure,
                    ),
                    completed(worker, Value::Null),
                    RunEvent::PrimaryGraphCompleted {
                        result: TerminalResult::Failed {
                            reason: "work_rejected".parse().assert_value(),
                            context: None,
                        },
                        auxiliary_pending: true,
                    },
                ],
            )
            .await
            .assert_value();
        let primary = latched.snapshot.primary_graph.assert_value().result;
        let TerminalResult::Failed {
            context: Some(context),
            ..
        } = &primary
        else {
            panic!("primary failure must retain diagnostics before optional work");
        };
        assert_eq!(context.recorded[0].message.as_str(), "primary failure");
        let interrupted = ledger
            .append(
                &run_id,
                vec![
                    RunEvent::SafeLog {
                        execution: None,
                        timestamp: UnixTimestampMillis::MIN,
                        stream: SafeLogStream::Error,
                        line: SafeLogLine::new("postmortem unavailable").assert_value(),
                        diagnostic: None,
                    },
                    RunEvent::RunTokenUsageObserved { usage: None },
                    RunEvent::Terminal {
                        result: primary.clone(),
                    },
                ],
            )
            .await
            .assert_value();
        assert_eq!(interrupted.snapshot.terminal, Some(primary.clone()));
        let retained = interrupted.snapshot.primary_graph.assert_value();
        assert_eq!(retained.result, primary);
        assert!(!retained.auxiliary_pending);
        assert!(!interrupted.snapshot.token_usage.assert_value().complete);
        assert_eq!(
            ledger
                .get(&run_id)
                .await
                .assert_value()
                .assert_value()
                .snapshot
                .terminal,
            Some(primary)
        );
    }
}
