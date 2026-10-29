use super::*;
use openengine_cluster_protocol::{FieldName, NonEmptyEnumSet, NodeName, WorkerRef, NodeInstructions};
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId};
use crate::native_v2_runner::{NodeRole, NodeResponseContract};

fn binding() -> NodeRuntimeBinding {
    serde_json::from_value(json!({"kind":"decision","api":"systemone","model":"caller/model", "minProbability":0.85,
        "uncertainLabel":"uncertain","connections":{"decisions":["DECISION_ENDPOINT","DECISION_API_KEY"]}})).unwrap()
}

fn response() -> NodeResponseContract {
    let options = NonEmptyEnumSet::new(vec![
        EnumLabel::new("simple").unwrap(),
        EnumLabel::new("complex").unwrap(),
    ])
    .unwrap();
    NodeResponseContract::Verifier {
        output: contract::output_schema(&options).unwrap(),
        signals: BTreeMap::from([(
            FieldName::new("route").unwrap(),
            NonEmptyEnumSet::new(vec![
                EnumLabel::new("simple").unwrap(),
                EnumLabel::new("complex").unwrap(),
                EnumLabel::new("uncertain").unwrap(),
            ])
            .unwrap(),
        )]),
        diagnostic: contract::diagnostic_schema().unwrap(),
    }
}

fn answer(probability: f64) -> Value {
    json!({"model":"returned/build","answers":{"route":{"type":"choice","choice":"simple","probabilities":{"simple":probability,"complex":1.0-probability}}},
        "usage":{"input_tokens":13,"output_tokens":2},"id":"gateway-id","provider_metadata":{"ignored":true}})
}

fn normalized(value: Value) -> Result<WorkerOutcome, NodeRunnerError> {
    let binding = binding();
    let contract = validate_contract(&response(), &binding).unwrap();
    normalize(value, &contract, &binding, "private-test-key")
}

#[test]
fn probabilities_drive_signals_and_original_evidence_survives_abstention() {
    for (p, label) in [
        (0.95, "simple"),
        (0.85, "simple"),
        (0.84, "uncertain"),
        (0.5, "uncertain"),
    ] {
        let WorkerOutcome::Verifier {
            output, signals, ..
        } = normalized(answer(p)).unwrap()
        else {
            panic!("verifier outcome")
        };
        assert_eq!(signals[&FieldName::new("route").unwrap()].as_str(), label);
        assert_eq!(output["choice"], "simple");
        assert_eq!(output["probabilities"]["simple"], p);
        assert_eq!(output["model"], "returned/build");
    }
}

#[test]
fn incomplete_or_inconsistent_provider_answers_never_become_signals() {
    let original = answer(0.9);
    for (path, value) in [
        ("/answers/route/type", json!("noul")),
        ("/answers/route/choice", json!("unknown")),
        ("/answers/route/choice", json!("complex")),
        ("/answers/route/probabilities", json!({"simple":0.9})),
        (
            "/answers/route/probabilities",
            json!({"simple":0.9,"complex":0.4}),
        ),
        (
            "/answers/route/probabilities",
            json!({"simple":1.1,"complex":-0.1}),
        ),
        ("/model", json!("private-test-key")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(path).unwrap() = value;
        assert!(normalized(changed).is_err(), "accepted {path}");
    }
    let mut changed = original;
    changed["answers"]["extra"] = changed["answers"]["route"].clone();
    assert!(normalized(changed).is_err());
}

async fn invocation(endpoint: &str, key: &str) -> DriverInvocation {
    let binding = binding();
    let environment = ResolvedEnvironment::exact(
        &binding,
        BTreeMap::from([
            (
                EnvironmentVariableName::new(ENDPOINT).unwrap(),
                endpoint.to_owned(),
            ),
            (
                EnvironmentVariableName::new(API_KEY).unwrap(),
                key.to_owned(),
            ),
        ]),
    )
    .unwrap();
    let node = NodeInvocation {
        reference: ExecutionRef {
            run_id: openengine_cluster_protocol::RunId::new("decision-test"),
            node: NodeName::new("triage").unwrap(),
            node_instance: NodeInstanceId::new(1).unwrap(),
            execution: ExecutionId::new(1).unwrap(),
        },
        worker: WorkerRef::new("triage@1").unwrap(),
        instructions: Some(
            NodeInstructions::new("Classify the task as simple or complex.").unwrap(),
        ),
        input: json!({"task":"Fix a spelling mistake."}),
        binding,
    };
    let adapter = DecisionAdapter::new().unwrap();
    let session = adapter.open(&node, &environment).await.unwrap();
    DriverInvocation {
        host_context: None,
        node,
        role: NodeRole::Verifier,
        response: response(),
        environment,
        session,
        provider_session_slot: NodeInstanceId::new(1).unwrap(),
    }
}

async fn server(status: u16, body: Value) -> (String, tokio::task::JoinHandle<(String, Value)>) {
    server_bytes(status, serde_json::to_vec(&body).unwrap()).await
}

async fn server_bytes(
    status: u16,
    body: Vec<u8>,
) -> (String, tokio::task::JoinHandle<(String, Value)>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/custom/prefix/systemone",
        listener.local_addr().unwrap()
    );
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut data = Vec::new();
        let header_end = loop {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0 && data.len() < MAX_DECISION_BYTES);
            data.extend_from_slice(&chunk[..n]);
            if let Some(end) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let head = String::from_utf8(data[..header_end].to_vec()).unwrap();
        let length = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|n| n.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while data.len() < header_end + length {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            data.extend_from_slice(&chunk[..n]);
        }
        let request = serde_json::from_slice(&data[header_end..header_end + length]).unwrap();
        let header = format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nLocation: /must-not-follow\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        (head, request)
    });
    (endpoint, task)
}

