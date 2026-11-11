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
        ClaudeProvider::Gateway => ProviderFixture {
            model: "provider-owned-model",
            provider_value: "gateway-fake",
            environment: vec!["GATEWAY_BASE_URL", "GATEWAY_API_KEY"],
            values: vec![
                ("GATEWAY_BASE_URL", "https://gateway.example/anthropic"),
                ("GATEWAY_API_KEY", "gateway-fake"),
            ],
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
        ClaudeProvider::Foundry => ProviderFixture {
            model: "provider-owned-deployment",
            provider_value: "foundry-fake",
            environment: vec!["AZURE_FOUNDRY_API_KEY", "AZURE_FOUNDRY_RESOURCE"],
            values: vec![
                ("AZURE_FOUNDRY_API_KEY", "foundry-fake"),
                ("AZURE_FOUNDRY_RESOURCE", "contoso-ai"),
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
        workspace.read("foundry-key.txt"),
        workspace.read("foundry-resource.txt"),
        workspace.read("azure-foundry-key.txt"),
        workspace.read("use-foundry.txt"),
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
            "unset",
            "unset",
            "unset",
            "unset",
        ],
        ClaudeProvider::Gateway => [
            provider_value,
            "unset",
            "https://gateway.example/anthropic",
            "unset",
            "unset",
            "unset",
            "unset",
            "unset",
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
            "unset",
            "unset",
            "unset",
            "unset",
        ],
        ClaudeProvider::Foundry => [
            "unset",
            "unset",
            "unset",
            "unset",
            "unset",
            "unset",
            "unset",
            provider_value,
            "contoso-ai",
            "unset",
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
        ClaudeProvider::Gateway,
        ClaudeProvider::Foundry,
    ] {
        let workspace = TestDirectory::new("claude-command");
        write_claude_script(&workspace, SUCCESS_SCRIPT);
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
        assert_version_output(&mut attach).await;
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
fn arguments_leave_permission_policy_to_the_adapter() {
    use crate::native_v2_claude::command::{ClaudeTurnArguments, claude_arguments};

    let arguments = claude_arguments(
        Vec::new(),
        ClaudeTurnArguments {
            model: "provider-owned-model",
            effort: None,
            resume_id: Some("same-session"),
            json_schema: "{}".to_owned(),
        },
    );
    assert!(
        !arguments
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

#[tokio::test]
async fn native_tool_selection_survives_correction_and_reused_session() {
    let workspace = TestDirectory::new("claude-native-config");
    write_claude_script(&workspace, SUCCESS_SCRIPT);
    let mut binding = agent_binding(
        "opaque",
        None,
        SessionScope::NodeInstance,
        &[ANTHROPIC_KEY, "CORRECT_OUTPUT"],
    );
    crate::harnesses::native_config::tests::set(
        &mut binding,
        json!({"args":["--tools","Read,Grep,Glob"]}),
    );
    let runner = runner(
        &workspace,
        ClaudeProvider::Anthropic,
        binding.clone(),
        false,
    )
    .await;
    let handle = runner
        .start(request(
            binding.clone(),
            1,
            &[(ANTHROPIC_KEY, "fixture-key"), ("CORRECT_OUTPUT", "true")],
        ))
        .await
        .assert_value();
    let (_, result) = super::failure::complete_with_durable(handle).await;
    result.assert_value();
    let handle = runner
        .start(request(
            binding.clone(),
            2,
            &[(ANTHROPIC_KEY, "fixture-key"), ("CORRECT_OUTPUT", "false")],
        ))
        .await
        .assert_value();
    super::failure::complete_with_durable(handle)
        .await
        .1
        .assert_value();
    for name in ["initial.args", "resumed.args"] {
        let args = workspace.read(name);
        assert!(args.contains("--tools\nRead,Grep,Glob\n"), "{args}");
        assert!(!args.contains("--dangerously-skip-permissions"), "{args}");
    }
    let NodeRuntimeBinding::Agent {
        harness_config,
        session_scope,
        ..
    } = &mut binding
    else {
        unreachable!()
    };
    *harness_config = None;
    *session_scope = SessionScope::Execution;
    let clean_admitted = admitted_claude(ClaudeProvider::Anthropic, binding.clone(), false).await;
    let clean = runner
        .auxiliary_factory()
        .fork(&clean_admitted)
        .assert_value();
    let handle = clean
        .start(request(
            binding,
            3,
            &[(ANTHROPIC_KEY, "fixture-key"), ("CORRECT_OUTPUT", "false")],
        ))
        .await
        .assert_value();
    super::failure::complete_with_durable(handle)
        .await
        .1
        .assert_value();
    assert!(!workspace.read("initial.args").contains("--tools"));
}

#[tokio::test]
async fn native_tools_compose_with_foundry_on_correction_and_revisit() {
    let workspace = TestDirectory::new("claude-foundry-native-config");
    write_claude_script(&workspace, SUCCESS_SCRIPT);
    let fixture = provider_fixture(ClaudeProvider::Foundry);
    let mut environment = fixture.environment;
    environment.push("CORRECT_OUTPUT");
    let mut binding = agent_binding("opaque", None, SessionScope::NodeInstance, &environment);
    crate::harnesses::native_config::tests::set(
        &mut binding,
        json!({"args":["--tools","Read,Grep,Glob"]}),
    );
    let runner = runner(&workspace, ClaudeProvider::Foundry, binding.clone(), false).await;
    for (execution, correct_output) in [(1, "true"), (2, "false")] {
        let mut values = fixture.values.clone();
        values.push(("CORRECT_OUTPUT", correct_output));
        let handle = runner
            .start(request(binding.clone(), execution, &values))
            .await
            .assert_value();
        super::failure::complete_with_durable(handle)
            .await
            .1
            .assert_value();
    }
    for name in ["initial.args", "resumed.args"] {
        let args = workspace.read(name);
        assert!(args.contains("--tools\nRead,Grep,Glob\n"), "{args}");
        assert!(!args.contains("--dangerously-skip-permissions"), "{args}");
        assert!(!args.contains(fixture.provider_value), "{args}");
    }
    assert_resumed_session(&workspace.read("resumed.args"));
    assert_eq!(workspace.read("homes.txt").lines().count(), 3);
    assert_provider_environment(&workspace, ClaudeProvider::Foundry, fixture.provider_value);
}

#[tokio::test]
async fn native_configuration_rejection_fails_once_with_node_and_cause() {
    let workspace = TestDirectory::new("claude-native-rejection");
    write_claude_script(
        &workspace,
        "printf 'launch\\n' >> launches\nprintf '%s\\n' 'error: invalid --tools native-selection' >&2\nexit 2\n",
    );
    let mut binding = agent_binding("opaque", None, SessionScope::Execution, &[ANTHROPIC_KEY]);
    crate::harnesses::native_config::tests::set(
        &mut binding,
        json!({"args":["--tools","native-selection"]}),
    );
    let runner = runner(
        &workspace,
        ClaudeProvider::Anthropic,
        binding.clone(),
        false,
    )
    .await;
    let handle = runner
        .start(request(binding, 1, &[(ANTHROPIC_KEY, "fixture-key")]))
        .await
        .assert_value();
    let (logs, result) = super::failure::complete_with_durable(handle).await;
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("--tools") && error.contains("claude") && error.contains("node `agent`"),
        "{error}"
    );
    assert_eq!(workspace.read("launches").lines().count(), 1);
    assert!(!logs.logs.join("\n").contains("requesting correction"));
}

#[test]
fn native_settings_and_owned_memory_hook_share_one_inline_layer() {
    use crate::execution::{
        driver::WorkspaceCapability, WorkspaceAccessMode, process::ProcessSessionCommand,
    };
    let mut command = ProcessSessionCommand {
        program: "claude".into(),
        argv: vec![
            "--settings".into(),
            r#"{"permissions":{"deny":["Write"]},"unknownNativeKey":true}"#.into(),
        ],
        environment: BTreeMap::new(),
        workspace: WorkspaceCapability {
            current_dir: ".".into(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: None,
    };
    super::super::memory::install(&mut command, Path::new("/owned/capability")).assert_value();
    assert_eq!(
        command
            .argv
            .iter()
            .filter(|arg| *arg == "--settings")
            .count(),
        1
    );
    let settings: Value = serde_json::from_str(&command.argv[1]).assert_value();
    assert_eq!(settings["permissions"]["deny"], json!(["Write"]));
    assert_eq!(settings["unknownNativeKey"], true);
    assert!(settings["hooks"]["PreToolUse"].is_array());
}

#[test]
fn contained_native_configuration_keeps_host_permissions_and_allows_tool_selection() {
    assert!(
        super::super::native_config::validate_contained(&[
            "--permission-mode".into(),
            "default".into()
        ])
        .is_err()
    );
    assert!(
        super::super::native_config::validate_contained(&[
            "--settings".into(),
            r#"{"sandbox":{"enabled":true}}"#.into()
        ])
        .is_err()
    );
    assert!(
        super::super::native_config::validate_contained(&[
            "--tools".into(),
            "Read,Grep,Glob".into()
        ])
        .is_ok()
    );
}

#[tokio::test]
async fn native_prompt_files_and_session_selectors_are_rejected_before_process_launch() {
    use crate::native_v2_runner::{
        DriverInvocation, NodeDriver, NodeResponseContract, NodeRole, SessionFactory,
    };

    for flag in [
        "--system-prompt-file",
        "--append-system-prompt-file",
        "--from-pr",
        "--teleport",
    ] {
        let workspace = TestDirectory::new("claude-native-ownership-rejection");
        let executable = workspace.write_executable(
            "claude",
            "#!/bin/sh\nprintf 'launch\\n' >> launches\nexit 2\n",
        );
        let mut config = adapter_configuration(
            ClaudeProvider::Anthropic,
            executable.to_str().assert_value(),
            BTreeMap::new(),
        );
        config.workspace = workspace.path().to_owned();
        config.runtime_home = workspace.path().to_owned();
        let adapter = ClaudeAdapter::new_for_test(config).assert_value();
        let mut binding =
            agent_binding("opaque", None, SessionScope::NodeInstance, &[ANTHROPIC_KEY]);
        crate::harnesses::native_config::tests::set(
            &mut binding,
            json!({"args":[flag,"native-value"]}),
        );
        // Exercise the adapter boundary directly, independently of registry admission.
        let request = request(binding, 1, &[(ANTHROPIC_KEY, "fixture-key")]);
        let session = adapter
            .open(&request.invocation, &request.environment)
            .await
            .assert_value();
        let invocation = DriverInvocation {
            dispatch: request.dispatch,
            host_context: None,
            provider_session_slot: request.invocation.reference.node_instance,
            node: request.invocation,
            role: NodeRole::Worker,
            response: NodeResponseContract::Worker {
                output: openengine_cluster_protocol::PayloadType::Null,
            },
            environment: request.environment,
            session: session.clone(),
        };
        let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
        let (result, logs) = tokio::join!(adapter.run(invocation, control), async {
            let mut logs = Vec::new();
            while let Ok(output) = durable.recv_output().await {
                logs.push(output.text);
            }
            logs.join("\n")
        });
        let detail = result.unwrap_err().to_string();
        for expected in [
            "node `agent`",
            "harness `claude`",
            "harnessConfig.args",
            flag,
        ] {
            assert!(detail.contains(expected), "{detail}");
        }
        assert!(!workspace.path().join("launches").exists());
        assert!(!logs.contains("continuing") && !logs.contains("requesting correction"));
        session.close().await;
        assert!(!session.is_live().await);
    }
}
