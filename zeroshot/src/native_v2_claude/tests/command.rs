use super::*;

struct ProviderFixture {
    model: &'static str,
    provider_value: &'static str,
    environment: Vec<&'static str>,
    values: Vec<(&'static str, &'static str)>,
}

fn provider_fixture(provider: ClaudeProvider) -> ProviderFixture {
    match provider {
        ClaudeProvider::Anthropic => ProviderFixture {
            model: "claude-sonnet-5",
            provider_value: "anthropic-fake",
            environment: vec![ANTHROPIC_KEY],
            values: vec![(ANTHROPIC_KEY, "anthropic-fake")],
        },
        ClaudeProvider::OpenRouter => ProviderFixture {
            model: "anthropic/provider-owned-model",
            provider_value: "openrouter-fake",
            environment: vec![OPENROUTER_KEY],
            values: vec![(OPENROUTER_KEY, "openrouter-fake")],
        },
        ClaudeProvider::Bedrock => ProviderFixture {
            model: "global.anthropic.provider-owned-model",
            provider_value: "bedrock-fake",
            environment: vec![AWS_BEARER_TOKEN_BEDROCK, AWS_REGION],
            values: vec![
                (AWS_BEARER_TOKEN_BEDROCK, "bedrock-fake"),
                (AWS_REGION, "us-east-1"),
            ],
        },
    }
}

fn assert_provider_environment(
    workspace: &TestDirectory,
    provider: ClaudeProvider,
    provider_value: &str,
) {
    let observed = [
        workspace.read("anthropic-key.txt"),
        workspace.read("anthropic-token.txt"),
        workspace.read("anthropic-base-url.txt"),
        workspace.read("openrouter-key.txt"),
        workspace.read("bedrock-key.txt"),
        workspace.read("aws-region.txt"),
        workspace.read("use-bedrock.txt"),
    ]
    .map(|value| value.trim().to_owned());
    let expected = match provider {
        ClaudeProvider::Anthropic => [
            provider_value,
            "unset",
            "unset",
            "unset",
            "unset",
            "unset",
            "unset",
        ],
        ClaudeProvider::OpenRouter => [
            "",
            provider_value,
            OPENROUTER_BASE_URL,
            provider_value,
            "unset",
            "unset",
            "unset",
        ],
        ClaudeProvider::Bedrock => [
            "unset",
            "unset",
            "unset",
            "unset",
            provider_value,
            "us-east-1",
            "1",
        ],
    }
    .map(str::to_owned);
    assert_eq!(observed, expected);
}

#[tokio::test]
async fn scripted_provider_commands_are_exact_and_ambient_free() {
    for provider in [
        ClaudeProvider::Anthropic,
        ClaudeProvider::OpenRouter,
        ClaudeProvider::Bedrock,
    ] {
        let workspace = TestDirectory::new("claude-command");
        workspace.write("fake-claude.sh", SUCCESS_SCRIPT);
        let ProviderFixture {
            model,
            provider_value,
            mut environment,
            mut values,
        } = provider_fixture(provider);
        environment.push("TEST_SECRET");
        values.push(("TEST_SECRET", "sentinel-secret"));
        let binding = agent_binding(
            model,
            Some(ReasoningEffort::Max),
            SessionScope::Execution,
            &environment,
        );
        let runner = runner(&workspace, provider, binding.clone(), false).await;
        let mut handle = runner
            .start(request(binding, 1, &values))
            .await
            .assert_value();
        let mut attach = handle.take_initial_output().assert_value();
        let (live, completion) = tokio::join!(attach.recv_output(), handle.completion());
        assert_eq!(live.assert_value().text, "visible [REDACTED]");
        assert_eq!(
            completion.assert_value().outcome,
            WorkerOutcome::Verified {
                output: json!("done"),
                artifacts: Vec::new(),
            }
        );
        assert_token_usage(attach.recv_usage().await.assert_value(), [11, 4, 6, 2]);
        assert_eq!(attach.recv().await, Err(AttachReceiveError::Closed));
        let arguments = workspace.read("initial.args");
        let expected_prefix = format!(
            concat!(
                "--print\n--input-format\ntext\n--output-format\nstream-json\n",
                "--verbose\n--include-partial-messages\n--model\n{}\n--json-schema\n",
            ),
            model
        );
        assert!(arguments.starts_with(&expected_prefix));
        let schema = arguments
            .lines()
            .skip_while(|line| *line != "--json-schema")
            .nth(1)
            .and_then(|line| serde_json::from_str::<Value>(line).ok())
            .assert_value();
        assert_eq!(
            schema.pointer("/properties/response").assert_value(),
            &json!({"type":"string"})
        );
        assert!(arguments.contains("--effort\nmax\n--dangerously-skip-permissions\n"));
        assert!(!arguments.contains("--setting-sources"));
        assert!(!arguments.contains("perform the node task"));
        let prompt = workspace.read("initial.prompt");
        assert!(prompt.contains("Authored instructions:\nExercise the Claude adapter."));
        assert!(prompt.contains("Input JSON:\n\"perform the node task\""));
        assert!(prompt.contains("Runtime-owned response contract:\n{\"kind\":\"worker\""));
        assert_eq!(workspace.read("ambient.txt").trim(), "unset");
        assert_provider_environment(&workspace, provider, provider_value);
    }
}

#[test]
fn private_verifier_workspaces_allow_noninteractive_build_side_effects() {
    use crate::native_v2_claude::command::{ClaudeTurnArguments, claude_arguments};
    use crate::native_v2_runner::NodeRole;

    let arguments = claude_arguments(
        Vec::new(),
        ClaudeTurnArguments {
            model: "provider-owned-model",
            effort: None,
            role: NodeRole::Verifier,
            private_workspace: true,
            resume_id: Some("same-session"),
            json_schema: "{}".to_owned(),
        },
    )
    .assert_value();
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "--dangerously-skip-permissions")
    );
    assert!(
        !arguments
            .iter()
            .any(|argument| argument == "--permission-mode")
    );
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--resume", "same-session"])
    );
}