#[tokio::test]
async fn http_adapter_preserves_auth_path_state_and_usage() {
    let (endpoint, server) = server(200, answer(0.9)).await;
    let invocation = invocation(&endpoint, "private-test-key").await;
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let result = DecisionAdapter::new()
        .unwrap()
        .run(invocation, control)
        .await
        .unwrap();
    assert!(matches!(result, WorkerOutcome::Verifier { .. }));
    let usage = durable.recv_usage().await.unwrap().unwrap();
    assert_eq!(serde_json::to_value(usage).unwrap()["inputTokens"], 13);
    let (head, body) = server.await.unwrap();
    assert!(head.starts_with("POST /custom/prefix/systemone HTTP/1.1"));
    assert!(
        head.to_lowercase()
            .contains("authorization: bearer private-test-key")
    );
    assert_eq!(body["state"], json!({"task":"Fix a spelling mistake."}));
    assert_eq!(body["model"], "caller/model");
    assert_eq!(
        body["questions"]["route"]["criteria"],
        json!({"simple":null,"complex":null})
    );
}

async fn run_failure(
    invocation: DriverInvocation,
) -> (
    WorkerOutcome,
    Option<crate::native_v2_contract::TokenUsageDelta>,
    String,
) {
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let adapter = DecisionAdapter::new().unwrap();
    let (outcome, (usage, diagnostic)) = tokio::join!(adapter.run(invocation, control), async {
        let usage = durable.recv_usage().await.unwrap();
        let diagnostic = durable.recv_output().await.unwrap();
        assert_eq!(diagnostic.stream, LiveOutputStream::Error);
        (usage, diagnostic.text)
    });
    (outcome.unwrap(), usage, diagnostic)
}

