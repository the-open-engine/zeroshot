#![cfg(unix)]

#[path = "tests/backpressure.rs"]
mod backpressure;
#[path = "tests/command.rs"]
mod command;
#[path = "tests/correction.rs"]
mod correction;
#[path = "tests/environment.rs"]
mod environment;
#[path = "tests/limits.rs"]
mod limits;
#[path = "tests/local_identity.rs"]
mod local_identity;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use openengine_cluster_protocol::{
    EnumLabel, FieldName, GraphSpec, IdempotencyKey, NodeName, RunSize, RunTitle, WorkerOutcome,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{json, Value};

use super::command::{
    ANTHROPIC_KEY, AWS_BEARER_TOKEN_BEDROCK, AWS_REGION, OPENROUTER_BASE_URL, OPENROUTER_KEY,
};
use super::{ClaudeAdapter, ClaudeAdapterConfig, ClaudeAdapterConfigError, ClaudeProcessEnvironment};
use crate::execution::{SessionScope, process::HostedProcessPool};
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;
use crate::native_v2_candidate::test_support::{
    NodeRequestFixture, TestDirectory, admit, environment_name, full_graph, success_node,
};
use crate::native_v2_contract::{
    ClaudeProvider, DeclaredConnections, DeclaredEnvironment, NodeRuntimeBinding, RunSubmission,
    RuntimePlan, TokenUsageDelta,
};
use crate::native_v2_runner::{
    AttachReceiveError, LiveOutputStream, NativeNodeRunner, NodeRunRequest, NodeRunner,
    NodeRunnerError, ResolvedEnvironment,
};
use crate::worker_catalog::{self, ReasoningEffort};

fn adapter_configuration(
    provider: ClaudeProvider,
    executable: &str,
    native_environment: BTreeMap<String, String>,
) -> ClaudeAdapterConfig {
    ClaudeAdapterConfig {
        provider,
        executable: executable.to_owned(),
        prefix_arguments: Vec::new(),
        workspace: PathBuf::from("/workspace"),
        runtime_home: PathBuf::from("/runtime"),
        local_user_home: Some(PathBuf::from("/user")),
        native_environment: LocalHarnessEnvironment::new(native_environment),
        base_environment: ClaudeProcessEnvironment::new(BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("TERM".to_owned(), "dumb".to_owned()),
        ]))
        .assert_value(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000).assert_value(),
    }
}

#[test]
fn coverage_contract_claude_configuration_redacts_values_and_rejects_invalid_launch_inputs() {
    let environment = ClaudeProcessEnvironment::new(BTreeMap::from([(
        "PATH".to_owned(),
        "secret-search-path".to_owned(),
    )]))
    .assert_value();
    let debug = format!("{environment:?}");
    assert!(debug.contains("PATH"));
    assert!(!debug.contains("secret-search-path"));
    assert!(matches!(
        ClaudeProcessEnvironment::new(BTreeMap::from([(
            "SECRET".to_owned(),
            "value".to_owned(),
        )])),
        Err(ClaudeAdapterConfigError::NonMinimalEnvironment(name)) if name == "SECRET"
    ));
    assert!(matches!(
        ClaudeProcessEnvironment::new(BTreeMap::from([(
            "PATH".to_owned(),
            "bad\0value".to_owned(),
        )])),
        Err(ClaudeAdapterConfigError::InvalidEnvironment)
    ));
    assert!(matches!(
        ClaudeAdapter::new_local(adapter_configuration(
            ClaudeProvider::Anthropic,
            "",
            BTreeMap::new(),
        )),
        Err(ClaudeAdapterConfigError::EmptyExecutable)
    ));
}

