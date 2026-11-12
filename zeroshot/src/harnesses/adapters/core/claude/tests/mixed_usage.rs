use super::*;
use crate::harnesses::adapters::core::codex::{NativeV2CodexAdapter, NativeV2CodexConfig};
use crate::native_v2_contract::CodexProvider;

async fn invocation_usage(
    runtime: &NativeNodeRunner,
    binding: NodeRuntimeBinding,
    key: &str,
) -> TokenUsageDelta {
    let mut handle = runtime
        .start(request(binding, 1, &[(key, "test-key")]))
        .await
        .assert_value();
    let mut output = handle.take_initial_output().assert_value();
    assert!(matches!(
        handle.completion().await.assert_value().outcome,
        WorkerOutcome::Verified { .. }
    ));
    output.recv_usage().await.assert_value().assert_value()
}

#[tokio::test]
async fn claude_and_codex_invocations_sum_disjoint_token_buckets() {
    let workspace = TestDirectory::new("mixed-harness-usage");
    write_claude_script(
        &workspace,
        r#"
cat >/dev/null
printf '%s\n' '{"type":"result","subtype":"success","result":"{\"response\":\"done\"}","model_usage_since_last_result":{"model":{"inputTokens":2,"outputTokens":3,"cacheReadInputTokens":5,"cacheCreationInputTokens":7}}}'
"#,
    );
    let (claude, claude_binding) =
        anthropic_runner(&workspace, "test-model", None, SessionScope::Execution).await;
    let claude_usage = invocation_usage(&claude, claude_binding, ANTHROPIC_KEY).await;

    let codex_binding = agent_binding(
        "test-model",
        None,
        SessionScope::Execution,
        &["OPENAI_API_KEY"],
    );
    let admitted = admit(RunSubmission {
        environment: None,
        title: RunTitle::new("Mixed harness usage").assert_value(),
        graph: graph(false),
        initial_input: Value::Null,
        runtime: RuntimePlan::new(
            openengine_cluster_protocol::RuntimeLane::new("codex", "openai").assert_value(),
            run_size("medium"),
            BTreeMap::from([(NodeName::new("agent").assert_value(), codex_binding.clone())]),
        ),
        source: None,
        submission_key: IdempotencyKey::new("mixed-usage").assert_value(),
    })
    .await;
    let executable = workspace.write_executable("fake-codex", r#"#!/bin/sh
if [ "${1-}" = app-server ]; then exit 0; fi
cat >/dev/null
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":\"done\"}"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":30,"cache_write_input_tokens":20,"output_tokens":7}}'
"#);
    let adapter = Arc::new(NativeV2CodexAdapter::new_local(NativeV2CodexConfig {
        provider: CodexProvider::OpenAi,
        executable,
        workspace: workspace.path().to_owned(),
        runtime_home: workspace.path().to_owned(),
        local_user: None,
        native_environment: Default::default(),
        search_path: "/usr/bin:/bin".to_owned(),
        base_environment: Default::default(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000).assert_value(),
    }));
    let codex = NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value();
    let codex_usage = invocation_usage(&codex, codex_binding, "OPENAI_API_KEY").await;

    assert_eq!(claude_usage.input_tokens.get(), 2);
    assert_eq!(codex_usage.input_tokens.get(), 50);
    let total = claude_usage.checked_add(codex_usage).assert_value();
    assert_eq!(total.input_tokens.get(), 52);
    assert_eq!(total.output_tokens.get(), 10);
    assert_eq!(total.cache_read_input_tokens.assert_value().get(), 35);
    assert_eq!(total.cache_creation_input_tokens.assert_value().get(), 27);
    assert_eq!(
        total.input_tokens.get()
            + total.output_tokens.get()
            + total.cache_read_input_tokens.assert_value().get()
            + total.cache_creation_input_tokens.assert_value().get(),
        2 + 3 + 5 + 7 + 100 + 7
    );
}
