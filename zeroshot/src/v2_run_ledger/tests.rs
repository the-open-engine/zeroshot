use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use openengine_cluster_protocol::{
    ArtifactRef, CompiledGraphIr, Cursor, IdempotencyKey, MAX_SAFE_GENERATION, NodeName, Phase,
    PositiveInteger, RunId, Sha256Digest, RunSize, RunTitle, SourceBranchId, SourceRepositoryId,
    SourceRevisionId, ResolvedSource, TerminalResult, TokenCount, TokenUsage, UnixTimestampMillis,
    WorkerOutcome,
};
use serde_json::{Value, json};

use super::fake::FakeRunLedger;
use super::sqlite::SqliteRunLedger;
use super::{
    CreateRun, CreateRunOutcome, NodeState, RunEvent, RunLedger, RunLedgerError, RunPhase,
    RunSnapshot, SafeLogLine, SafeLogStream, MAX_SAFE_LOG_BYTES, apply_event, cursor_for,
    cursor_sequence,
};
use crate::full_v1_reducer::{ExecutionVoidReason, StructuralOccurrence};
use crate::native_v2_contract::{
    AdmittedRun, CodexProvider, ExecutionId, ExecutionRef, NodeCompletion, NodeInstanceId,
    RuntimePlan, TokenUsageDelta,
};

fn admitted_run() -> AdmittedRun {
    let graph: CompiledGraphIr = serde_json::from_str(include_str!(
        "../../../protocol/openengine-cluster/v1/fixtures/graph/canonical/base.json"
    ))
    .assert_value();
    AdmittedRun {
        title: RunTitle::new("Ledger test").assert_value(),
        graph,
        initial_input: Value::Null,
        runtime: RuntimePlan::Codex {
            provider: CodexProvider::OpenAi,
            size: RunSize::Medium,
            nodes: Default::default(),
        },
        source: ResolvedSource {
            repository: SourceRepositoryId::new("open-engine/zeroshot").assert_value(),
            branch: SourceBranchId::new("main").assert_value(),
            revision: SourceRevisionId::new("0123456789abcdef0123456789abcdef01234567")
                .assert_value(),
        },
    }
}

fn create(run: &str, key: &str, digest_byte: char) -> CreateRun {
    CreateRun {
        run_id: RunId::new(run),
        submission_key: IdempotencyKey::new(key).assert_value(),
        submission_digest: Sha256Digest::new(digest_byte.to_string().repeat(64)).assert_value(),
        admitted: admitted_run(),
    }
}

fn reference(run: &RunId, execution: u64) -> ExecutionRef {
    ExecutionRef {
        run_id: run.clone(),
        node: NodeName::new("worker").assert_value(),
        node_instance: NodeInstanceId::new(execution).assert_value(),
        execution: ExecutionId::new(execution).assert_value(),
    }
}

fn started(reference: ExecutionRef) -> RunEvent {
    RunEvent::NodeStarted {
        reference,
        occurrence: StructuralOccurrence {
            node: NodeName::new("worker").assert_value(),
            map_indices: Vec::new(),
        },
        attempt: PositiveInteger::new(1).assert_value(),
        input: json!({ "request": "edit" }),
    }
}

fn completed(reference: ExecutionRef, output: Value) -> RunEvent {
    RunEvent::NodeCompleted {
        completion: NodeCompletion {
            reference,
            outcome: WorkerOutcome::Verified {
                output,
                artifacts: Vec::new(),
            },
        },
    }
}

fn usage(
    input: u64,
    output: u64,
    cache_read: Option<u64>,
    cache_creation: Option<u64>,
) -> TokenUsageDelta {
    TokenUsageDelta {
        input_tokens: TokenCount::new(input).assert_value(),
        output_tokens: TokenCount::new(output).assert_value(),
        cache_read_input_tokens: cache_read.map(|value| TokenCount::new(value).assert_value()),
        cache_creation_input_tokens: cache_creation
            .map(|value| TokenCount::new(value).assert_value()),
    }
}

