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

fn script() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/native_v2_copilot/tests/runtime.py"
    ))
    .unwrap()
}

fn binding(names: impl Iterator<Item = String>) -> NodeRuntimeBinding {
    binding_for_model(names, "auto")
}

fn binding_for_model(names: impl Iterator<Item = String>, model: &str) -> NodeRuntimeBinding {
    NodeRuntimeBinding::Agent {
        model: crate::worker_catalog::ModelId::new(model).assert_value(),
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

struct FixturePrompt<'a> {
    instructions: &'static str,
    model: &'a str,
}

fn fixture_values(directory: &TestDirectory, mode: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("TEST_MODE".to_owned(), mode.to_owned()),
        (
            "CAPTURE_PATH".to_owned(),
            directory.child("capture").display().to_string(),
        ),
    ])
}

fn local_user(directory: &TestDirectory) -> CopilotLocalUser {
    CopilotLocalUser {
        home: directory.child("user-home"),
        copilot_home: directory.child("copilot-home"),
    }
}

impl Fixture {
    async fn new(mode: &str) -> Self {
        Self::with_values(mode, BTreeMap::new()).await
    }

    async fn with_values(mode: &str, extra: BTreeMap<String, String>) -> Self {
        let directory = TestDirectory::new("copilot");
        let executable = directory.write_executable("copilot", &script());
        let mut values = fixture_values(&directory, mode);
        values.insert(auth::TOKEN.to_owned(), "gho_fake-secret".to_owned());
        values.extend(extra);
        Self::with_executable(directory, executable, values, INSTRUCTIONS).await
    }

    async fn with_executable(
        directory: TestDirectory,
        executable: PathBuf,
        values: BTreeMap<String, String>,
        instructions: &'static str,
    ) -> Self {
        Self::with_configuration(
            (directory, executable),
            values,
            instructions,
            (None, BTreeMap::new()),
        )
        .await
    }

    async fn native() -> Self {
        Self::with_local_context(
            "native",
            BTreeMap::from([("NATIVE_CONTEXT".to_owned(), "preserved".to_owned())]),
        )
        .await
    }

    async fn ambient_token() -> Self {
        Self::with_local_context(
            "success",
            BTreeMap::from([("GH_TOKEN".to_owned(), "gho_fake-secret".to_owned())]),
        )
        .await
    }

    async fn custom_provider() -> Self {
        Self::with_local_provider("provider", BTreeMap::new(), custom_provider_environment()).await
    }

    async fn provider_key_command() -> Self {
        Self::with_local_provider(
            "provider_command",
            BTreeMap::from([(
                "COPILOT_PROVIDER_API_KEY_COMMAND".to_owned(),
                "provider-key-helper --fresh".to_owned(),
            )]),
            custom_provider_environment(),
        )
        .await
    }

    async fn invalid_command_environment(name: &str, value: String) -> Self {
        let mut base = custom_provider_environment();
        base.remove("COPILOT_PROVIDER_API_KEY");
        base.insert(
            "COPILOT_PROVIDER_API_KEY_COMMAND".to_owned(),
            "provider-key-helper --fresh".to_owned(),
        );
        base.insert(name.to_owned(), value);
        Self::with_local_provider("provider_command", BTreeMap::new(), base).await
    }

    async fn with_local_provider(
        mode: &str,
        extra_values: BTreeMap<String, String>,
        base_environment: BTreeMap<String, String>,
    ) -> Self {
        let directory = TestDirectory::new("copilot-provider");
        let executable = directory.write_executable("copilot", &script());
        let mut values = fixture_values(&directory, mode);
        values.extend(extra_values);
        let local_user = local_user(&directory);
        Self::with_configuration(
            (directory, executable),
            values,
            INSTRUCTIONS,
            (Some(local_user), base_environment),
        )
        .await
    }

    async fn declared_token_over_provider() -> Self {
        let mut base = custom_provider_environment();
        base.insert("COPILOT_OFFLINE".to_owned(), "true".to_owned());
        base.insert(
            "COPILOT_PROVIDERS_CONFIG".to_owned(),
            "/unreadable/provider-registry.json".to_owned(),
        );
        Self::with_local_provider(
            "declared_token",
            BTreeMap::from([(auth::TOKEN.to_owned(), "declared-copilot-token".to_owned())]),
            base,
        )
        .await
    }