#[tokio::test]
async fn provider_failures_preserve_error_kinds_without_exposing_credentials() {
    for status in [302, 401, 403, 503] {
        let (endpoint, server) = server(status, json!({"message":"private-test-key"})).await;
        let invocation = invocation(&endpoint, "private-test-key").await;
        let (outcome, usage, diagnostic) = run_failure(invocation).await;
        let expected = if matches!(status, 401 | 403) {
            WorkerOutcome::authentication_refusal()
        } else {
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
        };
        assert_eq!(outcome, expected);
        assert!(usage.is_none());
        assert!(diagnostic.contains(&status.to_string()));
        assert!(!diagnostic.contains("private-test-key") && !diagnostic.contains(&endpoint));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn invalid_json_is_a_malformed_worker_result() {
    let (endpoint, server) = server_bytes(200, b"private-test-key invalid JSON".to_vec()).await;
    let invocation = invocation(&endpoint, "private-test-key").await;
    let (outcome, usage, diagnostic) = run_failure(invocation).await;
    assert_eq!(outcome, WorkerOutcome::malformed());
    assert!(usage.is_none());
    assert!(!diagnostic.contains("private-test-key") && !diagnostic.contains(&endpoint));
    server.await.unwrap();
}

async fn stalled_request() -> (DriverInvocation, tokio::net::TcpListener) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    (invocation(&endpoint, "private-test-key").await, listener)
}

#[tokio::test(start_paused = true)]
async fn an_expired_http_deadline_is_a_timeout_worker_result() {
    let (invocation, listener) = stalled_request().await;
    let task = tokio::spawn(run_failure(invocation));
    let (_stream, _) = listener.accept().await.unwrap();
    tokio::time::advance(REQUEST_TIMEOUT + Duration::from_secs(1)).await;
    let (outcome, usage, diagnostic) = task.await.unwrap();
    assert_eq!(
        outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
    );
    assert!(usage.is_none());
    assert!(diagnostic.contains("timed out"));
}

#[tokio::test]
async fn cancellation_interrupts_a_stalled_http_response() {
    let (invocation, listener) = stalled_request().await;
    let (cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    let adapter = DecisionAdapter::new().unwrap();
    let task = tokio::spawn(async move { adapter.run(invocation, control).await });
    let (_stream, _) = listener.accept().await.unwrap();
    cancel.send(true).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        Err(NodeRunnerError::Cancelled)
    ));
}

#[tokio::test]
#[ignore = "requires an explicitly supplied decision endpoint, key and model"]
async fn live_systemone_smoke() {
    let endpoint = std::env::var("DECISION_ENDPOINT").expect("DECISION_ENDPOINT");
    let key = std::env::var("DECISION_API_KEY").expect("DECISION_API_KEY");
    let model = std::env::var("DECISION_MODEL").expect("DECISION_MODEL");
    let mut invocation = invocation(&endpoint, &key).await;
    let NodeRuntimeBinding::Decision {
        model: selected, ..
    } = &mut invocation.node.binding
    else {
        panic!("decision binding")
    };
    *selected = ModelId::new(model).unwrap();
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let outcome = DecisionAdapter::new()
        .unwrap()
        .run(invocation, control)
        .await
        .unwrap();
    let WorkerOutcome::Verifier {
        output, signals, ..
    } = outcome
    else {
        panic!("verifier outcome")
    };
    assert_eq!(output["choice"], "simple");
    assert_eq!(
        signals[&FieldName::new("route").unwrap()].as_str(),
        "simple"
    );
    assert!(durable.recv_usage().await.unwrap().is_some());
    println!("Live decision returned a valid choice, distribution, signal and token usage.");
}

fn graph() -> openengine_cluster_protocol::GraphSpec {
    let NodeResponseContract::Verifier {
        output,
        signals,
        diagnostic,
    } = response()
    else {
        panic!("verifier")
    };
    crate::native_v2_candidate::test_support::full_graph(vec![
        json!({
            "kind":"verifier","name":"triage","worker":"triage@1","instructions":"Classify this task as simple or complex.",
            "input":{"kind":"record","fields":{}},"output":output,"signals":signals,"diagnostic":diagnostic,
            "inputBindings":[],"writeBindings":[],"attempts":1,
        }),
        crate::native_v2_candidate::test_support::success_node(),
    ])
}

fn submission() -> openengine_cluster_protocol::RunSubmission {
    serde_json::from_value(json!({"title":"Decision graph","submissionKey":"decision-test","graph":graph(),"initialInput":null,
        "runtime":{"harness":"codex","provider":"openai","size":"small","nodes":{"triage":binding()}}})).unwrap()
}