fn projected_snapshot(run: &str) -> RunSnapshot {
    let admitted = admitted_run();
    RunSnapshot::admitted(RunId::new(run), &admitted)
}

fn safe_log(execution: Option<ExecutionId>) -> RunEvent {
    RunEvent::SafeLog {
        execution,
        timestamp: UnixTimestampMillis::new(1_725_000_000_123).assert_value(),
        stream: SafeLogStream::System,
        line: SafeLogLine::new("projection check").assert_value(),
    }
}

fn assert_invalid_projection(snapshot: &mut RunSnapshot, event: RunEvent, expected: &'static str) {
    assert_eq!(
        apply_event(snapshot, &event, 99),
        Err(RunLedgerError::InvalidEvent(expected))
    );
}

#[test]
fn public_phase_and_safe_log_contracts_cover_every_wire_boundary() {
    for (phase, protocol, display) in [
        (RunPhase::Admitted, Phase::Admitting, "admitted"),
        (RunPhase::Running, Phase::Running, "running"),
        (RunPhase::Stopping, Phase::Running, "stopping"),
        (RunPhase::Finished, Phase::Finished, "finished"),
    ] {
        assert_eq!(phase.protocol_phase(), protocol);
        assert_eq!(phase.to_string(), display);
    }

    let boundary = "x".repeat(MAX_SAFE_LOG_BYTES);
    let line = SafeLogLine::new(boundary.clone()).assert_value();
    assert_eq!(line.as_str(), boundary);
    assert_eq!(
        SafeLogLine::new("x".repeat(MAX_SAFE_LOG_BYTES + 1)),
        Err(RunLedgerError::SafeLogTooLarge)
    );
    assert_eq!(
        SafeLogLine::new("before\0after"),
        Err(RunLedgerError::InvalidSafeLog)
    );
    assert!(serde_json::from_str::<SafeLogLine>(r#""before\u0000after""#).is_err());
}

fn active_snapshot(run: &str) -> (RunSnapshot, ExecutionRef) {
    let run_id = RunId::new(run);
    let reference = reference(&run_id, 1);
    let mut snapshot = projected_snapshot(run_id.as_str());
    apply_event(&mut snapshot, &RunEvent::RunStarted, 1).assert_value();
    apply_event(&mut snapshot, &started(reference.clone()), 2).assert_value();
    (snapshot, reference)
}

#[test]
fn state_projection_rejects_invalid_cursors_dispatches_and_log_references() {
    for cursor in ["v1:1", "v2:not-a-number", "v2:18446744073709551616"] {
        assert_eq!(
            cursor_sequence(&Cursor::new(cursor)),
            Err(RunLedgerError::InvalidCursor)
        );
    }

    let (mut snapshot, active) = active_snapshot("projection-boundaries");
    assert_invalid_projection(&mut snapshot, RunEvent::RunStarted, "run already started");
    assert_invalid_projection(
        &mut snapshot,
        started(active.clone()),
        "execution was already dispatched",
    );

    apply_event(&mut snapshot, &safe_log(None), 3).assert_value();
    apply_event(&mut snapshot, &safe_log(Some(active.execution)), 4).assert_value();
    assert_invalid_projection(
        &mut snapshot,
        safe_log(Some(ExecutionId::new(2).assert_value())),
        "log references an unknown execution",
    );
    assert_invalid_projection(
        &mut snapshot,
        RunEvent::Terminal {
            result: TerminalResult::Succeeded {
                output: Value::Null,
            },
        },
        "cannot finish with active executions",
    );
}

#[test]
fn state_projection_rejects_usage_and_settlement_identity_corruption() {
    let (mut snapshot, active) = active_snapshot("settlement-boundaries");
    let run_id = snapshot.run_id.clone();
    assert_invalid_projection(
        &mut snapshot,
        RunEvent::TokenUsageObserved {
            execution: ExecutionId::new(2).assert_value(),
            usage: None,
        },
        "token usage references an unknown execution",
    );
    apply_event(
        &mut snapshot,
        &RunEvent::TokenUsageObserved {
            execution: active.execution,
            usage: Some(usage(MAX_SAFE_GENERATION, 0, None, None)),
        },
        5,
    )
    .assert_value();
    assert_invalid_projection(
        &mut snapshot,
        RunEvent::TokenUsageObserved {
            execution: active.execution,
            usage: Some(usage(1, 0, None, None)),
        },
        "token usage overflow",
    );

    let mut mismatched = active.clone();
    mismatched.node_instance = NodeInstanceId::new(2).assert_value();
    assert_invalid_projection(
        &mut snapshot,
        completed(mismatched, Value::Null),
        "execution reference does not match dispatch",
    );
    let unknown = reference(&run_id, 2);
    assert_invalid_projection(
        &mut snapshot,
        completed(unknown, Value::Null),
        "execution was not dispatched",
    );

    apply_event(&mut snapshot, &completed(active.clone(), Value::Null), 6).assert_value();
    assert_invalid_projection(
        &mut snapshot,
        completed(active.clone(), Value::Null),
        "execution is already settled",
    );
    assert_invalid_projection(
        &mut snapshot,
        RunEvent::TokenUsageObserved {
            execution: active.execution,
            usage: None,
        },
        "token usage references a settled execution",
    );
    apply_event(
        &mut snapshot,
        &RunEvent::Terminal {
            result: TerminalResult::Succeeded {
                output: Value::Null,
            },
        },
        7,
    )
    .assert_value();
    assert_invalid_projection(&mut snapshot, safe_log(None), "run is already terminal");
}

#[test]
fn state_projection_rejects_dispatch_and_stop_replays_after_stop_intent() {
    let mut stopped = projected_snapshot("stopped-corrupt-replay");
    stopped.phase = RunPhase::Running;
    stopped.force_stop_requested = true;
    let stopped_run = stopped.run_id.clone();
    assert_invalid_projection(
        &mut stopped,
        started(reference(&stopped_run, 1)),
        "cannot dispatch after force-stop",
    );
    assert_invalid_projection(
        &mut stopped,
        RunEvent::ForceStopRequested,
        "force-stop was already requested",
    );
}

#[tokio::test]
async fn node_completion_with_artifact_reference_is_never_persisted() {
    let ledger = FakeRunLedger::new();
    let run_id = RunId::new("artifact-free-run");
    ledger
        .create_or_get(create(run_id.as_str(), "artifact-free", 'a'))
        .await
        .assert_value();
    let reference = reference(&run_id, 1);
    ledger
        .append(
            &run_id,
            vec![RunEvent::RunStarted, started(reference.clone())],
        )
        .await
        .assert_value();
    let artifact: ArtifactRef = serde_json::from_str(include_str!(
        "../../../protocol/openengine-cluster/v1/fixtures/graph/positive/artifact-ref.json"
    ))
    .assert_value();
    let rejected = ledger
        .append(
            &run_id,
            vec![RunEvent::NodeCompleted {
                completion: NodeCompletion {
                    reference: reference.clone(),
                    outcome: WorkerOutcome::Verified {
                        output: Value::Null,
                        artifacts: vec![artifact],
                    },
                },
            }],
        )
        .await;
    assert_eq!(
        rejected,
        Err(RunLedgerError::InvalidEvent(
            "native-v2 node outcomes cannot contain artifact references"
        ))
    );
    let stored = ledger.get(&run_id).await.assert_value().assert_value();
    assert!(matches!(
        stored
            .snapshot
            .executions
            .get(&reference.execution)
            .assert_value()
            .state,
        NodeState::Active
    ));
}

#[tokio::test]
async fn fake_create_is_exactly_idempotent_and_conflicts_fail_closed() {
    let ledger = FakeRunLedger::new();
    let request = create("run-1", "submission-1", 'a');

    assert!(matches!(
        ledger.create_or_get(request.clone()).await.assert_value(),
        CreateRunOutcome::Created(_)
    ));
    let existing = ledger
        .create_or_get(CreateRun {
            run_id: RunId::new("ignored-retry-id"),
            ..request.clone()
        })
        .await
        .assert_value();
    assert!(matches!(existing, CreateRunOutcome::Existing(_)));
    assert_eq!(existing.stored().snapshot.run_id, request.run_id);

    let conflict = ledger
        .create_or_get(CreateRun {
            submission_digest: Sha256Digest::new("b".repeat(64)).assert_value(),
            ..request.clone()
        })
        .await
        .assert_error();
    assert!(matches!(
        conflict,
        RunLedgerError::SubmissionConflict { .. }
    ));

    let run_id_conflict = ledger
        .create_or_get(create("run-1", "submission-2", 'c'))
        .await
        .assert_error();
    assert_eq!(run_id_conflict, RunLedgerError::RunIdConflict);
}

#[tokio::test]
async fn fake_projects_outputs_voids_safe_logs_and_terminal_in_order() {
    let ledger = FakeRunLedger::new();
    let request = create("run-events", "submission-events", 'd');
    let run_id = request.run_id.clone();
    ledger.create_or_get(request).await.assert_value();
    let first = reference(&run_id, 1);
    let second = reference(&run_id, 2);
    let appended = ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                started(first.clone()),
                started(second.clone()),
                RunEvent::SafeLog {
                    execution: Some(first.execution),
                    timestamp: UnixTimestampMillis::new(1_725_000_000_123).assert_value(),
                    stream: SafeLogStream::Output,
                    line: SafeLogLine::new("working").assert_value(),
                },
                completed(first.clone(), json!({ "changed": true })),
                RunEvent::ExecutionVoided {
                    reference: second.clone(),
                    reason: ExecutionVoidReason::ParallelJoin,
                },
                RunEvent::Terminal {
                    result: TerminalResult::Succeeded {
                        output: json!({ "merged": true }),
                    },
                },
            ],
        )
        .await
        .assert_value();
    assert_eq!(appended.events.len(), 7);
    assert_eq!(appended.snapshot.cursor, cursor_for(7));
    assert_eq!(appended.snapshot.phase, RunPhase::Finished);
    assert!(appended.snapshot.token_usage.is_none());
    assert!(matches!(
        appended
            .snapshot
            .executions
            .get(&first.execution)
            .assert_value()
            .state,
        NodeState::Completed { .. }
    ));
    assert!(matches!(
        appended
            .snapshot
            .executions
            .get(&second.execution)
            .assert_value()
            .state,
        NodeState::Voided {
            reason: ExecutionVoidReason::ParallelJoin,
            ..
        }
    ));

    let observation = ledger
        .snapshot_and_tail(&run_id, Some(&cursor_for(3)))
        .await
        .assert_value();
    assert_eq!(observation.snapshot.cursor, cursor_for(7));
    assert_eq!(observation.events.len(), 4);
    assert_eq!(observation.events.assert_at(0).cursor, cursor_for(4));
    assert_eq!(
        safe_log_timestamp(&observation.events.assert_at(0).event),
        Some(1_725_000_000_123)
    );

    assert_eq!(
        ledger
            .append(&run_id, vec![RunEvent::RunStarted])
            .await
            .assert_error(),
        RunLedgerError::InvalidEvent("run is already terminal")
    );
}

