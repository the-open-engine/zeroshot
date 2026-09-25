#![cfg(unix)]

#[path = "tests/cancellation.rs"]
mod cancellation;
#[path = "tests/correction.rs"]
mod correction;
#[path = "tests/environment.rs"]
mod environment;
#[path = "tests/large_output.rs"]
mod large_output;
#[path = "tests/large_prompt.rs"]
mod large_prompt;
#[path = "tests/local_identity.rs"]
mod local_identity;
#[path = "tests/process_start.rs"]
mod process_start;
#[path = "tests/retry.rs"]
mod retry;
#[path = "tests/session_id.rs"]
mod session_id;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use openengine_cluster_protocol::{
    IdempotencyKey, NodeName, RunSize, RunTitle, TokenCount, WorkerOutcome,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{json, Value};

use super::*;
use super::command::{AWS_BEARER_TOKEN_BEDROCK, AWS_REGION};
use crate::execution::SessionScope;
use crate::native_v2_candidate::test_support::{
    NodeRequestFixture, TestDirectory, admit, environment_name, full_graph, success_node,
};
use crate::native_v2_contract::{
    AdmittedRun, DeclaredConnections, DeclaredEnvironment, EnvironmentVariableName,
    NodeRuntimeBinding, RunSubmission, RuntimePlan, TokenUsageDelta,
};
use crate::native_v2_runner::{
    AttachReceiveError, NativeNodeRunner, NodeHandle, NodeRunRequest, NodeRunner,
};
use crate::worker_catalog::{self, ReasoningEffort};

const SCRIPT: &str = r#"#!/bin/sh
set -eu
prompt=$(/usr/bin/cat)
{
  /usr/bin/printf '%s\n' '---'
  for argument in "$@"; do
    /usr/bin/printf 'arg=%s\n' "$argument"
  done
  /usr/bin/printf 'prompt=%s\n' "$prompt"
  /usr/bin/printf 'codex_home=%s\n' "$CODEX_HOME"
  /usr/bin/printf 'gateway_key=%s\n' "${GATEWAY_API_KEY-unset}"
  /usr/bin/printf 'openrouter_key=%s\n' "${OPENROUTER_API_KEY-unset}"
  /usr/bin/printf 'openai_key=%s\n' "${OPENAI_API_KEY-unset}"
  /usr/bin/printf 'codex_key=%s\n' "${CODEX_API_KEY-unset}"
  /usr/bin/printf 'bedrock_key=%s\n' "${AWS_BEARER_TOKEN_BEDROCK-unset}"
  /usr/bin/printf 'aws_region=%s\n' "${AWS_REGION-unset}"
  /usr/bin/printf 'path=%s\n' "${PATH-unset}"
  /usr/bin/printf 'home=%s\n' "${HOME-unset}"
  /usr/bin/printf 'scratch=%s\n' "$TMPDIR"
  /usr/bin/printf 'ambient=%s\n' "${AMBIENT_SENTINEL-unset}"
} >> "$CAPTURE_PATH"

schema_path=
previous=
for argument in "$@"; do
  if [ "$previous" = "--output-schema" ]; then schema_path=$argument; fi
  previous=$argument
done
test -n "$schema_path"
/usr/bin/printf 'schema_path=%s\n' "$schema_path" >> "$CAPTURE_PATH"
/usr/bin/printf 'schema=%s\n' "$(/usr/bin/cat "$schema_path")" >> "$CAPTURE_PATH"

resumed=false
thread_id=${THREAD_ID-thread-123}
for argument in "$@"; do
  if [ "$argument" = "resume" ]; then
    resumed=true
  fi
done

/usr/bin/printf '{"type":"thread.started","thread_id":"%s"}\n' "$thread_id"
/usr/bin/printf '%s\n' '{"type":"turn.started"}'
if [ "${OPENROUTER_API_KEY-unset}" != unset ]; then
  /usr/bin/printf '%s%s\n' \
    '{"type":"item.completed","item":{"type":"agent_message",' \
    '"text":"visible fake-openrouter-key"}}'
fi
if [ "${GATEWAY_API_KEY-unset}" != unset ]; then
  /usr/bin/printf '%s%s\n' \
    '{"type":"item.completed","item":{"type":"agent_message",' \
    '"text":"visible fake-gateway-key"}}'
fi
if [ "${ALWAYS_MALFORMED-false}" = true ]; then
  /usr/bin/printf '%s%s\n' \
    '{"type":"item.completed","item":{"type":"agent_message",' \
    '"text":"{\"response\":{\"answer\":\"wrong\"}}"}}'
elif [ "${CORRECT_OUTPUT-false}" = true ] && [ "$resumed" = false ]; then
  /usr/bin/printf '%s%s\n' \
    '{"type":"item.completed","item":{"type":"agent_message",' \
    '"text":"{\"response\":{\"answer\":\"wrong\"}}"}}'
elif [ "$resumed" = true ]; then
  /usr/bin/printf '%s%s\n' \
    '{"type":"item.completed","item":{"type":"agent_message",' \
    '"text":"{\"response\":{\"answer\":43}}"}}'
else
  /usr/bin/printf '%s%s\n' \
    '{"type":"item.completed","item":{"type":"agent_message",' \
    '"text":"{\"response\":{\"answer\":42}}"}}'
fi

if [ "${SLOW_RUN-false}" = true ]; then
  /usr/bin/printf '%s\n' "$$" > "$PID_PATH"
  /usr/bin/sleep 30
fi
/usr/bin/printf '%s%s\n' \
  '{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":1,' \
  '"cache_write_input_tokens":3,"output_tokens":2}}'
"#;

const RESUMED_USAGE_SCRIPT: &str = r#"#!/bin/sh
set -eu
resumed=false
for argument in "$@"; do
  if [ "$argument" = "resume" ]; then
    resumed=true
  fi
done
/usr/bin/printf '%s\n' '{"type":"thread.started","thread_id":"thread-usage"}'
if [ "${CORRECT_OUTPUT-false}" = true ] && [ "$resumed" = false ]; then
  /usr/bin/printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":{\"answer\":\"wrong\"}}"}}'