#[test]
fn coverage_contract_claude_local_configuration_preserves_only_provider_compatible_selectors() {
    let native = BTreeMap::from([
        ("ANTHROPIC_API_KEY".to_owned(), "ambient-key".to_owned()),
        ("CLAUDE_CODE_USE_BEDROCK".to_owned(), "selector".to_owned()),
        (
            "ANTHROPIC_BASE_URL".to_owned(),
            "https://native.invalid".to_owned(),
        ),
        ("HTTPS_PROXY".to_owned(), "https://proxy.invalid".to_owned()),
    ]);
    let anthropic = ClaudeAdapter::new_local(adapter_configuration(
        ClaudeProvider::Anthropic,
        "claude",
        native.clone(),
    ))
    .assert_value();
    assert_eq!(
        anthropic.local_environment.get("ANTHROPIC_API_KEY"),
        Some(&"ambient-key".to_owned())
    );
    assert!(
        anthropic
            .local_environment
            .contains_key("CLAUDE_CODE_USE_BEDROCK")
    );

    let openrouter = ClaudeAdapter::new_local(adapter_configuration(
        ClaudeProvider::OpenRouter,
        "claude",
        native,
    ))
    .assert_value();
    assert!(
        !openrouter
            .local_environment
            .contains_key("ANTHROPIC_API_KEY")
    );
    assert!(
        !openrouter
            .local_environment
            .contains_key("CLAUDE_CODE_USE_BEDROCK")
    );
    assert!(
        openrouter
            .local_environment
            .contains_key("ANTHROPIC_BASE_URL")
    );
    assert!(openrouter.local_environment.contains_key("HTTPS_PROXY"));

    let hosted = ClaudeAdapter::new(adapter_configuration(
        ClaudeProvider::Anthropic,
        "claude",
        BTreeMap::from([("ANTHROPIC_API_KEY".to_owned(), "secret".to_owned())]),
    ))
    .assert_value();
    assert!(hosted.local_environment.is_empty());
    assert!(hosted.local_user_home.is_none());
}

#[test]
fn coverage_contract_claude_declared_auth_wins_and_reserved_or_invalid_paths_fail_closed() {
    let adapter = ClaudeAdapter::new_local(adapter_configuration(
        ClaudeProvider::Anthropic,
        "claude",
        BTreeMap::from([("ANTHROPIC_API_KEY".to_owned(), "ambient".to_owned())]),
    ))
    .assert_value();
    let declared = agent_binding(
        "claude-provider-model",
        None,
        SessionScope::Execution,
        &["ANTHROPIC_API_KEY"],
    );
    let resolved = ResolvedEnvironment::exact(
        &declared,
        BTreeMap::from([(environment_name("ANTHROPIC_API_KEY"), "declared".to_owned())]),
    )
    .assert_value();
    let environment = adapter
        .process_environment(&resolved, Path::new("/runtime"))
        .assert_value();
    assert_eq!(
        environment.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("declared")
    );
    assert!(
        adapter
            .process_environment(&resolved, Path::new(""))
            .is_err()
    );

    let reserved = agent_binding(
        "claude-provider-model",
        None,
        SessionScope::Execution,
        &["HOME"],
    );
    let reserved = ResolvedEnvironment::exact(
        &reserved,
        BTreeMap::from([(environment_name("HOME"), "declared-home".to_owned())]),
    )
    .assert_value();
    let error = adapter
        .process_environment(&reserved, Path::new("/runtime"))
        .err()
        .assert_value();
    assert!(error.to_string().contains("reserved process configuration"));
}

fn agent_binding(
    model: &str,
    effort: Option<ReasoningEffort>,
    scope: SessionScope,
    environment: &[&str],
) -> NodeRuntimeBinding {
    let connections = if environment.is_empty() {
        DeclaredConnections::empty()
    } else {
        DeclaredConnections::single(
            "provider",
            DeclaredEnvironment::new(environment.iter().map(|name| environment_name(name)))
                .assert_value(),
        )
        .assert_value()
    };
    NodeRuntimeBinding::Agent {
        model: worker_catalog::ModelId::new(model).assert_value(),
        effort,
        session_scope: scope,
        connections,
    }
}

fn graph(verifier: bool) -> GraphSpec {
    let executable = if verifier {
        json!({
            "kind":"verifier", "name":"agent", "worker":"agent.claude@1",
            "instructions":"Exercise the Claude adapter.",
            "input":{"kind":"null"}, "output":{"kind":"null"},
            "inputBindings":[], "writeBindings":[], "timeoutMs":60000, "attempts":1,
            "signals":{"verdict":["accepted","rejected"]}, "diagnostic":{"kind":"null"}
        })
    } else {
        json!({
            "kind":"step", "name":"agent", "worker":"agent.claude@1",
            "instructions":"Exercise the Claude adapter.",
            "input":{"kind":"null"}, "output":{"kind":"string"},
            "inputBindings":[], "writeBindings":[], "timeoutMs":60000, "attempts":1
        })
    };
    full_graph(vec![executable, success_node()])
}