fn safe_log_timestamp(event: &RunEvent) -> Option<u64> {
    match event {
        RunEvent::SafeLog { timestamp, .. } => Some(timestamp.get()),
        _ => None,
    }
}

#[tokio::test]
async fn token_usage_sums_nodes_and_retries_while_preserving_missing_data() {
    let ledger = FakeRunLedger::new();
    let run_id = RunId::new("run-usage");
    let first = reference(&run_id, 1);
    let second = reference(&run_id, 2);
    ledger
        .create_or_get(create(run_id.as_str(), "submission-usage", '6'))
        .await
        .assert_value();

    let appended = ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                started(first.clone()),
                started(second.clone()),
                RunEvent::TokenUsageObserved {
                    execution: first.execution,
                    usage: Some(usage(10, 2, Some(4), Some(1))),
                },
                RunEvent::TokenUsageObserved {
                    execution: first.execution,
                    usage: Some(usage(3, 1, Some(2), None)),
                },
                RunEvent::TokenUsageObserved {
                    execution: second.execution,
                    usage: None,
                },
            ],
        )
        .await
        .assert_value();

    assert_eq!(
        appended.snapshot.token_usage,
        Some(TokenUsage {
            input_tokens: TokenCount::new(13).assert_value(),
            output_tokens: TokenCount::new(3).assert_value(),
            cache_read_input_tokens: Some(TokenCount::new(6).assert_value()),
            cache_creation_input_tokens: None,
            complete: false,
        })
    );
}

