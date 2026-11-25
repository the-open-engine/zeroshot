use super::*;
use crate::native_v2_cli::{CliRunStatus, CliSubscription, CliSubscriptionItem};
use crate::native_v2_contract::{NodeCompletion, TokenUsageDelta};
use crate::native_v2_observability::lineage::{part_cursor, split_cursor};
use crate::native_v2_runner::test_support::{node_started, request};
use crate::v2_run_ledger::{RunEvent, SafeLogLine, SafeLogStream};
use openengine_cluster_protocol::{
    SubscriptionCloseReason, TokenCount, UnixTimestampMillis, WorkerOutcome,
};

/// Each part reuses the same node and execution numbers, as independent local ledgers do.
async fn record(
    backend: &LocalCliBackend,
    key: &str,
    predecessor: Option<&RunId>,
    end: Option<RunEvent>,
) -> RunId {
    let id = RunId::new(uuid::Uuid::now_v7().to_string());
    let submission = contract_submission(key);
    let ledger = create_contract_run(backend, &id, &submission).await;
    let reference = request(id.as_str(), "worker", (1, 1)).invocation.reference;
    let mut events = vec![RunEvent::RunStarted, node_started(&reference, json!(null))];
    for execution in [None, Some(reference.execution)] {
        events.push(RunEvent::SafeLog {
            execution,
            timestamp: UnixTimestampMillis::MIN,
            stream: SafeLogStream::Output,
            line: SafeLogLine::new(key).assert_value(),
            diagnostic: None,
        });
    }
    events.push(RunEvent::TokenUsageObserved {
        execution: reference.execution,
        usage: Some(TokenUsageDelta {
            input_tokens: TokenCount::new(7).assert_value(),
            output_tokens: Default::default(),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        }),
    });
    if let Some(end) = end {
        events.push(RunEvent::NodeCompleted {
            completion: NodeCompletion {
                reference,
                outcome: WorkerOutcome::Verified {
                    output: json!(null),
                    artifacts: Vec::new(),
                },
            },
        });
        events.push(end);
    }
    ledger.append(&id, events).await.assert_value();
    let recovery = recovery_document(
        submission,
        backend.current_directory.clone(),
        predecessor.cloned(),
    );
    backend
        .write_recovery_document(&id, &recovery)
        .assert_value();
    id
}

fn suspended() -> RunEvent {
    RunEvent::Suspended {
        suspension: serde_json::from_value(
            json!({"reason":"command", "node":"worker", "createdAt":1000}),
        )
        .assert_value(),
    }
}

fn finished() -> RunEvent {
    RunEvent::Terminal {
        result: TerminalResult::Succeeded {
            output: json!(null),
        },
    }
}

fn link(backend: &LocalCliBackend, source: &RunId, successor: &RunId) {
    let mut recovery = backend.read_recovery_document(source).assert_value();
    recovery.successor_run_id = Some(successor.clone());
    backend
        .write_recovery_document(source, &recovery)
        .assert_value();
}

fn watch(run: &RunId, part: Option<&RunId>, after: Option<&Cursor>) -> RunWatchParams {
    RunWatchParams {
        run_id: run.clone(),
        part_id: part.cloned(),
        from_cursor: after.cloned(),
    }
}

fn logs(run: &RunId, part: Option<&RunId>, after: Option<&Cursor>) -> RunLogsParams {
    RunLogsParams {
        run_id: run.clone(),
        part_id: part.cloned(),
        from_cursor: after.cloned(),
        execution: None,
    }
}

async fn next<E: Send>(stream: &mut ChannelSubscription<E>) -> CliSubscriptionItem<E> {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("stream should make progress")
        .assert_value()
        .expect("stream must deliver its close reason")
}

async fn done<E: Send>(stream: &mut ChannelSubscription<E>) -> Vec<E> {
    let mut events = Vec::new();
    loop {
        match next(stream).await {
            CliSubscriptionItem::Event(event) => events.push(event),
            CliSubscriptionItem::Closed { reason } => {
                assert_eq!(reason, SubscriptionCloseReason::Done);
                assert!(stream.next().await.assert_value().is_none());
                return events;
            }
        }
    }
}