async fn runner(
    workspace: &TestDirectory,
    provider: ClaudeProvider,
    binding: NodeRuntimeBinding,
    verifier: bool,
) -> NativeNodeRunner {
    runner_with_command(
        workspace,
        TestRunnerConfig {
            provider,
            binding,
            verifier,
            command: TestProviderCommand {
                executable: "/bin/sh".to_owned(),
                prefix_arguments: vec![
                    workspace
                        .path()
                        .join("fake-claude.sh")
                        .to_string_lossy()
                        .into_owned(),
                ],
                base_environment: BTreeMap::new(),
            },
        },
    )
    .await
}

struct TestProviderCommand {
    executable: String,
    prefix_arguments: Vec<String>,
    base_environment: BTreeMap<String, String>,
}

struct TestRunnerConfig {
    provider: ClaudeProvider,
    binding: NodeRuntimeBinding,
    verifier: bool,
    command: TestProviderCommand,
}

async fn runner_with_command(
    workspace: &TestDirectory,
    configuration: TestRunnerConfig,
) -> NativeNodeRunner {
    let TestRunnerConfig {
        provider,
        binding,
        verifier,
        command,
    } = configuration;
    let runtime = RuntimePlan::Claude {
        provider,
        size: RunSize::Medium,
        nodes: BTreeMap::from([(NodeName::new("agent").assert_value(), binding)]),
    };
    let admitted = admit(RunSubmission {
        environment: None,
        title: RunTitle::new("Claude adapter test").assert_value(),
        graph: graph(verifier),
        initial_input: Value::Null,
        runtime,
        source: serde_json::from_value(json!({
            "repository": "open-engine/zeroshot",
            "branch": "main",
            "revision": "0123456789abcdef0123456789abcdef01234567"
        }))
        .assert_value(),
        submission_key: IdempotencyKey::new("claude-test").assert_value(),
    })
    .await;
    let mut base_environment = BTreeMap::from([
        (
            "HOME".to_owned(),
            workspace.path().to_string_lossy().into_owned(),
        ),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
    ]);
    base_environment.extend(command.base_environment);
    let base_environment = ClaudeProcessEnvironment::new(base_environment).assert_value();
    let adapter = Arc::new(
        ClaudeAdapter::new_for_test(ClaudeAdapterConfig {
            provider,
            executable: command.executable,
            prefix_arguments: command.prefix_arguments,
            workspace: workspace.path().to_owned(),
            runtime_home: workspace.path().to_owned(),
            local_user_home: None,
            native_environment: Default::default(),
            base_environment,
            process_pool: HostedProcessPool::new(10_002, 10_002, 20_000).assert_value(),
        })
        .assert_value(),
    );
    NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value()
}

fn request(binding: NodeRuntimeBinding, execution: u64, values: &[(&str, &str)]) -> NodeRunRequest {
    let environment = values
        .iter()
        .map(|(name, value)| (environment_name(name), (*value).to_owned()))
        .collect::<BTreeMap<_, _>>();
    NodeRequestFixture {
        run_id: "claude-run",
        node: "agent",
        node_instance: 1,
        execution,
        worker: "agent.claude@1",
        instructions: "Exercise the Claude adapter.",
        input: Value::String("perform the node task".to_owned()),
        binding,
        environment,
    }
    .into_request()
}

async fn anthropic_runner(
    workspace: &TestDirectory,
    model: &str,
    effort: Option<ReasoningEffort>,
    scope: SessionScope,
) -> (NativeNodeRunner, NodeRuntimeBinding) {
    let binding = agent_binding(model, effort, scope, &[ANTHROPIC_KEY]);
    let runtime = runner(workspace, ClaudeProvider::Anthropic, binding.clone(), false).await;
    (runtime, binding)
}

async fn start_anthropic(
    workspace: &TestDirectory,
    model: &str,
    effort: Option<ReasoningEffort>,
) -> crate::native_v2_runner::NodeHandle {
    let (runtime, binding) =
        anthropic_runner(workspace, model, effort, SessionScope::Execution).await;
    runtime
        .start(request(binding, 1, &[(ANTHROPIC_KEY, "anthropic-fake")]))
        .await
        .assert_value()
}

fn assert_token_usage(usage: Option<TokenUsageDelta>, expected: [u64; 4]) {
    let usage = usage.assert_value();
    assert_eq!(usage.input_tokens.get(), expected[0]);
    assert_eq!(usage.output_tokens.get(), expected[1]);
    assert_eq!(
        usage.cache_read_input_tokens.assert_value().get(),
        expected[2]
    );
    assert_eq!(
        usage.cache_creation_input_tokens.assert_value().get(),
        expected[3]
    );
}