#[tokio::test]
async fn force_stop_is_idempotent_and_prevents_new_dispatches() {
    let ledger = FakeRunLedger::new();
    let request = create("run-stop", "submission-stop", 'e');
    let run_id = request.run_id.clone();
    ledger.create_or_get(request).await.assert_value();
    ledger
        .append(&run_id, vec![RunEvent::RunStarted])
        .await
        .assert_value();

    let first = ledger.request_force_stop(&run_id).await.assert_value();
    assert_eq!(first.events.len(), 1);
    assert!(first.snapshot.force_stop_requested);
    assert_eq!(first.snapshot.phase, RunPhase::Stopping);
    let second = ledger.request_force_stop(&run_id).await.assert_value();
    assert!(second.events.is_empty());
    assert_eq!(second.snapshot.cursor, first.snapshot.cursor);

    assert_eq!(
        ledger
            .append(&run_id, vec![started(reference(&run_id, 1))])
            .await
            .assert_error(),
        RunLedgerError::InvalidEvent("run is not dispatchable")
    );
}

#[tokio::test]
async fn parallel_completions_receive_one_durable_order() {
    let ledger = FakeRunLedger::new();
    let request = create("run-parallel", "submission-parallel", '7');
    let run_id = request.run_id.clone();
    let first = reference(&run_id, 1);
    let second = reference(&run_id, 2);
    ledger.create_or_get(request).await.assert_value();
    ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                started(first.clone()),
                started(second.clone()),
            ],
        )
        .await
        .assert_value();

    let first_completion = vec![completed(first.clone(), json!("first"))];
    let second_completion = vec![completed(second.clone(), json!("second"))];
    let (first_result, second_result) = tokio::join!(
        ledger.append(&run_id, first_completion),
        ledger.append(&run_id, second_completion),
    );
    let first_result = first_result.assert_value();
    let second_result = second_result.assert_value();
    let mut completion_cursors = [
        first_result.events.assert_at(0).cursor.clone(),
        second_result.events.assert_at(0).cursor.clone(),
    ];
    completion_cursors.sort();
    assert_eq!(completion_cursors, [cursor_for(4), cursor_for(5)]);

    let observation = ledger
        .snapshot_and_tail(&run_id, Some(&cursor_for(3)))
        .await
        .assert_value();
    assert_eq!(observation.snapshot.cursor, cursor_for(5));
    assert_eq!(observation.events.len(), 2);
    assert!(matches!(
        observation
            .snapshot
            .executions
            .get(&first.execution)
            .assert_value()
            .state,
        NodeState::Completed { .. }
    ));
    assert!(matches!(
        observation
            .snapshot
            .executions
            .get(&second.execution)
            .assert_value()
            .state,
        NodeState::Completed { .. }
    ));
}