elif [ "$resumed" = true ]; then
  /usr/bin/printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":{\"answer\":43}}"}}'
else
  /usr/bin/printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":{\"answer\":42}}"}}'
fi
if [ "$resumed" = true ]; then
  /usr/bin/printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":3,"cached_input_tokens":2,"cache_write_input_tokens":4,"output_tokens":5}}'
else
  /usr/bin/printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":1,"cache_write_input_tokens":3,"output_tokens":2}}'
fi
"#;

const SUCCESS_OUTPUT_SCRIPT: &str = r#"
/usr/bin/printf '%s%s\n' \
  '{"type":"item.completed","item":{"type":"agent_message",' \
  '"text":"{\"response\":{\"answer\":42}}"}}'
/usr/bin/printf '%s\n' '{"type":"turn.completed"}'
"#;

fn script_with_success(prefix: &str) -> String {
    let mut script = String::with_capacity(prefix.len() + SUCCESS_OUTPUT_SCRIPT.len());
    script.push_str(prefix);
    script.push_str(SUCCESS_OUTPUT_SCRIPT);
    script
}

fn token_usage(
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
) -> TokenUsageDelta {
    TokenUsageDelta {
        input_tokens: TokenCount::new(input_tokens).assert_value(),
        output_tokens: TokenCount::new(output_tokens).assert_value(),
        cache_read_input_tokens: cache_read_input_tokens
            .map(|value| TokenCount::new(value).assert_value()),
        cache_creation_input_tokens: cache_creation_input_tokens
            .map(|value| TokenCount::new(value).assert_value()),
    }
}

fn scripted_adapter(
    directory: &TestDirectory,
    provider: CodexProvider,
) -> Arc<NativeV2CodexAdapter> {
    scripted_adapter_with(directory, provider, "codex-script", SCRIPT)
}

fn scripted_adapter_with(
    directory: &TestDirectory,
    provider: CodexProvider,
    name: &str,
    script: &str,
) -> Arc<NativeV2CodexAdapter> {
    let executable = directory.write_executable(name, script);
    let runtime_home = directory.child("runtime-home");
    fs::create_dir_all(&runtime_home).assert_value();
    adapter_with_configuration(directory, provider, executable, runtime_home)
}

