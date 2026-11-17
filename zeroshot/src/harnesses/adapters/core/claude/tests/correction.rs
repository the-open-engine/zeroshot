use super::*;

async fn correction_runner(
    workspace: &TestDirectory,
    script: &str,
    environment: &[&str],
) -> (NativeNodeRunner, NodeRuntimeBinding) {
    write_claude_script(workspace, script);
    let binding = agent_binding(
        "claude-sonnet-5",
        Some(ReasoningEffort::Max),
        SessionScope::Execution,
        environment,
    );
    let runtime = runner(workspace, ClaudeProvider::Anthropic, binding.clone(), false).await;
    (runtime, binding)
}

#[tokio::test]
async fn invalid_output_is_corrected_in_the_same_claude_session() {
    let workspace = TestDirectory::new("claude-correction");
    let (runner, binding) = correction_runner(
        &workspace,
        SUCCESS_SCRIPT,
        &[ANTHROPIC_KEY, "CORRECT_OUTPUT"],
    )
    .await;
    let handle = runner
        .start(request(
            binding,
            1,
            &[
                (ANTHROPIC_KEY, "anthropic-fake"),
                ("CORRECT_OUTPUT", "true"),
            ],
        ))
        .await
        .assert_value();
    let (captured, completion) = super::failure::complete_with_durable(handle).await;
    let completion = completion.assert_value();
    let logs = captured.logs.join("\n");
    assert_eq!(
        logs.matches(
            "final output rejected: provider response must be exactly an object containing response"
        )
        .count(),
        1
    );
    assert!(!logs.contains("Your previous final response was rejected mechanically"));

    assert!(matches!(
        completion.outcome,
        WorkerOutcome::Verified { output, .. } if output == json!("done")
    ));
    assert_resumed_session(&workspace.read("resumed.args"));
    let prompt = workspace.read("resumed.prompt");
    assert_tool_delivery(&workspace.read("initial.prompt"));
    assert_tool_delivery(&prompt);
    assert!(prompt.contains("Your previous final response was rejected mechanically"));
    assert!(prompt.contains("provider response must be exactly an object containing response"));
}

#[tokio::test]
async fn malformed_output_stops_after_two_claude_correction_turns() {
    let workspace = TestDirectory::new("claude-malformed-limit");
    let (runner, binding) = correction_runner(
        &workspace,
        r#"
set -eu
cat >/dev/null
printf '%s\n' "$@" >> attempts.args
printf '%s\n' '---' >> attempts.args
printf '%s\n' '{"type":"system","subtype":"init","session_id":"malformed-session"}'
printf '%s%s\n' \
  '{"type":"result","subtype":"success","is_error":false,' \
  '"result":"not-json","session_id":"malformed-session"}'
"#,
        &[ANTHROPIC_KEY],
    )
    .await;
    let handle = runner
        .start(request(binding, 1, &[(ANTHROPIC_KEY, "anthropic-fake")]))
        .await
        .assert_value();
    let (captured, completion) = super::failure::complete_with_durable(handle).await;
    assert_eq!(
        completion.assert_value().outcome,
        WorkerOutcome::malformed()
    );
    let logs = captured.logs.join("\n");
    assert_eq!(
        logs.matches(
            "final output rejected: provider response must be exactly an object containing response"
        )
        .count(),
        3
    );
    assert_eq!(workspace.read("attempts.args").matches("---").count(), 3);
}