#[tokio::test]
async fn sqlite_survives_reopen_and_preserves_cursor_tail_and_identity() {
    let path = unique_database_path();
    let request = create("run-sqlite", "submission-sqlite", 'f');
    let run_id = request.run_id.clone();
    {
        let ledger = SqliteRunLedger::open(&path).assert_value();
        ledger.create_or_get(request.clone()).await.assert_value();
        ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    started(reference(&run_id, 1)),
                    RunEvent::SafeLog {
                        execution: Some(ExecutionId::new(1).assert_value()),
                        timestamp: UnixTimestampMillis::new(1_725_000_000_123).assert_value(),
                        stream: SafeLogStream::System,
                        line: SafeLogLine::new("persisted timestamp").assert_value(),
                    },
                    RunEvent::TokenUsageObserved {
                        execution: ExecutionId::new(1).assert_value(),
                        usage: Some(usage(5, 1, None, None)),
                    },
                    completed(reference(&run_id, 1), json!("done")),
                ],
            )
            .await
            .assert_value();
    }

    let reopened = SqliteRunLedger::open(&path).assert_value();
    let stored = reopened.get(&run_id).await.assert_value().assert_value();
    assert_eq!(stored.snapshot.cursor, cursor_for(5));
    let persisted_usage = stored.snapshot.token_usage.assert_value();
    assert_eq!(persisted_usage.input_tokens.get(), 5);
    assert_eq!(persisted_usage.output_tokens.get(), 1);
    assert!(persisted_usage.complete);
    assert!(matches!(
        reopened.create_or_get(request).await.assert_value(),
        CreateRunOutcome::Existing(_)
    ));
    let observation = reopened
        .snapshot_and_tail(&run_id, Some(&cursor_for(1)))
        .await
        .assert_value();
    assert_eq!(observation.events.len(), 4);
    assert_eq!(observation.events.assert_at(0).cursor, cursor_for(2));
    let persisted_log = observation
        .events
        .iter()
        .find_map(|event| match &event.event {
            RunEvent::SafeLog {
                timestamp, line, ..
            } => Some((*timestamp, line)),
            _ => None,
        })
        .assert_value();
    assert_eq!(persisted_log.0.get(), 1_725_000_000_123);
    assert_eq!(persisted_log.1.as_str(), "persisted timestamp");

    drop(reopened);
    std::fs::remove_file(&path).assert_value();
}