fn adapter_with_configuration(
    directory: &TestDirectory,
    provider: CodexProvider,
    executable: PathBuf,
    runtime_home: PathBuf,
) -> Arc<NativeV2CodexAdapter> {
    let workspace = directory.child("workspace");
    fs::create_dir_all(&workspace).assert_value();
    Arc::new(NativeV2CodexAdapter::new_for_test(NativeV2CodexConfig {
        base_environment: Default::default(),
        provider,
        executable,
        workspace,
        runtime_home,
        local_user: None,
        native_environment: Default::default(),
        search_path: "/usr/bin:/bin".to_owned(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000).assert_value(),
    }))
}

fn environment_names(names: &[&str]) -> DeclaredEnvironment {
    DeclaredEnvironment::new(names.iter().map(|name| environment_name(name))).assert_value()
}

fn binding(scope: SessionScope, environment: &[&str]) -> NodeRuntimeBinding {
    binding_with_model("gpt-5.6-sol", scope, environment)
}

fn binding_with_model(
    model: &str,
    scope: SessionScope,
    environment: &[&str],
) -> NodeRuntimeBinding {
    let connections = if environment.is_empty() {
        DeclaredConnections::empty()
    } else {
        DeclaredConnections::single("provider", environment_names(environment)).assert_value()
    };
    NodeRuntimeBinding::Agent {
        model: worker_catalog::ModelId::new(model).assert_value(),
        effort: Some(ReasoningEffort::Max),
        session_scope: scope,
        connections,
    }
}

async fn admitted(binding: NodeRuntimeBinding, provider: CodexProvider) -> AdmittedRun {
    let graph = full_graph(vec![
        json!({
            "kind":"step","name":"work","worker":"agent.work@1",
            "instructions":"Exercise the Codex adapter.",
            "input":{"kind":"null"},
            "output":{"kind":"record","fields":{
                "answer":{"type":{"kind":"integer"},"required":true}
            }},
            "inputBindings":[],"writeBindings":[],"timeoutMs":1000,"attempts":1
        }),
        success_node(),
    ]);
    admit(RunSubmission {
        environment: None,
        title: RunTitle::new("Codex adapter test").assert_value(),
        graph,
        initial_input: Value::Null,
        runtime: RuntimePlan::Codex {
            provider,
            size: RunSize::Medium,
            nodes: BTreeMap::from([(NodeName::new("work").assert_value(), binding)]),
        },
        source: serde_json::from_value(json!({
            "repository": "open-engine/zeroshot",
            "branch": "main",
            "revision": "0123456789abcdef0123456789abcdef01234567"
        }))
        .assert_value(),
        submission_key: IdempotencyKey::new("codex-adapter-test").assert_value(),
    })
    .await
}

fn request(admitted: &AdmittedRun, execution: u64, values: &[(&str, String)]) -> NodeRunRequest {
    let binding = admitted
        .runtime
        .nodes()
        .get(&NodeName::new("work").assert_value())
        .assert_value()
        .clone();
    let values = values
        .iter()
        .map(|(name, value)| (environment_name(name), value.clone()))
        .collect();
    NodeRequestFixture {
        run_id: "run-codex",
        node: "work",
        node_instance: 1,
        execution,
        worker: "agent.work@1",
        instructions: "Exercise the Codex adapter.",
        input: json!({"task":"change the workspace"}),
        binding,
        environment: values,
    }
    .into_request()
}

fn runner(admitted: &AdmittedRun, adapter: Arc<NativeV2CodexAdapter>) -> NativeNodeRunner {
    NativeNodeRunner::new(admitted, adapter.clone(), adapter).assert_value()
}

async fn openai_runtime(
    directory: &TestDirectory,
    scope: SessionScope,
    environment: &[&str],
) -> (AdmittedRun, NativeNodeRunner) {
    openai_scripted_runtime(
        directory,
        OpenAiScript {
            scope,
            environment,
            name: "codex-script",
            body: SCRIPT,
        },
    )
    .await
}

struct OpenAiScript<'a> {
    scope: SessionScope,
    environment: &'a [&'a str],
    name: &'a str,
    body: &'a str,
}

