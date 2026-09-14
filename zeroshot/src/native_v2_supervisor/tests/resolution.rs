use super::*;
use openengine_cluster_protocol::{
    RunConnectionRequirements, RunConnectionValues, StaticConnectionValues,
};

#[derive(Clone, Copy)]
enum Reply {
    Ready,
    Fail(ConnectionResolutionError),
    Pending,
}

struct Resolver {
    node: &'static str,
    replies: StdMutex<VecDeque<Reply>>,
    calls: AtomicUsize,
    dropped: AtomicUsize,
}

impl Resolver {
    fn new(node: &'static str, replies: impl IntoIterator<Item = Reply>) -> Arc<Self> {
        Arc::new(Self {
            node,
            replies: StdMutex::new(replies.into_iter().collect()),
            calls: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        })
    }

    async fn wait_for_call(&self) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while self.calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .assert_value_with("resolver started");
    }
}

pub(super) struct PendingResolution<'a>(pub(super) &'a AtomicUsize);

impl Drop for PendingResolution<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl RunConnectionResolver for Resolver {
    async fn resolve(
        &self,
        requirements: RunConnectionRequirements,
    ) -> Result<RunConnectionValues, ConnectionResolutionError> {
        if requirements.keys().any(|key| key.as_str() == self.node) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let reply = self
                .replies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or(Reply::Ready);
            match reply {
                Reply::Ready => {}
                Reply::Fail(error) => return Err(error),
                Reply::Pending => {
                    let _pending = PendingResolution(&self.dropped);
                    return std::future::pending().await;
                }
            }
        }
        requirements
            .into_iter()
            .map(|(key, fields)| {
                let values = fields
                    .into_iter()
                    .map(|field| (field, "private-token".to_owned()))
                    .collect();
                StaticConnectionValues::new(values)
                    .map(|values| (key, values))
                    .map_err(|_| ConnectionResolutionError::InvalidResponse)
            })
            .collect()
    }
}

async fn with_resolver(graph: GraphSpec, resolver: Arc<Resolver>) -> Harness {
    harness_with_options(
        graph,
        Value::Null,
        FakeDriver::default(),
        HarnessOptions {
            session_scope: SessionScope::Execution,
            resolver: Some(resolver),
        },
    )
    .await
}

pub(super) fn worker_graph(timeout: Option<u64>) -> GraphSpec {
    let mut worker = step("worker", 10_000);
    if let Some(timeout) = timeout {
        worker["timeoutMs"] = json!(timeout);
    } else {
        worker.as_object_mut().assert_value().remove("timeoutMs");
    }
    graph(
        sequence(vec![worker, succeed("done")], null_type()),
        null_type(),
    )
}

