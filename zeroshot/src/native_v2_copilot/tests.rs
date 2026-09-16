#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use openengine_cluster_protocol::{IdempotencyKey, NodeName, RunSize, RunTitle};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use super::*;
use crate::execution::SessionScope;
use crate::native_v2_candidate::test_support::{
    NodeRequestFixture, TestDirectory, admit, environment_name, full_graph, success_node,
};
use crate::native_v2_contract::{
    AdmittedRun, CopilotProvider, DeclaredConnections, NodeRuntimeBinding, RunSubmission,
    RuntimePlan,
};
use crate::native_v2_runner::{
    NativeNodeRunner, NodeHandle, NodeRunRequest, NodeRunner, DurableNodeEvent,
};

mod authentication;
mod framing;
mod hosted;

const INSTRUCTIONS: &str = "Return answer 42 using the required schema.";

const SCRIPT: &str = include_str!("tests/runtime.py");

fn binding(names: impl Iterator<Item = String>) -> NodeRuntimeBinding {
    NodeRuntimeBinding::Agent {
        model: crate::worker_catalog::ModelId::new("auto").assert_value(),
        effort: None,
        session_scope: SessionScope::NodeInstance,
        connections: DeclaredConnections::single(
            "github",
            crate::native_v2_contract::DeclaredEnvironment::new(
                names
                    .map(|name| environment_name(&name))
                    .collect::<BTreeSet<_>>(),
            )
            .assert_value(),
        )
        .assert_value(),
    }
}

async fn admitted(binding: NodeRuntimeBinding, instructions: &str) -> AdmittedRun {
    let graph = full_graph(vec![
        json!({
            "kind":"step", "name":"work", "worker":"agent.work@1",
            "instructions":instructions, "input":{"kind":"null"},
            "output":{"kind":"record","fields":{"answer":{"type":{"kind":"integer"},"required":true}}},
            "inputBindings":[], "writeBindings":[], "attempts":1,
        }),
        success_node(),
    ]);
    admit(RunSubmission {
        title: RunTitle::new("Copilot contract test").assert_value(),
        graph,
        initial_input: Value::Null,
        runtime: RuntimePlan::Copilot {
            provider: CopilotProvider::Github,
            size: RunSize::Medium,
            nodes: BTreeMap::from([(NodeName::new("work").assert_value(), binding)]),
        },
        source: serde_json::from_value(
            json!({"repository":"the-open-engine/zeroshot", "branch":"main",
            "revision":"0123456789abcdef0123456789abcdef01234567"}),
        )
        .assert_value(),
        submission_key: IdempotencyKey::new("copilot-test").assert_value(),
    })
    .await
}

struct Fixture {
    directory: TestDirectory,
    runtime: NativeNodeRunner,
    binding: NodeRuntimeBinding,
    instructions: &'static str,
    values: BTreeMap<String, String>,
}

impl Fixture {
    async fn new(mode: &str) -> Self {
        Self::with_values(mode, BTreeMap::new()).await
    }

    async fn with_values(mode: &str, extra: BTreeMap<String, String>) -> Self {
        let directory = TestDirectory::new("copilot");
        let executable = directory.write_executable("copilot", SCRIPT);
        let mut values = BTreeMap::from([
            (auth::TOKEN.to_owned(), "gho_fake-secret".to_owned()),
            ("TEST_MODE".to_owned(), mode.to_owned()),
            (
                "CAPTURE_PATH".to_owned(),
                directory.child("capture").display().to_string(),
            ),
        ]);
        values.extend(extra);
        Self::with_executable(directory, executable, values, INSTRUCTIONS).await
    }

    async fn with_executable(
        directory: TestDirectory,
        executable: PathBuf,
        values: BTreeMap<String, String>,
        instructions: &'static str,
    ) -> Self {
        let binding = binding(values.keys().cloned());
        let admitted = admitted(binding.clone(), instructions).await;
        let workspace = directory.child("workspace");
        let runtime_home = directory.child("runtime");
        std::fs::create_dir_all(&workspace).assert_value();
        std::fs::create_dir_all(&runtime_home).assert_value();
        let adapter = Arc::new(CopilotAdapter::new_local(CopilotConfig {
            executable,
            workspace,
            runtime_home,
            search_path: "/usr/bin:/bin".to_owned(),
            process_pool: HostedProcessPool::hosted_default(),
        }));
        let runtime = NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value();
        Self {
            directory,
            runtime,
            binding,
            instructions,
            values,
        }
    }

