use super::*;
use openengine_cluster_protocol::{
    NodeName, NodeRuntimeBinding, PositiveInteger, Sha256Digest, TerminalResult,
    UnixTimestampMillis, WorkerOutcome,
};
use crate::full_v1_reducer::StructuralOccurrence;
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeCompletion, NodeInstanceId};
use crate::native_v2_observability::history::{HistoryPage, ObservationState, RunDefinition};
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use crate::v2_run_ledger::{
    CreateRun, RunEvent, RunLedger, SafeLogLine, SafeLogStream, MAX_REPLAY_EVENTS,
};

const DEFINITION: &str = "/native-v2/history/definition";
const PAGE: &str = "/native-v2/history/page";

#[derive(Default)]
struct HistoryFactory {
    ledger: Arc<FakeRunLedger>,
    creates: AtomicUsize,
}

#[async_trait]
impl TargetControllerFactory for HistoryFactory {
    async fn create(&self) -> Result<Arc<NativeV2CloudController>, TargetAuthorityError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        NativeV2CloudController::new(self.ledger.clone(), Arc::new(NoAllocation))
            .await
            .map(Arc::new)
            .map_err(|error| TargetAuthorityError::unavailable(error.to_string()))
    }

    async fn submit(
        &self,
        _controller: &NativeV2CloudController,
        _request: TargetRunRequest,
    ) -> Result<TargetRunReceipt, TargetAuthorityError> {
        Err(TargetAuthorityError::invalid(
            "history fixture never submits a run",
        ))
    }
}

struct HistoryServer {
    address: std::net::SocketAddr,
    token: String,
    target: Arc<NativeV2TargetAuthority>,
    factory: Arc<HistoryFactory>,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl Drop for HistoryServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl HistoryServer {
    async fn new() -> Self {
        let factory = Arc::new(HistoryFactory::default());
        let target = Arc::new(NativeV2TargetAuthority::new(factory.clone()));
        let directory =
            std::env::temp_dir().join(format!("zeroshot-history-key-{}", uuid::Uuid::now_v7()));
        crate::execution::platform::create_private_directory(&directory).assert_value();
        let path = directory.join("key");
        {
            use std::io::Write;
            use crate::execution::platform::{private_file, FileAccess};
            let mut file = private_file(&path, FileAccess::CreateNew).assert_value();
            file.write_all("07".repeat(32).as_bytes()).assert_value();
        }
        let key = TargetBootstrapKey::load_and_unlink(&path).assert_value();
        std::fs::remove_dir(&directory).assert_value();
        let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
        let address = listener.local_addr().assert_value();
        let server = Arc::new(
            NativeV2TargetServer::new_private(
                target.clone(),
                identity(),
                format!("ws://{address}{OECP_PATH}"),
                key,
            )
            .assert_value(),
        );
        let task = tokio::spawn(server.serve(listener));
        let token = bootstrap_private_target(address).await;
        Self {
            address,
            token,
            target,
            factory,
            task,
        }
    }

    async fn post(&self, path: &str, value: Value) -> http_fixture::TestHttpResponse {
        let bytes = serde_json::to_vec(&value).assert_value();
        http(
            self.address,
            TestHttpRequest::body("POST", path, Some(&self.token), &bytes),
        )
        .await
    }

    async fn create_run(&self) {
        self.target.controller().await.assert_value();
        let mut submission = request().submission;
        submission.graph = BuiltinGraphTemplate::SingleWorker
            .materialize(TemplateDelivery::None)
            .assert_value();
        submission.initial_input = json!({"task":"Inspect native history"});
        submission.runtime = RuntimePlan::Codex {
            provider: CodexProvider::OpenAi,
            size: RunSize::Small,
            nodes: BTreeMap::from([(
                NodeName::new("worker").assert_value(),
                serde_json::from_value::<NodeRuntimeBinding>(json!({
                    "kind":"agent", "model":"test-model", "sessionScope":"execution",
                }))
                .assert_value(),
            )]),
        };
        let admitted = NativeV2Admission.admit(submission).await.assert_value();
        self.factory
            .ledger
            .create_or_get(CreateRun {
                run_id: run_id(),
                submission_key: IdempotencyKey::new("history").assert_value(),
                submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
                admitted,
            })
            .await
            .assert_value();
    }
}

fn assert_code(response: http_fixture::TestHttpResponse, status: u16, code: &str) {
    assert_eq!(
        response.status,
        status,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body).assert_value()["code"],
        code
    );
}

