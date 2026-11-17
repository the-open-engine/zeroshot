use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::v2_run_ledger::steering;
use openengine_cluster_testkit::assertions::AssertValue;
use crate::active_memory::hooks::{
    ActiveMemoryHooks,
    tests::{invocation, steer_record},
};

// Each page is legal and timely, but the complete record takes longer than a callback.
struct SlowPages {
    ledger: Arc<dyn RunLedger>,
    reads: Mutex<Vec<Option<Cursor>>>,
    fail_next_read: AtomicBool,
    timeout_next_receipt: AtomicBool,
}
#[async_trait::async_trait]
impl RunLedger for SlowPages {
    async fn create_or_get(
        &self,
        request: crate::v2_run_ledger::CreateRun,
    ) -> Result<crate::v2_run_ledger::CreateRunOutcome, RunLedgerError> {
        self.ledger.create_or_get(request).await
    }
    async fn get(
        &self,
        run: &RunId,
    ) -> Result<Option<crate::v2_run_ledger::StoredRun>, RunLedgerError> {
        self.ledger.get(run).await
    }
    async fn get_by_submission_key(
        &self,
        key: &openengine_cluster_protocol::IdempotencyKey,
    ) -> Result<Option<crate::v2_run_ledger::StoredRun>, RunLedgerError> {
        self.ledger.get_by_submission_key(key).await
    }
    async fn list(&self) -> Result<Vec<crate::v2_run_ledger::RunSummary>, RunLedgerError> {
        self.ledger.list().await
    }
    async fn append(
        &self,
        run: &RunId,
        events: Vec<RunEvent>,
    ) -> Result<crate::v2_run_ledger::AppendResult, RunLedgerError> {
        if matches!(events.first(), Some(RunEvent::Vector { vector })
            if matches!(vector.content, RunVectorContent::SteerReceipt { .. }))
            && self.timeout_next_receipt.swap(false, Ordering::SeqCst)
        {
            // The production receipt deadline must cancel this first publication. Its
            // offer remains due, independently of scheduling or SQLite write speed.
            std::future::pending::<()>().await;
        }
        self.ledger.append(run, events).await
    }
    async fn request_force_stop(
        &self,
        run: &RunId,
    ) -> Result<crate::v2_run_ledger::AppendResult, RunLedgerError> {
        self.ledger.request_force_stop(run).await
    }
    async fn request_suspend(
        &self,
        run: &RunId,
    ) -> Result<crate::v2_run_ledger::AppendResult, RunLedgerError> {
        self.ledger.request_suspend(run).await
    }
    async fn vector_cursor(&self, run: &RunId, id: &VectorId) -> Result<Cursor, RunLedgerError> {
        self.ledger.vector_cursor(run, id).await
    }
    async fn snapshot_and_tail(
        &self,
        run: &RunId,
        after: Option<&Cursor>,
    ) -> Result<SnapshotAndTail, RunLedgerError> {
        if self.fail_next_read.swap(false, Ordering::SeqCst) {
            return Err(RunLedgerError::Storage);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        let result = self.ledger.snapshot_and_tail(run, after).await;
        if result.is_ok() {
            self.reads.lock().assert_value().push(after.cloned());
        }
        result
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_many_maximum_entries_progress_across_callback_deadlines_and_reach_new_prompts() {
    use crate::v2_run_ledger::{CreateRun, sqlite::SqliteRunLedger};
    let (source, run) = steer_record().await;
    let stored = source.get(&run).await.assert_value().assert_value();
    let directory = tempfile::tempdir().assert_value();
    let ledger = Arc::new(SlowPages {
        ledger: Arc::new(
            SqliteRunLedger::open(directory.path().join("ledger.sqlite")).assert_value(),
        ),
        reads: Mutex::default(),
        fail_next_read: AtomicBool::new(false),
        timeout_next_receipt: AtomicBool::new(true),
    });
    ledger
        .create_or_get(CreateRun {
            run_id: run.clone(),
            submission_key: stored.submission_key,
            submission_digest: stored.submission_digest,
            admitted: stored.admitted,
        })
        .await
        .assert_value();
    let hooks = ActiveMemoryHooks::empty().assert_value();
    hooks.bind_ledger(run.clone(), ledger.clone());
    let mut first = invocation();
    tokio::time::timeout(Duration::from_secs(30), async {
        while hooks.prepare_steers(&mut first, false).await.is_none() {}
    })
    .await
    .assert_value();
    let events: Vec<_> = (0..257)
        .map(|i| {
            vector_event(
                &run,
                RunVectorContent::Steer {
                    text: format!(
                        "{i:03}{}",
                        "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES - 3)
                    ),
                },
            )
            .assert_value()
        })
        .collect();
    let expected: Vec<_> = events
        .iter()
        .map(|e| match e {
            RunEvent::Vector { vector } => vector.id.clone(),
            _ => unreachable!(),
        })
        .collect();
    let mut after = ledger
        .append(&run, events)
        .await
        .assert_value()
        .snapshot
        .cursor;
    let mut received = Vec::new();
    let mut offered = BTreeSet::new();
    let mut repeated = false;
    tokio::time::timeout(Duration::from_secs(30), async {
        while received.len() < expected.len() {
            let (query, delivery) = hooks
                .tool_delivery(
                    &first,
                    &serde_json::json!({"tool_name":"Read"}),
                    std::path::Path::new("/workspace"),
                )
                .await;
            let mut position = 0;
            let mut previous = None;
            for id in &delivery.steers {
                let i = expected
                    .iter()
                    .position(|expected| expected == id)
                    .assert_value();
                assert!(previous.is_none_or(|previous| i > previous));
                previous = Some(i);
                let text = format!(
                    "[{id}] {i:03}{}",
                    "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES - 3)
                );
                let found = delivery.context[position..].find(&text).assert_value();
                assert_eq!(found, if position == 0 { PREAMBLE.len() } else { 1 });
                position += found + text.len();
                repeated |= !offered.insert(id.clone());
            }
            if !delivery.steers.is_empty() {
                assert_eq!(&delivery.context[position..], "\n</steer>");
            }
            let forced_timeout =
                !delivery.steers.is_empty() && ledger.timeout_next_receipt.load(Ordering::SeqCst);
            hooks.acknowledge(&query, &delivery, true).await;
            // Acknowledgement can time out without publication, or while an already
            // started SQLite write drains. Read durable receipts through that same
            // connection before advancing; a repeated offer is valid until then.
            loop {
                let events = ledger
                    .ledger
                    .steering_events(&run, Some(&after))
                    .await
                    .assert_value();
                if events.is_empty() {
                    break;
                }
                for stored in events {
                    after = stored.cursor;
                    if let RunEvent::Vector { vector } = stored.event
                        && let RunVectorContent::SteerReceipt { steer_id, .. } = vector.content
                    {
                        assert!(offered.contains(&steer_id));
                        if !received.contains(&steer_id) {
                            assert_eq!(expected.get(received.len()), Some(&steer_id));
                            received.push(steer_id);
                        }
                    }
                }
            }
            if forced_timeout {
                assert!(received.is_empty());
            }
        }
    })
    .await
    .assert_value();
    assert_eq!(received, expected);
    assert!(repeated, "a receipt timeout must leave its offer due");
    assert!(!ledger.timeout_next_receipt.load(Ordering::SeqCst));
    let reads = ledger.reads.lock().assert_value().clone();
    assert_eq!(reads.iter().filter(|cursor| cursor.is_none()).count(), 2);
    assert!(
        reads
            .iter()
            .flatten()
            .any(|cursor| cursor.as_str() != "v2:0")
    );
    let (_, delivery) = hooks
        .tool_delivery(
            &first,
            &serde_json::json!({"tool_name":"Read"}),
            std::path::Path::new("/workspace"),
        )
        .await;
    assert!(delivery.context.is_empty());
    // A new conversation needs every standing steer in its prompt, even though a full scan
    // exceeds one callback deadline and another conversation has already received all of them.
    let hooks = ActiveMemoryHooks::empty().assert_value();
    hooks.bind_ledger(run.clone(), ledger.clone());
    let mut next = invocation();
    next.node.reference.execution = crate::native_v2_contract::ExecutionId::new(3).assert_value();
    next.provider_home_slot = crate::native_v2_contract::NodeInstanceId::new(3).assert_value();
    ledger.fail_next_read.store(true, Ordering::SeqCst);
    let mut attempts = 0;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            attempts += 1;
            if hooks.prepare_steers(&mut next, false).await.is_some() {
                break;
            }
        }
    })
    .await
    .assert_value();
    assert!(attempts >= 2, "the failed read must be retried");
    let prompt = next.host_context.as_ref().assert_value();
    assert!(prompt.len() > 64 * 1024 * 1024);
    let mut position = 0;
    for (i, id) in expected.iter().enumerate() {
        let entry = format!(
            "[{id}] {i:03}{}",
            "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES - 3)
        );
        let found = prompt[position..].find(&entry).assert_value();
        assert_eq!(found, if i == 0 { PREAMBLE.len() } else { 1 });
        position += found + entry.len();
    }
    assert_eq!(&prompt[position..], "\n</steer>");
    hooks
        .finish_steers(next.node.reference.execution.get(), true)
        .await;
    let vectors = steering::all(ledger.ledger.as_ref(), &run)
        .await
        .assert_value();
    // A lost acknowledgement may leave more than one receipt vector. Count each
    // (steer, execution) once, just as status does, and require every expected pair.
    let receipts: BTreeSet<_> = vectors
        .into_iter()
        .filter_map(|vector| match vector.content {
            RunVectorContent::SteerReceipt {
                steer_id,
                execution,
                ..
            } => Some((steer_id, execution)),
            _ => None,
        })
        .collect();
    let expected_receipts: BTreeSet<_> = expected
        .iter()
        .flat_map(|id| {
            [&first.node.reference, &next.node.reference]
                .map(|reference| (id.clone(), reference.public_execution().assert_value()))
        })
        .collect();
    assert_eq!(receipts, expected_receipts);
    assert_eq!(receipts.len(), 514);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_restored_conversation_keeps_receipts_until_its_provider_home_is_replaced() {
    let (ledger, run) = steer_record().await;
    let hooks = ActiveMemoryHooks::empty().assert_value();
    hooks.bind_ledger(run.clone(), ledger.clone());
    let mut first = invocation();
    if let openengine_cluster_protocol::NodeRuntimeBinding::Agent { session_scope, .. } =
        &mut first.node.binding
    {
        *session_scope = crate::execution::SessionScope::NodeInstance;
    }
    let direction = steering::record(
        ledger.as_ref(),
        openengine_cluster_protocol::RunSteerParams {
            run_id: run.clone(),
            text: "Standing direction".into(),
        },
    )
    .await
    .assert_value();
    ledger
        .append(
            &run,
            vec![
                vector_event(
                    &run,
                    RunVectorContent::SteerReceipt {
                        steer_id: direction.steer.id,
                        conversation: "predecessor/node-instance/1".into(),
                        execution: first.node.reference.public_execution().assert_value(),
                        node_instance: first.node.reference.node_instance.get(),
                    },
                )
                .assert_value(),
            ],
        )
        .await
        .assert_value();
    let vectors = steering::all(ledger.as_ref(), &run).await.assert_value();
    hooks.restore_steers(
        first.node.reference.node_instance.get(),
        SteeringReceipts {
            conversation: "predecessor/node-instance/1".into(),
            receipts: BTreeSet::from([vectors.last().assert_value().id.clone()]),
        },
    );
    let delivery = hooks.prepare_steers(&mut first, true).await.assert_value();
    assert!(first.host_context.is_none());
    hooks.finish_steers(delivery, true).await;
    let mut revisit = first.clone();
    revisit.node.reference.execution =
        crate::native_v2_contract::ExecutionId::new(3).assert_value();
    let delivery = hooks
        .prepare_steers(&mut revisit, false)
        .await
        .assert_value();
    assert!(revisit.host_context.is_none());
    hooks.finish_steers(delivery, false).await;
    let mut fresh = revisit;
    fresh.node.reference.execution = crate::native_v2_contract::ExecutionId::new(4).assert_value();
    fresh.provider_home_slot = crate::native_v2_contract::NodeInstanceId::new(9).assert_value();
    let delivery = hooks.prepare_steers(&mut fresh, false).await.assert_value();
    assert!(
        fresh
            .host_context
            .as_ref()
            .assert_value()
            .contains("Standing direction")
    );
    hooks.finish_steers(delivery, true).await;
    assert_eq!(
        steering::status(ledger.as_ref(), &run)
            .await
            .assert_value()
            .steers[0]
            .received_executions,
        2
    );
}

async fn successor(ledger: &dyn RunLedger, source: &RunId, next: &RunId) {
    let previous = ledger.get(source).await.assert_value().assert_value();
    ledger
        .create_or_get(crate::v2_run_ledger::CreateRun {
            run_id: next.clone(),
            submission_key: openengine_cluster_protocol::IdempotencyKey::new(next.as_str())
                .assert_value(),
            submission_digest: previous.submission_digest,
            admitted: previous.admitted,
        })
        .await
        .assert_value();
    crate::v2_run_ledger::vectors::inherit(ledger, ledger, source, next)
        .await
        .assert_value();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_snapshot_receipts_exclude_later_receipts_across_repeated_successors_and_legacy_restore()
 {
    use crate::native_v2_contract::ExecutionId;
    use openengine_cluster_protocol::RunSteerParams;
    let (ledger, run) = steer_record().await;
    let hooks = ActiveMemoryHooks::empty().assert_value();
    hooks.bind_ledger(run.clone(), ledger.clone());
    let mut first = invocation();
    if let openengine_cluster_protocol::NodeRuntimeBinding::Agent { session_scope, .. } =
        &mut first.node.binding
    {
        *session_scope = crate::execution::SessionScope::NodeInstance;
    }
    steering::record(
        ledger.as_ref(),
        RunSteerParams {
            run_id: run.clone(),
            text: "before-snapshot".into(),
        },
    )
    .await
    .assert_value();
    let id = hooks.prepare_steers(&mut first, false).await.assert_value();
    hooks.finish_steers(id, true).await;
    let snapshot = hooks.capture_steers();
    steering::record(
        ledger.as_ref(),
        RunSteerParams {
            run_id: run.clone(),
            text: "after-snapshot".into(),
        },
    )
    .await
    .assert_value();
    let mut later = first.clone();
    later.host_context = None;
    later.node.reference.execution = ExecutionId::new(3).assert_value();
    let id = hooks.prepare_steers(&mut later, false).await.assert_value();
    hooks.finish_steers(id, true).await;
    let mut previous = run;
    let mut saved = snapshot;
    for (index, success) in [false, true, true].into_iter().enumerate() {
        let next = RunId::new(format!("steer-snapshot-next-{index}"));
        successor(ledger.as_ref(), &previous, &next).await;
        let hooks = ActiveMemoryHooks::empty().assert_value();
        hooks.bind_ledger(next.clone(), ledger.clone());
        for (instance, receipts) in &saved {
            hooks.restore_steers(instance.get(), receipts.clone());
        }
        let mut turn = first.clone();
        turn.host_context = None;
        turn.node.reference.run_id = next.clone();
        let id = hooks.prepare_steers(&mut turn, true).await.assert_value();
        if index < 2 {
            let context = turn.host_context.as_ref().assert_value();
            assert!(!context.contains("before-snapshot"));
            assert_eq!(context.matches("after-snapshot").count(), 1);
        } else {
            assert!(turn.host_context.is_none());
        }
        hooks.finish_steers(id, success).await;
        saved = hooks.capture_steers();
        previous = next;
    }
    let next = RunId::new("steer-legacy-snapshot");
    successor(ledger.as_ref(), &previous, &next).await;
    let hooks = ActiveMemoryHooks::empty().assert_value();
    hooks.bind_ledger(next.clone(), ledger);
    first.host_context = None;
    first.node.reference.run_id = next;
    hooks.prepare_steers(&mut first, true).await.assert_value();
    let context = first.host_context.as_ref().assert_value();
    assert!(context.contains("before-snapshot"));
    assert!(context.contains("after-snapshot"));
}

// Copilot applies hook context to the turn in progress and writes none of it to its session, so
// a steer shown that way must also reach the conversation's next prompt, and a successor that
// restores the conversation before then must not inherit the receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_unkept_at_a_tool_call_reaches_the_next_prompt_once_and_is_inherited_only_after_it() {
    let (ledger, run) = steer_record().await;
    let hooks = ActiveMemoryHooks::empty().assert_value();
    hooks.bind_ledger(run.clone(), ledger.clone());
    let mut first = invocation();
    if let openengine_cluster_protocol::NodeRuntimeBinding::Agent { session_scope, .. } =
        &mut first.node.binding
    {
        *session_scope = crate::execution::SessionScope::NodeInstance;
    }
    let instance = first.node.reference.node_instance;
    let execution = hooks.prepare_steers(&mut first, false).await.assert_value();
    assert!(first.host_context.is_none());
    steering::record(
        ledger.as_ref(),
        openengine_cluster_protocol::RunSteerParams {
            run_id: run.clone(),
            text: "Standing direction".into(),
        },
    )
    .await
    .assert_value();
    let tool = serde_json::json!({"tool_name":"Read"});
    let workspace = std::path::Path::new("/workspace");
    let (query, delivery) = hooks.tool_delivery(&first, &tool, workspace).await;
    assert!(delivery.context.contains("Standing direction"));
    hooks.acknowledge(&query, &delivery, false).await;
    let received = || async {
        steering::status(ledger.as_ref(), &run)
            .await
            .assert_value()
            .steers[0]
            .received_executions
    };
    assert_eq!(received().await, 1);
    // The turn in progress has it: no later tool call of this execution repeats it.
    let (_, again) = hooks.tool_delivery(&first, &tool, workspace).await;
    assert!(again.steers.is_empty());
    assert!(hooks.capture_steers()[&instance].receipts.is_empty());
    hooks.finish_steers(execution, true).await;
    assert!(hooks.capture_steers()[&instance].receipts.is_empty());

    let mut revisit = first.clone();
    revisit.host_context = None;
    revisit.node.reference.execution =
        crate::native_v2_contract::ExecutionId::new(3).assert_value();
    let execution = hooks
        .prepare_steers(&mut revisit, false)
        .await
        .assert_value();
    assert!(
        revisit
            .host_context
            .as_ref()
            .assert_value()
            .contains("Standing direction")
    );
    // A prompt the provider refused keeps nothing either.
    hooks.finish_steers(execution, false).await;
    assert!(hooks.capture_steers()[&instance].receipts.is_empty());
    revisit.host_context = None;
    let execution = hooks
        .prepare_steers(&mut revisit, false)
        .await
        .assert_value();
    assert!(revisit.host_context.is_some());
    hooks.finish_steers(execution, true).await;
    assert_eq!(hooks.capture_steers()[&instance].receipts.len(), 1);

    let mut later = revisit.clone();
    later.host_context = None;
    later.node.reference.execution = crate::native_v2_contract::ExecutionId::new(4).assert_value();
    let execution = hooks.prepare_steers(&mut later, false).await.assert_value();
    assert!(later.host_context.is_none());
    let (_, at_tool_call) = hooks.tool_delivery(&later, &tool, workspace).await;
    assert!(at_tool_call.steers.is_empty());
    hooks.finish_steers(execution, true).await;
    assert_eq!(received().await, 1);
}