#[tokio::test]
async fn logical_watch_waits_for_reciprocal_handoff_and_resumes_exclusively() {
    let root = TestDirectory::new("watch-handoff");
    let backend = contract_backend(root.path());
    let first = record(&backend, "first", None, Some(suspended())).await;
    let last = record(&backend, "last", Some(&first), Some(finished())).await;
    let mut logical = backend
        .run_watch(None, watch(&first, None, None))
        .await
        .assert_value();
    let mut physical = backend
        .run_watch(None, watch(&first, Some(&first), None))
        .await
        .assert_value();
    let original = done(&mut physical).await;
    for item in &original {
        let CliSubscriptionItem::Event(event) = next(&mut logical).await else {
            panic!("closed before suspension");
        };
        assert_eq!(event.part_id, first);
        assert_eq!(event.run_id, first);
        assert_eq!(event.cursor, part_cursor(&first, &item.cursor));
        assert_eq!(event.status, item.status);
    }
    assert!(matches!(
        original.last().unwrap().status,
        CliRunStatus::Target(RunStatus::Suspended { .. })
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(40), logical.next())
            .await
            .is_err()
    );
    link(&backend, &first, &last);
    let remaining = done(&mut logical).await;
    let cursor = part_cursor(&first, &original.last().unwrap().cursor);
    let mut resumed = backend
        .run_watch(None, watch(&last, None, Some(&cursor)))
        .await
        .assert_value();
    let replay = done(&mut resumed).await;
    assert_eq!(replay.len(), remaining.len());
    let mut exact = backend
        .run_watch(None, watch(&first, Some(&last), None))
        .await
        .assert_value();
    let exact = done(&mut exact).await;
    assert_eq!(exact.len(), remaining.len());
    for ((live, replayed), exact) in remaining.iter().zip(replay).zip(&exact) {
        assert_eq!((&live.run_id, &live.part_id), (&first, &last));
        assert_eq!(live.cursor, part_cursor(&last, &exact.cursor));
        assert_eq!(
            (&replayed.cursor, &replayed.status),
            (&live.cursor, &live.status)
        );
    }
    for (event, expected) in [(remaining.last().unwrap(), 14), (exact.last().unwrap(), 7)] {
        let CliRunStatus::Target(RunStatus::Finished { metadata, .. }) = &event.status else {
            panic!("expected finished part");
        };
        assert_eq!(
            metadata.token_usage.as_ref().unwrap().input_tokens.get(),
            expected
        );
    }
}