    async fn declared_provider_overlay() -> Self {
        let mut base = custom_provider_environment();
        base.insert(
            "COPILOT_PROVIDER_MODEL_ID".to_owned(),
            "ambient-capability-model".to_owned(),
        );
        base.insert(
            "COPILOT_PROVIDER_API_KEY_COMMAND".to_owned(),
            "ambient-key-helper".to_owned(),
        );
        base.insert(
            "COPILOT_PROVIDER_BEARER_TOKEN".to_owned(),
            "ambient-bearer".to_owned(),
        );
        Self::with_local_provider(
            "provider_overlay",
            BTreeMap::from([
                (
                    "COPILOT_PROVIDER_BASE_URL".to_owned(),
                    "https://declared.example/v1".to_owned(),
                ),
                (
                    "COPILOT_PROVIDER_API_KEY".to_owned(),
                    "declared-provider-key".to_owned(),
                ),
                (
                    "COPILOT_PROVIDER_WIRE_MODEL".to_owned(),
                    "declared-wire-model".to_owned(),
                ),
            ]),
            base,
        )
        .await
    }

    async fn declared_native_setting_overlay() -> Self {
        let mut base = custom_provider_environment();
        base.insert("COPILOT_OFFLINE".to_owned(), "false".to_owned());
        Self::with_local_provider(
            "settings",
            BTreeMap::from([("COPILOT_OFFLINE".to_owned(), "true".to_owned())]),
            base,
        )
        .await
    }

    async fn registry(explicit: bool, command: bool) -> Self {
        let directory = TestDirectory::new("copilot-registry");
        let executable = directory.write_executable("copilot", &script());
        let copilot_home = directory.child("copilot-home");
        std::fs::create_dir_all(&copilot_home).assert_value();
        let registry = if command {
            json!({
                "providers":[{
                    "name":"gateway", "type":"openai",
                    "baseUrl":"https://registry.example/v1",
                    "apiKeyCommand":"registry-key-helper --fresh",
                    "wireApi":"responses"
                }],
                "models":[{
                    "id":"fixture", "provider":"gateway",
                    "modelId":"gpt-5.6-sol", "wireModel":"wire-registry-model"
                }]
            })
        } else {
            json!({
                "providers":[{
                    "name":"gateway", "type":"openai",
                    "baseUrl":"https://registry.example/v1",
                    "apiKey":"registry-sensitive-value", "wireApi":"responses"
                }],
                "models":[{
                    "id":"fixture", "provider":"gateway",
                    "modelId":"gpt-5.6-sol", "wireModel":"wire-registry-model"
                }]
            })
        };
        let registry_path = if explicit {
            directory.write("explicit-providers.json", &registry.to_string())
        } else {
            let path = copilot_home.join("providers.json");
            std::fs::write(&path, registry.to_string()).assert_value();
            path
        };
        let mut base = custom_provider_environment();
        if explicit {
            base.insert(
                "COPILOT_PROVIDERS_CONFIG".to_owned(),
                registry_path.display().to_string(),
            );
        }
        let mode = if command {
            "registry_command"
        } else {
            "registry"
        };
        let values = fixture_values(&directory, mode);
        let local_user = local_user(&directory);
        Self::with_model_configuration(
            (directory, executable),
            values,
            FixturePrompt {
                instructions: INSTRUCTIONS,
                model: "gateway/fixture",
            },
            (Some(local_user), base),
        )
        .await
    }

    async fn invalid_registry(contents: &[u8]) -> Self {
        let directory = TestDirectory::new("copilot-invalid-registry");
        let executable = directory.write_executable("copilot", &script());
        let registry_path = directory.child("providers.json");
        std::fs::write(&registry_path, contents).assert_value();
        let values = fixture_values(&directory, "native");
        let local_user = local_user(&directory);
        Self::with_configuration(
            (directory, executable),
            values,
            INSTRUCTIONS,
            (
                Some(local_user),
                BTreeMap::from([(
                    "COPILOT_PROVIDERS_CONFIG".to_owned(),
                    registry_path.display().to_string(),
                )]),
            ),
        )
        .await
    }

    async fn with_local_context(mode: &str, base_environment: BTreeMap<String, String>) -> Self {
        Self::with_local_provider(mode, BTreeMap::new(), base_environment).await
    }

    async fn with_configuration(
        source: (TestDirectory, PathBuf),
        values: BTreeMap<String, String>,
        instructions: &'static str,
        native: (Option<CopilotLocalUser>, BTreeMap<String, String>),
    ) -> Self {
        Self::with_model_configuration(
            source,
            values,
            FixturePrompt {
                instructions,
                model: "auto",
            },
            native,
        )
        .await
    }