#[tokio::test]
async fn gateway_correction_keeps_endpoint_and_key_on_resume() {
    let workspace = TestDirectory::new("claude-gateway-correction");
    write_claude_script(&workspace, SUCCESS_SCRIPT);
    let binding = agent_binding(
        "opaque/model",
        None,
        SessionScope::Execution,
        &["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "CORRECT_OUTPUT"],
    );
    let runtime = runner(&workspace, ClaudeProvider::Gateway, binding.clone(), false).await;
    let completion = runtime
        .start(request(
            binding,
            1,
            &[
                ("GATEWAY_BASE_URL", "https://gateway.example/anthropic"),
                ("GATEWAY_API_KEY", "gateway-key"),
                ("CORRECT_OUTPUT", "true"),
            ],
        ))
        .await
        .assert_value()
        .completion()
        .await
        .assert_value();
    assert_eq!(
        completion.outcome,
        WorkerOutcome::Verified {
            output: json!("done"),
            artifacts: Vec::new()
        }
    );
    assert_resumed_session(&workspace.read("resumed.args"));
    assert_tool_delivery(&workspace.read("initial.prompt"));
    assert_tool_delivery(&workspace.read("resumed.prompt"));
    assert_eq!(
        workspace.read("anthropic-base-url.txt").trim(),
        "https://gateway.example/anthropic"
    );
    assert_eq!(workspace.read("anthropic-key.txt").trim(), "gateway-key");
    assert_eq!(workspace.read("anthropic-token.txt").trim(), "unset");
}

#[tokio::test]
async fn object_null_and_verifier_corrections_use_native_tool_delivery_on_stdin() {
    let object = json!({"kind":"record","fields":{
        "answer":{"type":{"kind":"integer"},"required":true}
    }});
    for (verifier, output_type, value) in [
        (false, object.clone(), json!({"answer":42})),
        (false, json!({"kind":"null"}), Value::Null),
        (
            true,
            object,
            json!({"output":{"answer":42},"signals":{"verdict":"accepted"},"diagnostic":null}),
        ),
    ] {
        let workspace = TestDirectory::new("claude-tool-delivery");
        write_claude_script(
            &workspace,
            r#"
set -eu
target=initial
for arg in "$@"; do
  if [ "$arg" = "--resume" ]; then target=resumed; fi
done
cat > "$target.prompt"
printf '%s\n' "$@" > "$target.args"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"tool-session"}'
if [ "$target" = initial ]; then
  printf '%s\n' '{"type":"result","subtype":"success","structured_output":{"response":false},"session_id":"tool-session"}'
else
  cat answer.json
fi
"#,
        );
        workspace.write(
            "answer.json",
            &json!({
                "type":"result","subtype":"success","session_id":"tool-session",
                "result":"ignored","structured_output":{"response":value}
            })
            .to_string(),
        );
        let mut source_graph = serde_json::to_value(graph(verifier)).assert_value();
        source_graph["root"]["children"][0]["output"] = output_type;
        let binding = agent_binding(
            "opaque-model",
            None,
            SessionScope::Execution,
            &[ANTHROPIC_KEY],
        );
        let runtime = runner_with_command(
            &workspace,
            TestRunnerConfig {
                provider: ClaudeProvider::Anthropic,
                binding: binding.clone(),
                graph: serde_json::from_value(source_graph).assert_value(),
                command: TestProviderCommand {
                    executable: "/bin/sh".to_owned(),
                    prefix_arguments: vec![
                        workspace
                            .child("fake-claude.sh")
                            .to_string_lossy()
                            .into_owned(),
                    ],
                    base_environment: BTreeMap::new(),
                },
            },
        )
        .await;
        let completion = runtime
            .start(request(binding, 1, &[(ANTHROPIC_KEY, "fixture-key")]))
            .await
            .assert_value()
            .completion()
            .await
            .assert_value();
        if verifier {
            assert!(
                matches!(completion.outcome,WorkerOutcome::Verifier {output, signals, diagnostic, ..}
                if output == value["output"] && signals.len() == 1 && diagnostic.is_null())
            );
        } else {
            assert!(
                matches!(completion.outcome,WorkerOutcome::Verified {output,..} if output == value)
            );
        }
        for turn in ["initial", "resumed"] {
            assert_tool_delivery(&workspace.read(&format!("{turn}.prompt")));
            assert!(
                workspace
                    .read(&format!("{turn}.args"))
                    .contains("--json-schema\n")
            );
        }
        assert!(
            workspace
                .read("resumed.args")
                .contains("--resume\ntool-session\n")
        );
    }
}
