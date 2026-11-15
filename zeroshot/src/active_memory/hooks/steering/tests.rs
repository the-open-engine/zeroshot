use super::*;
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
        self.reads.lock().assert_value().push(after.cloned());
        tokio::time::sleep(Duration::from_millis(10)).await;
        self.ledger.snapshot_and_tail(run, after).await
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
    hooks.prepare_steers(&mut first, false).await.assert_value();
    let events: Vec<_> = (0..200)
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
    ledger.append(&run, events).await.assert_value();
    let mut received = Vec::new();
    for _ in 0..200 {
        let (query, delivery) = hooks
            .tool_delivery(
                &first,
                &serde_json::json!({"tool_name":"Read"}),
                std::path::Path::new("/workspace"),
            )
            .await;
        for (offset, id) in delivery.steers.iter().enumerate() {
            let i = received.len() + offset;
            let text = format!(
                "[{id}] {i:03}{}",
                "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES - 3)
            );
            assert!(delivery.context.contains(&text));
        }
        received.extend(delivery.steers.iter().cloned());
        hooks.acknowledge(&query, &delivery).await;
        if received.len() == expected.len() {
            break;
        }
    }
    assert_eq!(received, expected);
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
    hooks.prepare_steers(&mut next, false).await.assert_value();
    let prompt = next.host_context.as_ref().assert_value();
    let mut position = 0;
    for (i, id) in expected.iter().enumerate() {
        let entry = format!(
            "[{id}] {i:03}{}",
            "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES - 3)
        );
        let found = prompt[position..].find(&entry).assert_value();
        position += found + entry.len();
    }
    hooks
        .finish_steers(next.node.reference.execution.get(), true)
        .await;
    let vectors = steering::all(ledger.ledger.as_ref(), &run)
        .await
        .assert_value();
    assert_eq!(
        vectors
            .iter()
            .filter(|v| matches!(v.content, RunVectorContent::SteerReceipt { .. }))
            .count(),
        400
    );
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