async fn cancel_and_assert_reaped(
    mut handle: crate::native_v2_runner::NodeHandle,
    workspace: &TestDirectory,
    child_pid: u32,
) {
    handle.cancel();
    assert_eq!(handle.completion().await, Err(NodeRunnerError::Cancelled));
    let process = format!("/proc/{child_pid}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while Path::new(&process).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value_with("provider child remained alive after cancellation");
    assert!(!workspace.child("survivor.txt").exists());
}

async fn complete_two_turns(runner: &NativeNodeRunner, binding: &NodeRuntimeBinding) {
    for execution in [1, 2] {
        complete_turn(runner, binding, execution).await;
    }
}

async fn complete_turn(
    runner: &NativeNodeRunner,
    binding: &NodeRuntimeBinding,
    execution: u64,
) -> WorkerOutcome {
    runner
        .start(request(
            binding.clone(),
            execution,
            &[(ANTHROPIC_KEY, "anthropic-fake")],
        ))
        .await
        .assert_value()
        .completion()
        .await
        .assert_value()
        .outcome
}

async fn two_turn_workspace(
    label: &str,
    model: &str,
    effort: ReasoningEffort,
    scope: SessionScope,
) -> TestDirectory {
    let workspace = TestDirectory::new(label);
    workspace.write("fake-claude.sh", SUCCESS_SCRIPT);
    let binding = agent_binding(model, Some(effort), scope, &[ANTHROPIC_KEY]);
    let runner = runner(
        &workspace,
        ClaudeProvider::Anthropic,
        binding.clone(),
        false,
    )
    .await;
    complete_two_turns(&runner, &binding).await;
    let homes = workspace.read("homes.txt");
    assert!(
        homes
            .lines()
            .all(|home| Path::new(home).exists() == (scope == SessionScope::NodeInstance))
    );
    let scratches = workspace.read("scratches.txt");
    crate::native_v2_candidate::test_support::assert_removed_directories(
        &scratches.lines().collect::<Vec<_>>(),
        2,
    );
    runner
        .close_run(&openengine_cluster_protocol::RunId::new("claude-run"))
        .await;
    assert!(homes.lines().all(|home| !Path::new(home).exists()));
    workspace
}

fn assert_resumed_session(arguments: &str) {
    assert!(arguments.lines().any(|line| line == "--resume"));
    assert!(arguments.lines().any(|line| line == "session-1"));
}

const SUCCESS_SCRIPT: &str = r#"
set -eu
target=initial
previous=
for argument in "$@"; do
  if [ "$previous" = "--resume" ]; then target=resumed; fi
  previous=$argument
done
: > "$target.args"
for argument in "$@"; do printf '%s\n' "$argument" >> "$target.args"; done
cat > "$target.prompt"
printf '%s\n' "${ANTHROPIC_API_KEY-unset}" > anthropic-key.txt
printf '%s\n' "${ANTHROPIC_AUTH_TOKEN-unset}" > anthropic-token.txt
printf '%s\n' "${ANTHROPIC_BASE_URL-unset}" > anthropic-base-url.txt
printf '%s\n' "${OPENROUTER_API_KEY-unset}" > openrouter-key.txt
printf '%s\n' "${AWS_BEARER_TOKEN_BEDROCK-unset}" > bedrock-key.txt
printf '%s\n' "${AWS_REGION-unset}" > aws-region.txt
printf '%s\n' "${CLAUDE_CODE_USE_BEDROCK-unset}" > use-bedrock.txt
printf '%s\n' "${UNDECLARED_AMBIENT_SENTINEL-unset}" > ambient.txt
printf '%s\n' "$HOME" >> homes.txt
printf '%s\n' "$TMPDIR" >> scratches.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"session-1"}'
printf '%s%s\n' \
  '{"type":"stream_event","event":{"type":"content_block_delta",' \
  '"delta":{"type":"text_delta","text":"visible sentinel-secret"}}}'
if [ "${CORRECT_OUTPUT-false}" = true ] && [ "$target" = initial ]; then
  result=done
else
  result='{\"response\":\"done\"}'
fi
printf '%s%s%s%s\n' \
  '{"type":"result","subtype":"success","is_error":false,"result":"' \
  "$result" \
  '","session_id":"session-1","usage":{"input_tokens":11,"output_tokens":4,' \
  '"cache_read_input_tokens":6,"cache_creation_input_tokens":2}}'