    fn request(&self, execution: u64) -> NodeRunRequest {
        NodeRequestFixture {
            run_id: "run-copilot",
            node: "work",
            node_instance: 1,
            execution,
            worker: "agent.work@1",
            instructions: self.instructions,
            input: Value::Null,
            binding: self.binding.clone(),
            environment: self
                .values
                .iter()
                .map(|(key, value)| (environment_name(key), value.clone()))
                .collect(),
        }
        .into_request()
    }

    async fn start(&self, execution: u64) -> NodeHandle {
        self.runtime
            .start(self.request(execution))
            .await
            .assert_value()
    }

    fn capture(&self) -> Vec<Value> {
        self.directory
            .read("capture")
            .lines()
            .map(|line| serde_json::from_str(line).assert_value())
            .collect()
    }
}

async fn complete(
    handle: NodeHandle,
) -> (
    Vec<DurableNodeEvent>,
    Result<WorkerOutcome, NodeRunnerError>,
) {
    complete_with_deadline(handle, Duration::from_secs(30)).await
}

async fn complete_with_deadline(
    mut handle: NodeHandle,
    deadline: Duration,
) -> (
    Vec<DurableNodeEvent>,
    Result<WorkerOutcome, NodeRunnerError>,
) {
    let mut output = handle.take_initial_output().assert_value();
    let mut events = Vec::new();
    let finished = tokio::time::timeout(deadline, async {
        tokio::join!(
            async {
                while let Ok(event) = output.recv().await {
                    events.push(event);
                }
            },
            handle.completion()
        )
    })
    .await;
    if finished.is_err() {
        handle.cancel();
        let _ = handle.completion().await;
    }
    assert!(
        finished.is_ok(),
        "Copilot test timed out; safe events: {events:?}"
    );
    let (_, completion) = finished.assert_value();
    (events, completion.map(|value| value.outcome))
}

fn verified(outcome: Result<WorkerOutcome, NodeRunnerError>) {
    assert!(outcome.is_ok(), "Copilot outcome: {outcome:?}");
    assert!(
        matches!(outcome.assert_value(), WorkerOutcome::Verified { output, .. } if output == json!({"answer":42}))
    );
}

#[tokio::test]
async fn schema_permissions_usage_and_resume_preserve_the_session() {
    let fixture = Fixture::new("permission").await;
    let (events, outcome) = complete(fixture.start(1).await).await;
    verified(outcome);
    assert!(events.iter().any(
        |event| matches!(event, DurableNodeEvent::TokenUsage(Some(usage))
        if usage.input_tokens.get() == 7 && usage.output_tokens.get() == 3)
    ));
    verified(complete(fixture.start(2).await).await.1);
    let capture = fixture.capture();
    let created = capture
        .iter()
        .find(|message| message["method"] == "session.create")
        .assert_value();
    let resumed = capture
        .iter()
        .find(|message| message["method"] == "session.resume")
        .assert_value();
    assert_eq!(
        created["params"]["sessionId"],
        resumed["params"]["sessionId"]
    );
    assert!(
        !fixture
            .directory
            .child("workspace")
            .join(".copilot")
            .exists()
    );
}

#[tokio::test]
async fn malformed_output_receives_only_two_corrections_in_the_same_session() {
    for mode in ["correction", "malformed"] {
        let fixture = Fixture::new(mode).await;
        let (_, outcome) = complete(fixture.start(1).await).await;
        if mode == "correction" {
            verified(outcome);
        } else {
            assert_eq!(outcome.assert_value(), WorkerOutcome::malformed());
        }
        let capture = fixture.capture();
        assert_eq!(
            capture
                .iter()
                .filter(|message| message["method"] == "session.create")
                .count(),
            1
        );
        assert_eq!(
            capture
                .iter()
                .filter(|message| message["method"] == "session.send")
                .count(),
            if mode == "correction" { 2 } else { 3 }
        );
    }
}