#[tokio::test(start_paused = true)]
async fn transient_resolution_recovers_without_consuming_an_execution_attempt() {
    let resolver = Resolver::new(
        "worker",
        [
            Reply::Fail(ConnectionResolutionError::Unavailable),
            Reply::Fail(ConnectionResolutionError::Unavailable),
            Reply::Ready,
        ],
    );
    let harness = with_resolver(worker_graph(None), resolver.clone()).await;
    assert!(matches!(
        harness.supervisor.drive().await.assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 3);
    assert_eq!(harness.driver.starts("worker"), 1);
    let stored = stored_run(&harness.ledger).await;
    assert_eq!(stored.snapshot.executions.len(), 1);
    assert!(
        stored
            .snapshot
            .executions
            .values()
            .all(|execution| execution.attempt.get() == 1)
    );
    let tail = harness
        .ledger
        .snapshot_and_tail(&stored.snapshot.run_id, None)
        .await
        .assert_value();
    let encoded = serde_json::to_string(&tail.events).assert_value();
    assert!(!encoded.contains("private-token"));
    assert!(encoded.contains("waiting for connection resolution"));
}

async fn unstarted_outcome(harness: &Harness) -> WorkerOutcome {
    assert_eq!(harness.driver.starts("worker"), 0);
    let stored = stored_run(&harness.ledger).await;
    assert_eq!(stored.snapshot.executions.len(), 1);
    stored
        .snapshot
        .executions
        .values()
        .next()
        .assert_value()
        .outcome()
        .assert_value()
        .clone()
}

#[tokio::test(start_paused = true)]
async fn confirmed_refusal_and_invalid_response_are_not_retried() {
    for (error, expected) in [
        (
            ConnectionResolutionError::Refused,
            WorkerOutcome::authentication_refusal(),
        ),
        (
            ConnectionResolutionError::InvalidResponse,
            WorkerOutcome::malformed(),
        ),
    ] {
        let resolver = Resolver::new("worker", [Reply::Fail(error), Reply::Pending]);
        let harness = with_resolver(worker_graph(None), resolver.clone()).await;
        harness.supervisor.drive().await.assert_value();
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(unstarted_outcome(&harness).await, expected);
    }
}

#[tokio::test(start_paused = true)]
async fn node_deadline_includes_initial_resolution_and_stops_its_future() {
    let resolver = Resolver::new("worker", [Reply::Pending]);
    let harness = with_resolver(worker_graph(Some(20)), resolver.clone()).await;
    harness.supervisor.drive().await.assert_value();
    assert_eq!(resolver.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        unstarted_outcome(&harness).await,
        WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
    );
}

#[tokio::test]
async fn force_stop_and_runtime_loss_cancel_initial_resolution_without_late_start() {
    for lost in [false, true] {
        let resolver = Resolver::new("worker", [Reply::Pending]);
        let harness = with_resolver(worker_graph(None), resolver.clone()).await;
        let supervisor = harness.supervisor.clone();
        let drive = tokio::spawn(async move { supervisor.drive().await });
        resolver.wait_for_call().await;
        if lost {
            harness.supervisor.runtime_lost().await;
        } else {
            harness.supervisor.force_stop().await.assert_value();
        }
        let terminal = tokio::time::timeout(Duration::from_secs(1), drive)
            .await
            .assert_value()
            .assert_value()
            .assert_value();
        assert_eq!(
            terminal,
            TerminalResult::Failed {
                reason: EnumLabel::new(if lost {
                    "runtime_lost"
                } else {
                    "force_stopped"
                })
                .assert_value(),
            }
        );
        assert_eq!(resolver.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(harness.driver.starts("worker"), 0);
        assert_eq!(harness.sessions.opened.load(Ordering::SeqCst), 0);
        assert!(
            stored_run(&harness.ledger)
                .await
                .snapshot
                .active_executions()
                .next()
                .is_none()
        );
    }
}

#[tokio::test]
async fn a_waiting_resolver_does_not_block_a_parallel_winner_or_its_void() {
    let resolver = Resolver::new("slow", [Reply::Pending]);
    let harness = with_resolver(
        parallel(
            json!({"kind": "any"}),
            vec![verifier("slow", 10_000), verifier("fast", 10_000)],
        ),
        resolver.clone(),
    )
    .await;
    let terminal = tokio::time::timeout(Duration::from_secs(1), harness.supervisor.drive())
        .await
        .assert_value()
        .assert_value();
    assert!(matches!(terminal, TerminalResult::Succeeded { .. }));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(resolver.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(harness.driver.starts("fast"), 1);
    assert_eq!(harness.driver.starts("slow"), 0);
    assert_eq!(harness.sessions.opened.load(Ordering::SeqCst), 1);
    let stored = stored_run(&harness.ledger).await;
    let slow = stored
        .snapshot
        .executions
        .values()
        .find(|execution| execution.reference.node.as_str() == "slow")
        .assert_value();
    assert!(matches!(slow.state, NodeState::Voided { .. }));
}

#[tokio::test(start_paused = true)]
async fn credential_retry_does_not_reset_the_workers_remaining_deadline() {
    let resolver = Resolver::new(
        "worker",
        [
            Reply::Fail(ConnectionResolutionError::Unavailable),
            Reply::Ready,
        ],
    );
    let harness = with_resolver(worker_graph(Some(1_500)), resolver).await;
    harness
        .driver
        .state()
        .scripts
        .insert("worker".to_owned(), VecDeque::from([Behavior::Hang]));
    let started = tokio::time::Instant::now();
    harness.supervisor.drive().await.assert_value();
    assert_eq!(started.elapsed(), Duration::from_millis(1_500));
    assert_eq!(harness.driver.starts("worker"), 1);
    assert_eq!(harness.driver.cancellations("worker"), 1);
}