"#;

#[tokio::test]
async fn node_instance_scope_resumes_the_exact_claude_session() {
    let workspace = two_turn_workspace(
        "claude-resume",
        "claude-opus-5",
        ReasoningEffort::High,
        SessionScope::NodeInstance,
    )
    .await;
    assert!(workspace.child("initial.args").exists());
    let resumed = workspace.read("resumed.args");
    assert_resumed_session(&resumed);
    assert!(resumed.lines().any(|line| line == "high"));
    let homes = workspace
        .read("homes.txt")
        .lines()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(homes.len(), 1);
    assert!(
        homes
            .iter()
            .all(|home| home.ends_with("writer-node-instance-1"))
    );
}

#[tokio::test]
async fn execution_scope_never_resumes_a_prior_turn() {
    let workspace = two_turn_workspace(
        "claude-execution",
        "claude-sonnet-5",
        ReasoningEffort::Max,
        SessionScope::Execution,
    )
    .await;
    assert!(workspace.child("initial.args").exists());
    assert!(!workspace.child("resumed.args").exists());
    let homes = workspace
        .read("homes.txt")
        .lines()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(homes.len(), 2);
    assert!(
        homes
            .iter()
            .any(|home| home.ends_with("writer-execution-1"))
    );
    assert!(
        homes
            .iter()
            .any(|home| home.ends_with("writer-execution-2"))
    );
}

#[tokio::test]
async fn verifier_result_is_normalized_to_the_closed_worker_outcome() {
    let workspace = TestDirectory::new("claude-verifier");
    workspace.write(
        "fake-claude.sh",
        r#"
set -eu
cat > verifier.prompt
printf '%s\n' "$@" > verifier.args
printf '%s\n' '{"type":"system","subtype":"init","session_id":"verifier-session"}'
printf '%s%s%s%s\n' \
  '{"type":"result","subtype":"success","is_error":false,' \
  '"result":"ignored","structured_output":{"response":{"output":null,' \
  '"signals":{"verdict":"accepted"},' \
  '"diagnostic":null}}}'
"#,
    );
    let binding = agent_binding(
        "claude-fable-5",
        Some(ReasoningEffort::Xhigh),
        SessionScope::Execution,
        &[ANTHROPIC_KEY],
    );
    let runner = runner(&workspace, ClaudeProvider::Anthropic, binding.clone(), true).await;
    let completion = runner
        .start(request(binding, 1, &[(ANTHROPIC_KEY, "anthropic-fake")]))
        .await
        .assert_value()
        .completion()
        .await
        .assert_value();
    assert_eq!(
        completion.outcome,
        WorkerOutcome::Verifier {
            output: Value::Null,
            signals: BTreeMap::from([(
                FieldName::new("verdict").assert_value(),
                EnumLabel::new("accepted").assert_value(),
            )]),
            diagnostic: Value::Null,
            artifacts: Vec::new(),
        }
    );
    assert!(
        workspace
            .read("verifier.prompt")
            .contains("Runtime-owned verifier guidance:")
    );
    let arguments = workspace.read("verifier.args");
    assert!(!arguments.contains("--permission-mode"));
    assert!(!arguments.contains("--tools"));
    assert!(!arguments.contains("--setting-sources"));
    assert!(arguments.contains("--dangerously-skip-permissions"));
    assert!(
        workspace
            .read("verifier.prompt")
            .contains("\"signals\":{\"verdict\":[\"accepted\",\"rejected\"]}")
    );
}

#[tokio::test]
async fn cancellation_reaps_the_script_and_its_child_before_completion() {
    let workspace = TestDirectory::new("claude-cancel");
    workspace.write(
        "fake-claude.sh",
        r#"
set -eu
cat >/dev/null
(sleep 30; printf survived > survivor.txt) &
printf '%s' "$!" > child.pid
printf '%s%s\n' \
  '{"type":"stream_event","event":{"type":"content_block_delta",' \
  '"delta":{"type":"text_delta","text":"started"}}}'
wait
"#,
    );
    let mut handle = start_anthropic(&workspace, "claude-haiku-4-5", None).await;
    let mut attach = handle.take_initial_output().assert_value();
    assert_eq!(attach.recv_output().await.assert_value().text, "started");
    let child_pid: u32 = workspace.read("child.pid").parse().assert_value();
    cancel_and_assert_reaped(handle, &workspace, child_pid).await;
}

#[path = "tests/failure.rs"]
mod failure;