#[tokio::test]
async fn logs_resume_across_parts_and_execution_filters_stay_on_the_owning_part() {
    let root = TestDirectory::new("logs-handoff");
    let backend = contract_backend(root.path());
    let first = record(&backend, "before", None, Some(suspended())).await;
    let mut alone = backend
        .run_logs(None, logs(&first, None, None))
        .await
        .assert_value();
    let before = done(&mut alone).await;
    assert_eq!(before.len(), 2);
    let last = record(&backend, "after", Some(&first), Some(finished())).await;
    link(&backend, &first, &last);
    let mut combined = backend
        .run_logs(None, logs(&last, None, None))
        .await
        .assert_value();
    let mut all = done(&mut combined).await;
    assert_eq!(all.len(), 4);
    for (event, part) in all.iter().zip([&first, &first, &last, &last]) {
        assert_eq!((&event.run_id, &event.part_id), (&first, part));
        assert_eq!(split_cursor(&event.cursor).unwrap().0, *part);
        assert_eq!(
            event.record.message.as_str(),
            if part == &first { "before" } else { "after" }
        );
    }
    // Reconnect from the first part's final emitted event, then disconnect early again.
    let mut resumed = backend
        .run_logs(None, logs(&last, None, Some(&all[1].cursor)))
        .await
        .assert_value();
    let CliSubscriptionItem::Event(event) = next(&mut resumed).await else {
        panic!("missing successor output");
    };
    assert_eq!(event.cursor, all[2].cursor);
    drop(resumed);
    let current = all.pop().unwrap();
    let old = &before[1];
    for (filter, part, cursor, expected) in [
        (
            old,
            Some(&first),
            None,
            split_cursor(&old.cursor).unwrap().1,
        ),
        (old, None, Some(&before[0].cursor), old.cursor.clone()),
        (&current, None, None, current.cursor.clone()),
    ] {
        let mut filtered = backend
            .run_logs(
                None,
                RunLogsParams {
                    execution: filter.execution.clone(),
                    ..logs(&first, part, cursor)
                },
            )
            .await
            .assert_value();
        let events = done(&mut filtered).await;
        assert_eq!(events.len(), 1);
        assert_eq!(
            (&events[0].cursor, &events[0].part_id, &events[0].execution),
            (&expected, &filter.part_id, &filter.execution)
        );
    }
    assert!(
        backend
            .run_logs(
                None,
                RunLogsParams {
                    execution: old.execution.clone(),
                    ..logs(&first, None, None)
                }
            )
            .await
            .is_err()
    );
    assert!(
        backend
            .run_logs(None, logs(&first, Some(&last), Some(&old.cursor)))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn recorded_controller_loss_is_visible_until_a_handoff_commits() {
    let root = TestDirectory::new("lost-handoff");
    let backend = contract_backend(root.path());
    let first = record(&backend, "lost", None, None).await;
    let mut interrupted = backend
        .run_logs(None, logs(&first, None, None))
        .await
        .assert_value();
    let CliSubscriptionItem::Event(_) = next(&mut interrupted).await else {
        panic!("missing recorded log");
    };
    let CliSubscriptionItem::Event(tail) = next(&mut interrupted).await else {
        panic!("missing execution log");
    };
    assert!(
        interrupted
            .next()
            .await
            .unwrap_err()
            .to_string()
            .contains("zeroshot resume")
    );
    let last = record(&backend, "recovered", Some(&first), Some(finished())).await;
    link(&backend, &first, &last);
    let mut resumed = backend
        .run_logs(None, logs(&first, None, Some(&tail.cursor)))
        .await
        .assert_value();
    let events = done(&mut resumed).await;
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.part_id == last && event.run_id == first)
    );
    let mut historical = backend
        .run_logs(None, logs(&last, Some(&first), Some(&tail.cursor)))
        .await
        .assert_value();
    assert!(
        historical
            .next()
            .await
            .unwrap_err()
            .to_string()
            .contains(first.as_str())
    );
}

#[tokio::test]
async fn attach_from_the_root_preserves_the_active_successors_execution_and_output() {
    let root = TestDirectory::new("local-attach-handoff");
    let backend = contract_backend(root.path());
    let first = record(&backend, "paused", None, Some(suspended())).await;
    let last = record(&backend, "active", Some(&first), None).await;
    link(&backend, &first, &last);
    let status = backend
        .run_status(None, RunStatusParams::new(first.clone()))
        .await
        .assert_value();
    let CliRunStatus::Target(RunStatus::Running {
        active_executions, ..
    }) = status.status
    else {
        panic!("expected active child");
    };
    let execution = active_executions[0].execution.clone();
    let mut attached = backend
        .run_attach(
            None,
            RunAttachParams {
                run_id: first.clone(),
                part_id: None,
                execution: execution.clone(),
            },
        )
        .await
        .assert_value();
    let CliSubscriptionItem::Event(event) = next(&mut attached).await else {
        panic!("missing recorded execution output");
    };
    assert_eq!(
        (event.run_id, event.part_id, event.execution),
        (first.clone(), last.clone(), execution)
    );
    assert!(
        matches!(event.event, openengine_cluster_protocol::AgentAttachEvent::Output {text} if text.as_str() == "active")
    );
    assert!(
        attached
            .next()
            .await
            .unwrap_err()
            .to_string()
            .contains(last.as_str())
    );
    let stale = RunStatusParams {
        run_id: first.clone(),
        part_id: Some(first),
    };
    let error = backend.run_suspend(None, stale).await.unwrap_err();
    assert!(
        matches!(error, NativeV2CliError::Remote {code, details: Some(details), ..} if code == openengine_cluster_protocol::RUN_CONFLICT && details["currentPartId"] == last.as_str())
    );
}