#[tokio::test]
async fn private_history_validates_without_initializing_a_controller() {
    let fixture = HistoryServer::new().await;
    let valid = json!({"runId":run_id()});
    for path in [DEFINITION, PAGE] {
        let bytes = serde_json::to_vec(&valid).assert_value();
        assert_code(
            http(
                fixture.address,
                TestHttpRequest::body("POST", path, None, &bytes),
            )
            .await,
            401,
            "request.unauthorized",
        );
        assert_code(
            fixture.post(path, valid.clone()).await,
            503,
            "history_unavailable",
        );
        assert_code(
            fixture.post(path, json!({"runId":"bad"})).await,
            400,
            "request.invalid",
        );
        assert_code(
            fixture
                .post(path, json!({"runId":run_id(),"unexpected":true}))
                .await,
            400,
            "request.invalid",
        );
        assert_code(
            fixture
                .post(path, json!({"runId":run_id(),"extra":"x".repeat(4096)}))
                .await,
            400,
            "request.invalid",
        );
    }
    let query = format!("{PAGE}?after=v2:0");
    let malformed = http(
        fixture.address,
        TestHttpRequest::body("POST", &query, Some(&fixture.token), b"{}"),
    )
    .await;
    assert_code(malformed, 400, "request.invalid");
    assert_eq!(fixture.factory.creates.load(Ordering::SeqCst), 0);
    assert!(
        fixture
            .factory
            .ledger
            .list()
            .await
            .assert_value()
            .is_empty()
    );
}

#[tokio::test]
async fn direct_and_hosted_targets_do_not_expose_private_history() {
    let factory = Arc::new(FakeFactory::default());
    let (direct, _, direct_task) = direct_test_server(factory.clone()).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let hosted = listener.local_addr().assert_value();
    let server = Arc::new(
        NativeV2TargetServer::new_hosted(
            Arc::new(NativeV2TargetAuthority::new(factory.clone())),
            Arc::new(FakeSessions),
            format!("ws://{hosted}{OECP_PATH}"),
        )
        .assert_value(),
    );
    let hosted_task = tokio::spawn(server.serve(listener));
    let body = serde_json::to_vec(&json!({"runId":run_id()})).assert_value();
    for address in [direct, hosted] {
        for path in [DEFINITION, PAGE] {
            assert_code(
                http(
                    address,
                    TestHttpRequest::body("POST", path, Some("control-token"), &body),
                )
                .await,
                404,
                "request.not_found",
            );
        }
    }
    assert_eq!(factory.controllers.load(Ordering::SeqCst), 0);
    direct_task.abort();
    hosted_task.abort();
}

fn reference() -> ExecutionRef {
    ExecutionRef {
        run_id: run_id(),
        node: NodeName::new("worker").assert_value(),
        execution: ExecutionId::new(9_007_199_254_740_993).assert_value(),
        node_instance: NodeInstanceId::new(9_007_199_254_740_993).assert_value(),
    }
}