    async fn with_model_configuration(
        source: (TestDirectory, PathBuf),
        values: BTreeMap<String, String>,
        prompt: FixturePrompt<'_>,
        native: (Option<CopilotLocalUser>, BTreeMap<String, String>),
    ) -> Self {
        let (directory, executable) = source;
        let (local_user, base_environment) = native;
        let binding = binding_for_model(values.keys().cloned(), prompt.model);
        let admitted = admitted(binding.clone(), prompt.instructions).await;
        let workspace = directory.child("workspace");
        let runtime_home = directory.child("runtime");
        std::fs::create_dir_all(&workspace).assert_value();
        std::fs::create_dir_all(&runtime_home).assert_value();
        let mut local_command_environment = base_environment.clone();
        local_command_environment.insert(
            "COMMAND_DEPENDENCY".to_owned(),
            "private-command-context".to_owned(),
        );
        let adapter = Arc::new(CopilotAdapter::new_local(CopilotConfig {
            executable,
            workspace,
            runtime_home,
            local_user,
            base_environment,
            local_command_environment,
            search_path: "/usr/bin:/bin".to_owned(),
            process_pool: HostedProcessPool::hosted_default(),
        }));
        let runtime = NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value();
        Self {
            directory,
            runtime,
            binding,
            instructions: prompt.instructions,
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

fn custom_provider_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "COPILOT_PROVIDER_BASE_URL".to_owned(),
            "https://gateway.example/v1".to_owned(),
        ),
        ("COPILOT_PROVIDER_TYPE".to_owned(), "openai".to_owned()),
        (
            "COPILOT_PROVIDER_API_KEY".to_owned(),
            "provider-sensitive-value".to_owned(),
        ),
        (
            "COPILOT_PROVIDER_WIRE_API".to_owned(),
            "responses".to_owned(),
        ),
        (
            "COPILOT_PROVIDER_WIRE_MODEL".to_owned(),
            "gateway-model".to_owned(),
        ),
        (
            "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS".to_owned(),
            "4096".to_owned(),
        ),
        (
            "COPILOT_PROVIDER_HEADERS".to_owned(),
            "X-Tenant: alpha\\nX-Route: beta".to_owned(),
        ),
    ])
}

#[tokio::test]
async fn native_local_login_and_context_need_no_declared_token() {
    let fixture = Fixture::native().await;
    verified(complete(fixture.start(1).await).await.1);
}

#[tokio::test]
async fn ambient_local_token_crosses_private_rpc_only() {
    let fixture = Fixture::ambient_token().await;
    verified(complete(fixture.start(1).await).await.1);
}

#[tokio::test]
async fn local_custom_provider_configuration_crosses_private_rpc_only() {
    let fixture = Fixture::custom_provider().await;
    verified(complete(fixture.start(1).await).await.1);
}

#[tokio::test]
async fn local_provider_key_command_crosses_private_rpc() {
    let fixture = Fixture::provider_key_command().await;
    verified(complete(fixture.start(1).await).await.1);
}

#[tokio::test]
async fn unsupported_or_oversized_command_environments_fail_before_launch() {
    for fixture in [
        Fixture::invalid_command_environment("UNREPRESENTABLE,FIELD", "value".to_owned()).await,
        Fixture::invalid_command_environment("OVERSIZED_FIELD", "x".repeat(4 * 1024 * 1024 + 1))
            .await,
    ] {
        rejected_before_launch(&fixture).await;
    }
}

#[tokio::test]
async fn default_provider_registry_takes_precedence_over_legacy_environment() {
    let fixture = Fixture::registry(false, false).await;
    verified(complete(fixture.start(1).await).await.1);
    verified(complete(fixture.start(2).await).await.1);
    let capture = fixture.capture();
    for message in capture.iter().filter(|message| {
        matches!(
            message["method"].as_str(),
            Some("session.create" | "session.resume")
        )
    }) {
        assert_eq!(message["params"]["providers"][0]["name"], "gateway");
        assert_eq!(message["params"]["models"][0]["provider"], "gateway");
    }
}

#[tokio::test]
async fn explicit_registry_command_is_translated_to_the_working_singular_protocol() {
    let fixture = Fixture::registry(true, true).await;
    verify_two_turns(&fixture).await;
}

#[tokio::test]
async fn malformed_and_oversized_provider_registries_fail_closed() {
    for contents in [
        b"{not-json".to_vec(),
        vec![b' '; provider::MAX_REGISTRY_BYTES as usize + 1],
    ] {
        let fixture = Fixture::invalid_registry(&contents).await;
        rejected_before_launch(&fixture).await;
    }
}