async fn openai_scripted_runtime(
    directory: &TestDirectory,
    script: OpenAiScript<'_>,
) -> (AdmittedRun, NativeNodeRunner) {
    let adapter = scripted_adapter_with(directory, CodexProvider::OpenAi, script.name, script.body);
    let admitted = admitted(
        binding(script.scope, script.environment),
        CodexProvider::OpenAi,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    (admitted, runtime)
}

async fn start(
    runtime: &NativeNodeRunner,
    admitted: &AdmittedRun,
    execution: u64,
    values: &[(&str, String)],
) -> NodeHandle {
    runtime
        .start(request(admitted, execution, values))
        .await
        .assert_value()
}

async fn complete_with_logs(
    mut handle: NodeHandle,
) -> (String, Result<WorkerOutcome, NodeRunnerError>) {
    let mut output = handle.take_initial_output().assert_value();
    let (logs, completion) = tokio::join!(
        async {
            let mut logs = Vec::new();
            while let Ok(entry) = output.recv_output().await {
                logs.push(entry);
            }
            logs
        },
        handle.completion()
    );
    let logs = logs
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    (logs, completion.map(|completion| completion.outcome))
}

async fn complete_verified_with_logs(handle: NodeHandle) -> String {
    let (logs, completion) = complete_with_logs(handle).await;
    assert!(matches!(
        completion.assert_value(),
        WorkerOutcome::Verified { output, .. } if output == json!({"answer": 42})
    ));
    logs
}

fn assert_schema_capture(capture: &str) {
    let schema = capture
        .lines()
        .find_map(|line| line.strip_prefix("schema="))
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .assert_value();
    assert_eq!(
        schema
            .pointer("/properties/response/properties/answer")
            .assert_value(),
        &json!({"type":"integer"})
    );
    for path in capture
        .lines()
        .filter_map(|line| line.strip_prefix("schema_path="))
    {
        assert!(!Path::new(path).exists(), "schema file was not removed");
    }
}

fn assert_openrouter_capture(capture: &str) {
    for expected in [
        "arg=model_provider=\"openrouter\"",
        "arg=model_providers.openrouter.base_url=\"https://openrouter.ai/api/v1\"",
        "arg=model_providers.openrouter.env_key=\"OPENROUTER_API_KEY\"",
        "arg=model_providers.openrouter.wire_api=\"responses\"",
        "arg=--model\narg=openai/gpt-5.6-sol",
        "arg=model_reasoning_effort=\"max\"",
        "arg=--output-schema",
        "arg=--dangerously-bypass-approvals-and-sandbox",
        "Authored instructions:\nExercise the Codex adapter.",
        "Input JSON:\n{\"task\":\"change the workspace\"}",
        "Runtime-owned response contract:\n{\"kind\":\"worker\",\"output\":{\"kind\":\"record\"",
        "openrouter_key=fake-openrouter-key",
        "codex_key=unset",
        "path=/usr/bin:/bin",
        "ambient=unset",
    ] {
        assert!(
            capture.contains(expected),
            "missing capture evidence: {expected}"
        );
    }
    assert_schema_capture(capture);
    for suppressed in [
        "--ignore-user-config",
        "--ignore-rules",
        "--strict-config",
        "web_search=",
        "sandbox_workspace_write.network_access=",
    ] {
        assert!(!capture.contains(suppressed));
    }
}

#[tokio::test]
async fn openrouter_script_observes_exact_configuration_environment_output_and_attach() {
    let directory = TestDirectory::new("codex-openrouter");
    let capture = directory.child("capture");
    let adapter = scripted_adapter(&directory, CodexProvider::OpenRouter);
    let admitted = admitted(
        binding_with_model(
            "openai/gpt-5.6-sol",
            SessionScope::Execution,
            &["CAPTURE_PATH", "OPENROUTER_API_KEY"],
        ),
        CodexProvider::OpenRouter,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let mut handle = start(
        &runtime,
        &admitted,
        1,
        &[
            ("CAPTURE_PATH", capture.display().to_string()),
            ("OPENROUTER_API_KEY", "fake-openrouter-key".to_owned()),
        ],
    )
    .await;
    let mut attach = handle.take_initial_output().assert_value();
    let progress = attach.recv_output().await.assert_value();
    assert_eq!(progress.stream, LiveOutputStream::System);
    assert_eq!(progress.text, "Codex turn started");
    let attached = attach.recv_output().await.assert_value();
    assert_eq!(attached.stream, LiveOutputStream::Output);
    assert_eq!(attached.text, "visible [REDACTED]");
    assert_eq!(
        attach.recv_output().await.assert_value().text,
        r#"{"response":{"answer":42}}"#
    );
    let completion = handle.completion().await.assert_value();
    assert_eq!(
        completion.outcome,
        WorkerOutcome::Verified {
            output: json!({"answer":42}),
            artifacts: Vec::new(),
        }
    );
    let usage = attach.recv_usage().await.assert_value().assert_value();
    assert_eq!(
        serde_json::to_value(usage).assert_value(),
        json!({
            "inputTokens": 1,
            "outputTokens": 2,
            "cacheReadInputTokens": 1,
            "cacheCreationInputTokens": 3
        })
    );
    assert_eq!(attach.recv().await, Err(AttachReceiveError::Closed));

    let capture = fs::read_to_string(capture).assert_value();
    assert_openrouter_capture(&capture);
}

#[tokio::test]
async fn verifiers_and_workers_share_permission_defaults_and_preserve_authored_policy() {
    use crate::native_v2_capsule::provider_process::PermissionPolicy;
    use crate::native_v2_runner::test_support;

    for node in ["verify", "worker"] {
        for policy in [
            PermissionPolicy::Unset,
            PermissionPolicy::Configured,
            PermissionPolicy::Unavailable,
        ] {
            let directory = TestDirectory::new("codex-agent-policy");
            let response = if node == "verify" {
                json!({"response":{"output":null,"signals":{},"diagnostic":null}})
            } else {
                json!({"response":null})
            };
            let final_message = json!({"type":"item.completed","item":{
                "type":"agent_message","text":response.to_string()
            }});
            let script = format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" > arguments\n\
                 cat > prompt\nmkdir -p target\ntouch target/check-output\n\
                 printf '%s\\n' '{final_message}' '{{\"type\":\"turn.completed\"}}'\n"
            );
            let mut adapter =
                scripted_adapter_with(&directory, CodexProvider::OpenAi, "codex-policy", &script);
            let configuration = Arc::get_mut(&mut adapter).assert_value();
            configuration.test_permission_policy = Some(policy);
            configuration.config.local_user = Some(NativeV2CodexUser {
                home: directory.path().to_owned(),
                codex_home: directory.path().to_owned(),
            });
            let workspace = configuration.config.workspace.clone();
            let runtime = runner(&test_support::admitted(), adapter);
            runtime
                .start(test_support::request("policy", node, (1, 1)))
                .await
                .assert_value()
                .completion()
                .await
                .assert_value();
            let arguments = fs::read_to_string(workspace.join("arguments")).assert_value();
            assert_eq!(
                arguments.contains("--dangerously-bypass-approvals-and-sandbox"),
                policy == PermissionPolicy::Unset
            );
            assert!(!arguments.contains("--sandbox"));
            assert!(!arguments.contains("approval_policy"));
            assert!(workspace.join("target/check-output").exists());
            let prompt = fs::read_to_string(workspace.join("prompt")).assert_value();
            assert_eq!(
                prompt.contains("Runtime-owned verifier guidance:"),
                node == "verify"
            );
        }
    }
}

#[tokio::test]
async fn node_instance_session_reports_per_turn_codex_usage_deltas() {
    let directory = TestDirectory::new("codex-resumed-usage");
    let (admitted, runtime) = openai_scripted_runtime(
        &directory,
        OpenAiScript {
            scope: SessionScope::NodeInstance,
            environment: &["OPENAI_API_KEY"],
            name: "codex-resumed-usage",
            body: RESUMED_USAGE_SCRIPT,
        },
    )
    .await;
    let values = [("OPENAI_API_KEY", "fake-openai-key".to_owned())];

    let mut first_handle = start(&runtime, &admitted, 1, &values).await;
    let mut first_output = first_handle.take_initial_output().assert_value();
    assert!(matches!(
        first_handle.completion().await.assert_value().outcome,
        WorkerOutcome::Verified { output, .. } if output == json!({"answer":42})
    ));
    assert_eq!(
        first_output
            .recv_usage()
            .await
            .assert_value()
            .assert_value(),
        token_usage(1, 2, Some(1), Some(3))
    );

    let mut second_handle = start(&runtime, &admitted, 2, &values).await;
    let mut second_output = second_handle.take_initial_output().assert_value();
    assert!(matches!(
        second_handle.completion().await.assert_value().outcome,
        WorkerOutcome::Verified { output, .. } if output == json!({"answer":43})
    ));
    assert_eq!(
        second_output
            .recv_usage()
            .await
            .assert_value()
            .assert_value(),
        token_usage(2, 3, Some(1), Some(1))
    );
}

#[tokio::test]
async fn execution_session_normalizes_usage_for_correction_resume() {
    let directory = TestDirectory::new("codex-correction-usage");
    let (admitted, runtime) = openai_scripted_runtime(
        &directory,
        OpenAiScript {
            scope: SessionScope::Execution,
            environment: &["CORRECT_OUTPUT", "OPENAI_API_KEY"],
            name: "codex-correction-usage",
            body: RESUMED_USAGE_SCRIPT,
        },
    )
    .await;
    let values = [
        ("CORRECT_OUTPUT", "true".to_owned()),
        ("OPENAI_API_KEY", "fake-openai-key".to_owned()),
    ];
    let mut handle = start(&runtime, &admitted, 1, &values).await;
    let mut output = handle.take_initial_output().assert_value();

    assert!(matches!(
        handle.completion().await.assert_value().outcome,
        WorkerOutcome::Verified { output, .. } if output == json!({"answer":43})
    ));
    assert_eq!(
        output.recv_usage().await.assert_value().assert_value(),
        token_usage(1, 2, Some(1), Some(3))
    );
    assert_eq!(
        output.recv_usage().await.assert_value().assert_value(),
        token_usage(2, 3, Some(1), Some(1))
    );
}

#[tokio::test]
async fn openai_node_instance_session_resumes_the_exact_thread() {
    let directory = TestDirectory::new("codex-resume");
    let capture = directory.child("capture");
    let (admitted, runtime) = openai_runtime(
        &directory,
        SessionScope::NodeInstance,
        &["CAPTURE_PATH", "OPENAI_API_KEY"],
    )
    .await;
    let values = [
        ("CAPTURE_PATH", capture.display().to_string()),
        ("OPENAI_API_KEY", "fake-openai-key".to_owned()),
    ];
    let mut first_handle = start(&runtime, &admitted, 1, &values).await;
    let first = first_handle.completion().await.assert_value();
    let mut second_handle = start(&runtime, &admitted, 2, &values).await;
    let second = second_handle.completion().await.assert_value();
    assert!(matches!(
        first.outcome,
        WorkerOutcome::Verified { output, .. } if output == json!({"answer":42})
    ));
    assert!(matches!(
        second.outcome,
        WorkerOutcome::Verified { output, .. } if output == json!({"answer":43})
    ));
    let capture = fs::read_to_string(capture).assert_value();
    assert!(capture.contains("arg=model_provider=\"openai\""));
    assert!(capture.contains("arg=--model\narg=gpt-5.6-sol"));
    assert_eq!(capture.matches("arg=resume").count(), 1);
    assert_eq!(capture.matches("arg=thread-123").count(), 1);
    assert!(capture.contains("codex_key=fake-openai-key"));
    assert!(capture.contains("openrouter_key=unset"));
    let homes = capture
        .lines()
        .filter_map(|line| line.strip_prefix("codex_home="))
        .collect::<BTreeSet<_>>();
    assert_eq!(homes.len(), 1);
    assert!(
        homes
            .iter()
            .all(|home| home.ends_with("writer-node-instance-1"))
    );
    assert!(homes.iter().all(|home| Path::new(home).is_dir()));
    let scratches = capture
        .lines()
        .filter_map(|line| line.strip_prefix("scratch="))
        .collect::<Vec<_>>();
    crate::native_v2_candidate::test_support::assert_removed_directories(&scratches, 2);
    runtime
        .close_run(&openengine_cluster_protocol::RunId::new("run-codex"))
        .await;
    assert!(homes.iter().all(|home| !Path::new(home).exists()));
}