#[tokio::test]
async fn fails_closed_on_protocol_identity_and_provider_errors() {
    for mode in ["version", "identity", "error"] {
        let fixture = Fixture::new(mode).await;
        let (events, outcome) = complete(fixture.start(1).await).await;
        assert!(outcome.is_err());
        assert!(!format!("{events:?} {outcome:?}").contains("gho_fake-secret"));
    }
}

#[tokio::test]
async fn cancellation_preserves_usage_and_reaps_the_runtime() {
    let fixture = Fixture::new("cancel").await;
    let mut handle = fixture.start(1).await;
    let mut output = handle.take_initial_output().assert_value();
    let usage = output.recv_usage().await.assert_value().assert_value();
    assert_eq!(usage.input_tokens.get(), 7);
    handle.cancel();
    let completion = tokio::time::timeout(Duration::from_secs(10), handle.completion())
        .await
        .assert_value();
    assert_eq!(completion, Err(NodeRunnerError::Cancelled));
}

#[tokio::test]
#[ignore = "requires an explicitly authorized live Copilot user token and pinned CLI"]
async fn live_user_auth_and_schema() {
    let token = std::env::var("ZEROSHOT_COPILOT_TEST_TOKEN")
        .assert_value_with("live token must be supplied");
    let executable = std::env::var_os("ZEROSHOT_COPILOT_TEST_EXECUTABLE").assert_value();
    let fixture = Fixture::with_executable(
        TestDirectory::new("copilot-live"),
        PathBuf::from(executable),
        BTreeMap::from([(auth::TOKEN.to_owned(), token)]),
        INSTRUCTIONS,
    )
    .await;
    verified(complete(fixture.start(1).await).await.1);
    verified(complete(fixture.start(2).await).await.1);
}

#[tokio::test]
#[ignore = "requires an explicitly authorized live Copilot user token and pinned CLI"]
async fn live_tools_do_not_inherit_or_persist_the_user_token() {
    let token = std::env::var("ZEROSHOT_COPILOT_TEST_TOKEN")
        .assert_value_with("live token must be supplied");
    let executable = std::env::var_os("ZEROSHOT_COPILOT_TEST_EXECUTABLE").assert_value();
    let fixture = Fixture::with_executable(TestDirectory::new("copilot-live-tools"), PathBuf::from(executable),
        BTreeMap::from([(auth::TOKEN.to_owned(), token.clone())]),
        "Use the shell tool to verify that none of COPILOT_GITHUB_TOKEN, COPILOT_GITHUB_TOKEN_EXPIRES_AT, \
        COPILOT_SDK_AUTH_TOKEN, GH_TOKEN, or GITHUB_TOKEN are present in its environment. \
        Write ok to credential-isolation.txt only if all are absent. Never print environment values. \
        Then return answer 42 using the required schema.").await;
    let (events, outcome) =
        complete_with_deadline(fixture.start(1).await, Duration::from_secs(120)).await;
    verified(outcome);
    assert!(
        fixture
            .directory
            .child("workspace/credential-isolation.txt")
            .exists(),
        "Copilot did not execute the requested tool; safe events: {events:?}"
    );
    assert_eq!(
        fixture
            .directory
            .read("workspace/credential-isolation.txt")
            .trim(),
        "ok"
    );
    assert_no_persisted_token(&fixture.directory.child("runtime"), token.as_bytes());
}

fn assert_no_persisted_token(directory: &std::path::Path, token: &[u8]) {
    for entry in std::fs::read_dir(directory).assert_value() {
        let entry = entry.assert_value();
        let kind = entry.file_type().assert_value();
        if kind.is_dir() {
            assert_no_persisted_token(&entry.path(), token);
        }
        if kind.is_file() {
            let contents = std::fs::read(entry.path()).assert_value();
            assert!(
                !contents.windows(token.len()).any(|bytes| bytes == token),
                "Copilot persisted a user token"
            );
        }
    }
}