#[tokio::test]
async fn native_candidate_runs_a_decision_without_starting_a_harness() {
    use crate::native_v2_runner::{NodeRunner, NodeRunRequest};
    let (endpoint, server) = server(200, answer(0.9)).await;
    let admitted = crate::native_v2_admission::NativeV2Admission
        .admit(submission())
        .await
        .unwrap();
    assert!(admitted.runtime.lanes().is_empty());
    let directory = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let memory_path = state.path().join("memory.sqlite");
    let memory = crate::active_memory::runtime::PreparedActiveMemory::prepare(
        crate::active_memory::runtime::MemoryPreparation {
            admitted: &admitted,
            workspace: directory.path(),
            storage: Some(&memory_path),
        },
    )
    .unwrap();
    assert!(memory.is_none());
    assert!(!memory_path.exists());
    let runner = crate::native_v2_candidate::build_local_native_v2_candidate(
        &admitted,
        crate::native_v2_candidate::NativeV2CandidateConfig {
            active_memory: None,
            lanes: Vec::new(),
            workspace: directory.path().to_path_buf(),
            delivery: None,
            comment_delivery: None,
        },
    )
    .unwrap();
    let mut invocation = invocation(&endpoint, "private-test-key").await;
    invocation.node.instructions =
        Some(NodeInstructions::new("Classify this task as simple or complex.").unwrap());
    invocation.node.input = json!({});
    let run_id = invocation.node.reference.run_id.clone();
    let mut handle = runner
        .start(NodeRunRequest {
            invocation: invocation.node,
            environment: invocation.environment,
        })
        .await
        .unwrap();
    let completion = handle.completion().await.unwrap();
    assert!(matches!(completion.outcome, WorkerOutcome::Verifier { .. }));
    runner.close_run(&run_id).await;
    server.await.unwrap();
}

#[tokio::test]
async fn admission_refuses_invalid_decision_contracts_before_execution() {
    let original = submission();
    crate::native_v2_admission::NativeV2Admission
        .admit(original.clone())
        .await
        .unwrap();
    let original = serde_json::to_value(original).unwrap();
    for (path, value) in [
        ("/graph/root/children/0/signals", json!({})),
        (
            "/graph/root/children/0/signals",
            json!({"route":["simple","uncertain"]}),
        ),
        ("/graph/root/children/0/output", json!({"kind":"null"})),
        ("/graph/root/children/0/diagnostic", json!({"kind":"null"})),
        ("/graph/root/children/0/attempts", json!(2)),
        ("/graph/root/children/0/instructions", json!(null)),
        ("/runtime/nodes/triage/connections", json!({})),
        ("/runtime/nodes/triage/uncertainLabel", json!("missing")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(path).unwrap() = value;
        let changed = serde_json::from_value(changed).unwrap();
        assert!(
            crate::native_v2_admission::NativeV2Admission
                .admit(changed)
                .await
                .is_err(),
            "accepted {path}"
        );
    }
    let mut changed = original;
    let verifier = changed["graph"]["root"]["children"][0]
        .as_object_mut()
        .unwrap();
    verifier.insert("kind".to_owned(), json!("step"));
    verifier.remove("signals");
    verifier.remove("diagnostic");
    let changed = serde_json::from_value(changed).unwrap();
    assert!(
        crate::native_v2_admission::NativeV2Admission
            .admit(changed)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rejected_answers_preserve_known_usage_without_fabricating_a_signal() {
    let mut body = answer(0.9);
    body["answers"]["route"]["probabilities"] = json!({"simple":0.9});
    let (endpoint, server) = server(200, body).await;
    let invocation = invocation(&endpoint, "private-test-key").await;
    let (outcome, usage, _) = run_failure(invocation).await;
    assert_eq!(outcome, WorkerOutcome::malformed());
    assert!(usage.is_some());
    server.await.unwrap();
}

#[tokio::test]
async fn reserved_delivery_workers_require_delivery_bindings_at_profile_admission() {
    use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission, NativeV2AdmissionError};
    for worker in [
        "builtin.git-delivery.push@1",
        "builtin.git-delivery.pr@1",
        "builtin.git-delivery.pr@2",
        "builtin.git-delivery.merge@1",
        "builtin.git-delivery.merge@2",
        "builtin.git-delivery.merge@3",
        "builtin.comment-delivery.post@1",
    ] {
        let mut value = serde_json::to_value(submission()).unwrap();
        value["graph"]["root"]["children"][0]["worker"] = json!(worker);
        let submission: openengine_cluster_protocol::RunSubmission =
            serde_json::from_value(value).unwrap();
        assert_eq!(
            NativeV2Admission
                .validate_profile(
                    &submission.graph,
                    &submission.runtime,
                    DeliveryPolicy::Required
                )
                .await,
            Err(NativeV2AdmissionError::DeliveryWorkerRequiresBinding {
                node: NodeName::new("triage").unwrap(),
                worker: WorkerRef::new(worker).unwrap(),
            }),
        );
    }
}