async fn populate_history(fixture: &HistoryServer) {
    let mut events = vec![
        RunEvent::RunStarted,
        RunEvent::NodeStarted {
            reference: reference(),
            occurrence: StructuralOccurrence {
                node: NodeName::new("worker").assert_value(),
                map_indices: vec![],
            },
            attempt: PositiveInteger::new(1).assert_value(),
            input: json!({"task":"Inspect native history"}),
        },
    ];
    events.extend((0..MAX_REPLAY_EVENTS + 2).map(|index| RunEvent::SafeLog {
        execution: Some(reference().execution),
        timestamp: UnixTimestampMillis::new(index as u64 + 1).assert_value(),
        stream: SafeLogStream::Output,
        line: SafeLogLine::new(format!("safe output {index}")).assert_value(),
    }));
    let completion = NodeCompletion {
        reference: reference(),
        outcome: WorkerOutcome::Verified {
            output: Value::Null,
            artifacts: vec![],
        },
    };
    events.push(RunEvent::NodeCompleted { completion });
    events.push(RunEvent::Terminal {
        result: TerminalResult::Succeeded {
            output: Value::Null,
        },
    });
    fixture
        .factory
        .ledger
        .append(&run_id(), events)
        .await
        .assert_value();
}

#[tokio::test]
async fn private_history_exports_native_definition_and_bounded_resumable_pages() {
    let fixture = HistoryServer::new().await;
    fixture.create_run().await;
    populate_history(&fixture).await;
    let before = fixture
        .factory
        .ledger
        .get(&run_id())
        .await
        .assert_value()
        .assert_value();
    let service = fixture.target.history().await.assert_value();
    let response = fixture.post(DEFINITION, json!({"runId":run_id()})).await;
    assert_eq!(response.status, 200);
    let definition: RunDefinition = serde_json::from_slice(&response.body).assert_value();
    assert_eq!(
        json!(definition),
        json!(service.definition(&run_id()).await.assert_value())
    );
    assert_eq!(definition.version, 1);
    assert_eq!(definition.projection_version, 1);
    assert_eq!(definition.source, before.admitted.source);
    assert_eq!(definition.runtime, before.admitted.runtime);
    assert_eq!(definition.initial_input, before.admitted.initial_input);
    assert_eq!(
        definition.snapshot["executions"]["9007199254740993"]["reference"]["execution"],
        "9007199254740993"
    );
    let response = fixture.post(PAGE, json!({"runId":run_id()})).await;
    assert_eq!(response.status, 200);
    let first: HistoryPage = serde_json::from_slice(&response.body).assert_value();
    assert_eq!(first.events.len(), MAX_REPLAY_EVENTS);
    assert!(!first.complete && first.finished);
    assert_eq!(first.observation.state, ObservationState::Complete);
    assert_eq!(
        first.events[1]["event"]["reference"]["execution"],
        "9007199254740993"
    );
    assert!(first.control_error.is_none());
    assert!(!first.control.is_empty());
    assert_eq!(
        json!(first),
        json!(service.page(&run_id(), None).await.assert_value())
    );
    let continuation = json!({"runId":run_id(),"after":first.next_cursor});
    let response = fixture.post(PAGE, continuation.clone()).await;
    let last: HistoryPage = serde_json::from_slice(&response.body).assert_value();
    assert!(last.complete && last.finished);
    assert_eq!(last.events.len(), 6);
    assert_eq!(last.head_cursor, last.next_cursor);
    assert_eq!(
        serde_json::from_slice::<Value>(&fixture.post(PAGE, continuation).await.body)
            .assert_value(),
        json!(last)
    );
    assert_eq!(fixture.factory.creates.load(Ordering::SeqCst), 1);
    assert_eq!(
        before,
        fixture
            .factory
            .ledger
            .get(&run_id())
            .await
            .assert_value()
            .assert_value()
    );
}

#[tokio::test]
async fn private_history_preserves_missing_run_and_cursor_errors() {
    let fixture = HistoryServer::new().await;
    fixture.create_run().await;
    for path in [DEFINITION, PAGE] {
        assert_code(
            fixture.post(path, json!({"runId":other_run_id()})).await,
            404,
            "run_not_found",
        );
    }
    for after in ["bad", "v2:999", "v2:01", "v2:18446744073709551616"] {
        assert_code(
            fixture
                .post(PAGE, json!({"runId":run_id(),"after":after}))
                .await,
            400,
            "invalid_cursor",
        );
    }
    assert_eq!(fixture.factory.creates.load(Ordering::SeqCst), 1);
}