fn unique_database_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .assert_value()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "zeroshot-v2-run-ledger-{}-{nonce}.sqlite",
        std::process::id()
    ))
}

use openengine_cluster_testkit::assertions::{AssertAt, AssertError, AssertValue};

#[tokio::test]
async fn prerequisites_are_replayable_but_never_counted_as_new_work() {
    use crate::full_v1_reducer::{DurableExecution, DurableExecutionState, HistoryPosition};
    let ledger = FakeRunLedger::new();
    let run = RunId::new("seeded-ledger");
    ledger
        .create_or_get(create(run.as_str(), "seeded-ledger", '7'))
        .await
        .assert_value();
    let original = reference(&run, 1);
    let prior = DurableExecution {
        dispatch_position: HistoryPosition::new(1).assert_value(),
        node_instance: original.node_instance,
        execution: original.execution,
        occurrence: StructuralOccurrence {
            node: original.node,
            map_indices: Vec::new(),
        },
        attempt: PositiveInteger::new(1).assert_value(),
        input: Value::Null,
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(2).assert_value(),
            outcome: WorkerOutcome::Verified {
                output: json!({"saved": true}),
                artifacts: Vec::new(),
            },
        },
    };
    let seed = RunEvent::PriorExecution {
        execution: prior.clone(),
    };
    let stored = ledger.append(&run, vec![seed.clone()]).await.assert_value();
    assert_eq!(stored.snapshot.execution_seed, vec![prior]);
    assert!(stored.snapshot.executions.is_empty());
    assert!(stored.snapshot.token_usage.is_none());
    assert!(ledger.append(&run, vec![seed.clone()]).await.is_err());
    ledger
        .append(&run, vec![RunEvent::RunStarted])
        .await
        .assert_value();
    assert!(ledger.append(&run, vec![seed]).await.is_err());
    assert!(
        ledger
            .append(&run, vec![started(reference(&run, 1))])
            .await
            .is_err()
    );
    let fresh = ledger
        .append(&run, vec![started(reference(&run, 2))])
        .await
        .assert_value();
    assert_eq!(fresh.snapshot.executions.len(), 1);
    let encoded = serde_json::to_vec(&fresh.snapshot).assert_value();
    let decoded: super::RunSnapshot = serde_json::from_slice(&encoded).assert_value();
    assert_eq!(decoded, fresh.snapshot);
}

#[path = "tests/checkpoint_bytes.rs"]
mod checkpoint_bytes;