#[tokio::test]
async fn declared_copilot_token_suppresses_ambient_provider_configuration() {
    let fixture = Fixture::declared_token_over_provider().await;
    verified(complete(fixture.start(1).await).await.1);
    let created = fixture
        .capture()
        .into_iter()
        .find(|message| message["method"] == "session.create")
        .assert_value();
    assert_eq!(created["params"]["gitHubToken"], "declared-copilot-token");
    assert!(created["params"].get("provider").is_none());
}

#[tokio::test]
async fn declared_provider_fields_keep_model_selection_separate_from_capability_mapping() {
    let fixture = Fixture::declared_provider_overlay().await;
    verified(complete(fixture.start(1).await).await.1);
}

#[tokio::test]
async fn declared_native_settings_overlay_ambient_settings() {
    let fixture = Fixture::declared_native_setting_overlay().await;
    verified(complete(fixture.start(1).await).await.1);
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

async fn rejected_before_launch(fixture: &Fixture) {
    let (events, outcome) = complete(fixture.start(1).await).await;
    assert!(outcome.is_err(), "safe events: {events:?}");
    assert!(!fixture.directory.child("capture").exists());
}

async fn verify_two_turns(fixture: &Fixture) {
    for execution in [1, 2] {
        verified(complete(fixture.start(execution).await).await.1);
    }
}

#[tokio::test]
async fn schema_permissions_usage_and_resume_preserve_the_session() {
    let fixture = Fixture::new("permission").await;
    let (events, outcome) = complete(fixture.start(1).await).await;
    verified(outcome);
    assert!(events.iter().any(
        |event| matches!(event, DurableNodeEvent::TokenUsage(Some(usage))
        if usage.input_tokens.get() == 7 && usage.output_tokens.get() == 3
            && usage.cache_read_input_tokens.is_some_and(|value| value.get() == 2)
            && usage.cache_creation_input_tokens.is_some_and(|value| value.get() == 1))
    ));
    let logs = events
        .iter()
        .filter_map(|event| match event {
            DurableNodeEvent::Output { output, .. } => Some(output.text.as_str()),
            DurableNodeEvent::TokenUsage(_) => None,
        })
        .collect::<Vec<_>>();
    assert!(logs.contains(&"Copilot tool: contract-check"));
    assert!(
        logs.iter()
            .any(|message| message.starts_with("Copilot tool failed: "))
    );
    assert!(
        logs.iter()
            .all(|message| !message.contains("ignored child output"))
    );
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

#[test]
fn hosted_adapter_discards_user_only_credentials_and_provider_controls() {
    let config = CopilotConfig {
        executable: PathBuf::from("/provider/copilot"),
        workspace: PathBuf::from("/candidate"),
        runtime_home: PathBuf::from("/runtime"),
        local_user: Some(CopilotLocalUser {
            home: PathBuf::from("/user"),
            copilot_home: PathBuf::from("/user/.copilot"),
        }),
        base_environment: BTreeMap::from([
            (auth::TOKEN.to_owned(), "private-token".to_owned()),
            ("GH_TOKEN".to_owned(), "private-alias".to_owned()),
            (
                "COPILOT_PROVIDER_BASE_URL".to_owned(),
                "https://private.example".to_owned(),
            ),
            ("COPILOT_OFFLINE".to_owned(), "true".to_owned()),
            ("SAFE_FIELD".to_owned(), "preserved".to_owned()),
        ]),
        local_command_environment: BTreeMap::from([(
            "COMMAND_SECRET".to_owned(),
            "private-command-value".to_owned(),
        )]),
        search_path: "/usr/bin:/bin".to_owned(),
        process_pool: HostedProcessPool::hosted_default(),
    };
    let debug = format!("{config:?}");
    assert!(debug.contains("base_environment_fields"));
    assert!(!debug.contains("private-token"));
    assert!(!debug.contains("private-command-value"));

    let adapter = CopilotAdapter::new(config);
    assert!(adapter.config.local_user.is_none());
    assert_eq!(
        adapter.config.base_environment,
        BTreeMap::from([("SAFE_FIELD".to_owned(), "preserved".to_owned())])
    );
    assert!(adapter.config.local_command_environment.is_empty());
    assert!(adapter.local_token.is_none());
    assert!(adapter.local_provider.is_none());
    assert!(adapter.runners.is_hosted());
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
    verify_two_turns(&fixture).await;
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
