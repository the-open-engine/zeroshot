use super::*;
use async_trait::async_trait;
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, NativeNodeRunner, NodeDriver, NodeRunnerError, NodeSession,
    ResolvedEnvironment, SessionFactory,
};
use crate::native_v2_contract::NodeInvocation;

struct RecallHarness(ActiveMemoryHooks);

#[async_trait]
impl SessionFactory for RecallHarness {
    async fn open(
        &self,
        _invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        Ok(Arc::new(
            crate::native_v2_runner::test_support::CountingSession {
                closed: Arc::default(),
            },
        ))
    }
}

#[async_trait]
impl NodeDriver for RecallHarness {
    async fn run(
        &self,
        invocation: DriverInvocation,
        _control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let output = json!(self.0.tool_context(
            &invocation,
            &json!({"tool_name":"Read"}),
            std::path::Path::new("/workspace"),
        ));
        Ok(match invocation.role {
            crate::native_v2_runner::NodeRole::Verifier => WorkerOutcome::Verifier {
                output,
                signals: BTreeMap::new(),
                diagnostic: Value::Null,
                artifacts: Vec::new(),
            },
            _ => WorkerOutcome::Verified {
                output,
                artifacts: Vec::new(),
            },
        })
    }
}

#[tokio::test]
async fn recall_tracks_reused_worker_and_fresh_verifier_homes_independently_of_execution_ids() {
    for executions in [[1, 2, 3], [17, 300, 9001]] {
        let directory = TestDirectory::new("offline-live-recall");
        let settings = LiveSettings {
            lane: RuntimeLane::new("codex", "openai").assert_value(),
            model: "offline-model".to_owned(),
            workspace: directory.path().to_owned(),
            environment: BTreeMap::new(),
        };
        let registry = HarnessRegistry::local();
        let bindings = runtime_bindings(&settings, &registry);
        let admitted = NativeV2Admission
            .admit(live_submission(
                &settings,
                &bindings,
                worker_verifier_graph("Recall the marker.", "Recall the marker."),
                ("Offline recall contract", "offline-recall"),
            ))
            .await
            .assert_value();
        let memory = Arc::new(RecalledMemory {
            marker: "offline-recall-marker".to_owned(),
            queries: Mutex::default(),
            failures: Mutex::default(),
        });
        let harness = Arc::new(RecallHarness(
            ActiveMemoryHooks::new(memory.clone()).assert_value(),
        ));
        let runner = NativeNodeRunner::new(&admitted, harness.clone(), harness).assert_value();
        for ((node, node_instance, session), execution) in [
            ("worker", 1, "node-instance/1"),
            ("worker", 1, "node-instance/1"),
            ("verifier", 2, "execution/2"),
        ]
        .into_iter()
        .zip(executions)
        {
            let request = NodeRequestFixture {
                run_id: "offline-recall",
                node,
                node_instance,
                execution,
                worker: &format!("agent.{node}@1"),
                instructions: "Recall the marker.",
                input: Value::Null,
                binding: bindings[&NodeName::new(node).assert_value()].clone(),
                environment: BTreeMap::new(),
            }
            .into_request();
            let before = memory.queries.lock().assert_value().len();
            let mut handle = runner.start(request).await.assert_value();
            let _ = completion(&mut handle).await;
            assert_execution_recall(&memory, before, node, session);
        }
        runner.close_run(&RunId::new("offline-recall")).await;
        directory.write("marker.txt", &memory.marker);
        assert_recall(&settings, &memory, "marker.txt");
    }
}

#[test]
fn recall_rejects_wrong_home_scope_node_and_missing_execution_callback() {
    for (query, node, session) in [
        (
            Some(("worker", "node-instance/2")),
            "worker",
            "node-instance/1",
        ),
        (Some(("verifier", "execution/1")), "verifier", "execution/2"),
        (
            Some(("verifier", "node-instance/2")),
            "verifier",
            "execution/2",
        ),
        (Some(("worker", "execution/2")), "verifier", "execution/2"),
        (None, "worker", "node-instance/1"),
        (None, "verifier", "execution/2"),
    ] {
        let memory = RecalledMemory {
            marker: "marker".to_owned(),
            queries: Mutex::new(vec![("worker".to_owned(), "node-instance/1".to_owned())]),
            failures: Mutex::default(),
        };
        let before = memory.queries.lock().assert_value().len();
        if let Some((id, key)) = query {
            memory
                .queries
                .lock()
                .assert_value()
                .push((id.to_owned(), key.to_owned()));
        }
        assert!(
            std::panic::catch_unwind(|| assert_execution_recall(&memory, before, node, session))
                .is_err(),
            "incorrect recall was accepted: {query:?}"
        );
    }
}
