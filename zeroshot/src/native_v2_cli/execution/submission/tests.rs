use openengine_cluster_protocol::{
    ConnectionKey, EnvironmentVariableName, GraphProfile, MAX_MODEL_CONTEXT_WINDOW,
    MAX_MODEL_MAX_TOKENS, ModelId, NodeRuntimeBinding, PullRequestFeedback,
    RunConnectionRequirements, RunSize, RunTitle, SessionScope,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

#[test]
fn target_api_keys_cannot_be_collected_by_a_runtime_connection_requirement() {
    let fields = ["ZEROSHOT_API_KEY", "zeroshot_api_key", "OPENAI_API_KEY"]
        .into_iter()
        .map(|name| EnvironmentVariableName::new(name).assert_value())
        .collect();
    let selected = select_connection_requirements(
        BTreeMap::from([(ConnectionKey::new("provider").assert_value(), fields)]),
        |_| Some(OsString::from("must-stay-private")),
    )
    .assert_value();
    let values = selected.values().next().assert_value().as_map();
    assert_eq!(values.len(), 1);
    assert!(values.contains_key(&EnvironmentVariableName::new("OPENAI_API_KEY").assert_value()));
}

fn uniform(harness: UniformHarness, provider: UniformProvider) -> UniformRuntimePlan {
    UniformRuntimePlan {
        harness,
        provider,
        size: RunSize::Small,
        model: ModelId::new("provider-owned-model").assert_value(),
        effort: None,
        context_window: None,
        max_tokens: None,
        session_scope: None,
        connections: None,
        active_memory: Default::default(),
        global_agents: Default::default(),
    }
}

#[test]
fn uniform_runtime_preserves_authored_memory_controls_for_every_harness() {
    let graph = BuiltinGraphTemplate::SingleWorker
        .materialize(TemplateDelivery::None)
        .assert_value();
    for (harness, provider) in [
        ("codex", "openai"),
        ("claude", "anthropic"),
        ("copilot", "github"),
        ("pi", "bedrock"),
    ] {
        let authored = serde_json::json!({
            "harness": harness,
            "provider": provider,
            "model": "provider-owned-model",
            "activeMemory": {"repository": false, "graph": true, "budgetBytes": 512},
            "globalAgents": {"postmortem": {"enabled": false, "sourceNode": "worker", "timeoutMs": 1000}}
        });
        let plan: UniformRuntimePlan = serde_json::from_value(authored.clone()).assert_value();
        let runtime = plan
            .materialize(&graph, PullRequestFeedback::Consider, None)
            .assert_value();
        let serialized = serde_json::to_value(runtime).assert_value();
        assert_eq!(
            serialized["activeMemory"], authored["activeMemory"],
            "{harness}"
        );
        assert_eq!(
            serialized["globalAgents"], authored["globalAgents"],
            "{harness}"
        );

        let defaults: UniformRuntimePlan = serde_json::from_value(serde_json::json!({
            "harness": harness, "provider": provider, "model": "provider-owned-model"
        }))
        .assert_value();
        assert_eq!(defaults.active_memory, ActiveMemoryConfig::default());
        assert_eq!(defaults.global_agents, GlobalAgentsConfig::default());
    }
}

#[test]
fn wave6_cli_contract_uniform_runtime_and_template_bindings_preserve_authority() {
    let graph = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::Merge)
        .assert_value();
    assert_eq!(graph.profile, GraphProfile::Full);
    let runtime = uniform(UniformHarness::Codex, UniformProvider::OpenAi)
        .materialize(
            &graph,
            PullRequestFeedback::Ignore,
            Some(BuiltinGraphTemplate::SoftwareChange),
        )
        .assert_value();
    let RuntimePlan::Codex { nodes, .. } = &runtime else {
        panic!("expected Codex runtime")
    };
    assert!(nodes.values().any(|binding| matches!(
        binding,
        NodeRuntimeBinding::GitDelivery {
            pull_request_feedback: PullRequestFeedback::Ignore,
            ..
        }
    )));
    assert!(
        nodes
            .values()
            .any(|binding| matches!(binding, NodeRuntimeBinding::Agent { .. }))
    );

    for (harness, provider, accepted) in [
        (UniformHarness::Copilot, UniformProvider::Github, true),
        (UniformHarness::Copilot, UniformProvider::OpenAi, false),
        (UniformHarness::Codex, UniformProvider::OpenAi, true),
        (UniformHarness::Codex, UniformProvider::OpenRouter, true),
        (UniformHarness::Codex, UniformProvider::Gateway, true),
        (UniformHarness::Codex, UniformProvider::Bedrock, true),
        (UniformHarness::Codex, UniformProvider::Anthropic, false),
        (UniformHarness::Claude, UniformProvider::Anthropic, true),
        (UniformHarness::Claude, UniformProvider::OpenRouter, true),
        (UniformHarness::Claude, UniformProvider::Gateway, true),
        (UniformHarness::Claude, UniformProvider::Bedrock, true),
        (UniformHarness::Claude, UniformProvider::OpenAi, false),
        (UniformHarness::Pi, UniformProvider::Anthropic, true),
        (UniformHarness::Pi, UniformProvider::OpenAi, true),
        (UniformHarness::Pi, UniformProvider::OpenRouter, true),
        (UniformHarness::Pi, UniformProvider::Gateway, true),
        (UniformHarness::Pi, UniformProvider::Bedrock, true),
        (UniformHarness::Pi, UniformProvider::Github, false),
    ] {
        assert_eq!(
            uniform(harness, provider)
                .into_runtime_plan(harness, BTreeMap::new())
                .is_ok(),
            accepted,
            "unexpected compatibility for {harness:?}/{provider:?}"
        );
    }

    let delivery_name = nodes
        .iter()
        .find_map(|(name, binding)| {
            matches!(binding, NodeRuntimeBinding::GitDelivery { .. }).then(|| name.clone())
        })
        .assert_value();
    let mut invalid = runtime.clone();
    let invalid_nodes = match &mut invalid {
        RuntimePlan::Codex { nodes, .. } => nodes,
        _ => unreachable!(),
    };
    invalid_nodes.insert(
        delivery_name.clone(),
        NodeRuntimeBinding::Agent {
            lane: None,
            model: ModelId::new("wrong-kind").assert_value(),
            effort: None,
            context_window: None,
            max_tokens: None,
            session_scope: SessionScope::Execution,
            connections: DeclaredConnections::empty(),
        },
    );
    assert!(
        insert_template_binding(
            &mut invalid,
            delivery_name,
            git_delivery_binding(
                PullRequestFeedback::Consider,
                crate::native_v2_delivery::DeliveryMode::PullRequestV2
            )
            .assert_value(),
            false,
        )
        .assert_error()
        .to_string()
        .contains("template-owned node")
    );

    let no_delivery = RunGraph::Template {
        template: BuiltinGraphTemplate::SoftwareChange,
        delivery: TemplateDelivery::None,
        ignore_pr_feedback: false,
    };
    assert_eq!(
        apply_template_runtime(&no_delivery, runtime.clone(), PullRequestFeedback::Consider)
            .assert_value(),
        runtime
    );
}

#[test]
fn uniform_runtime_threads_model_limits_into_each_agent_binding() {
    let graph = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::Merge)
        .assert_value();
    let authored = serde_json::json!({
        "harness": "pi",
        "provider": "gateway",
        "model": "provider-owned-model",
        "contextWindow": 1_000_000,
        "maxTokens": 32_000
    });
    let plan: UniformRuntimePlan = serde_json::from_value(authored).assert_value();
    let runtime = plan
        .materialize(
            &graph,
            PullRequestFeedback::Consider,
            Some(BuiltinGraphTemplate::SoftwareChange),
        )
        .assert_value();
    let mut agents = 0;
    for (name, binding) in runtime.nodes() {
        if let NodeRuntimeBinding::Agent {
            context_window,
            max_tokens,
            ..
        } = binding
        {
            agents += 1;
            assert_eq!(*context_window, Some(1_000_000), "{name}");
            assert_eq!(*max_tokens, Some(32_000), "{name}");
        }
    }
    assert!(agents > 0);

    let defaults: UniformRuntimePlan = serde_json::from_value(serde_json::json!({
        "harness": "pi", "provider": "gateway", "model": "provider-owned-model"
    }))
    .assert_value();
    assert_eq!(defaults.context_window, None);
    assert_eq!(defaults.max_tokens, None);
    let defaults = defaults
        .materialize(
            &graph,
            PullRequestFeedback::Consider,
            Some(BuiltinGraphTemplate::SoftwareChange),
        )
        .assert_value();
    assert!(defaults.nodes().values().all(|binding| matches!(
        binding,
        NodeRuntimeBinding::GitDelivery { .. }
    ) || binding.context_window().is_none()
        && binding.max_tokens().is_none()));

    for (field, value) in [
        ("contextWindow", serde_json::json!(0)),
        (
            "contextWindow",
            serde_json::json!(MAX_MODEL_CONTEXT_WINDOW + 1),
        ),
        ("contextWindow", serde_json::json!(-1)),
        ("maxTokens", serde_json::json!(0)),
        ("maxTokens", serde_json::json!(MAX_MODEL_MAX_TOKENS + 1)),
        ("maxTokens", serde_json::json!(-1)),
    ] {
        let mut invalid = serde_json::json!({
            "harness": "pi", "provider": "gateway", "model": "provider-owned-model"
        });
        invalid[field] = value;
        let error = serde_json::from_value::<UniformRuntimePlan>(invalid)
            .expect_err("out-of-range model limit must be rejected");
        assert!(
            error.to_string().contains(field) && error.to_string().contains("1..=10000000"),
            "{field}: {error}"
        );
    }
}

#[test]
fn software_change_session_defaults_preserve_explicit_runtime_choices() {
    let graph = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::None)
        .assert_value();
    for (template, scope, worker_scope, reviewer_scope) in [
        (
            Some(BuiltinGraphTemplate::SoftwareChange),
            None,
            SessionScope::NodeInstance,
            SessionScope::Execution,
        ),
        (None, None, SessionScope::Execution, SessionScope::Execution),
        (
            Some(BuiltinGraphTemplate::SoftwareChange),
            Some(SessionScope::Execution),
            SessionScope::Execution,
            SessionScope::Execution,
        ),
        (
            Some(BuiltinGraphTemplate::SoftwareChange),
            Some(SessionScope::NodeInstance),
            SessionScope::NodeInstance,
            SessionScope::NodeInstance,
        ),
    ] {
        let mut uniform = uniform(UniformHarness::Codex, UniformProvider::OpenAi);
        uniform.session_scope = scope;
        let runtime = uniform
            .materialize(&graph, PullRequestFeedback::Consider, template)
            .assert_value();
        for (name, expected) in [
            ("worker", worker_scope),
            ("acceptance", reviewer_scope),
            ("code", reviewer_scope),
        ] {
            assert!(
                matches!(runtime.nodes().get(&NodeName::new(name).assert_value()),
                Some(NodeRuntimeBinding::Agent { session_scope, .. }) if *session_scope == expected)
            );
        }
    }
}

#[test]
fn auto_research_session_defaults_preserve_explicit_runtime_choices() {
    let template = BuiltinGraphTemplate::AutoResearch;
    let graph = template.materialize(TemplateDelivery::None).assert_value();
    for override_scope in [
        None,
        Some(SessionScope::Execution),
        Some(SessionScope::NodeInstance),
    ] {
        let mut uniform = uniform(UniformHarness::Codex, UniformProvider::OpenAi);
        uniform.session_scope = override_scope;
        let runtime = uniform
            .materialize(&graph, PullRequestFeedback::Consider, Some(template))
            .assert_value();
        for role in [
            "explorer",
            "synthesizer",
            "challenger",
            "researcher",
            "reviewer",
        ] {
            let expected = override_scope.unwrap_or_else(|| template.default_session_scope(role));
            assert!(
                matches!(runtime.nodes().get(&NodeName::new(role).assert_value()),
                Some(NodeRuntimeBinding::Agent { session_scope, .. }) if *session_scope == expected)
            );
        }
    }
}

#[test]
fn wave6_cli_contract_connection_selection_and_json_errors_keep_precedence() {
    let field = EnvironmentVariableName::new("OPENAI_API_KEY").assert_value();
    let requirements: RunConnectionRequirements = BTreeMap::from([(
        ConnectionKey::new("openai").assert_value(),
        vec![field.clone()],
    )]);
    assert!(
        select_connection_requirements(requirements.clone(), |_| None)
            .assert_value()
            .is_empty()
    );
    let selected = select_connection_requirements(requirements.clone(), |name| {
        (name == "OPENAI_API_KEY").then(|| std::ffi::OsString::from("secret"))
    })
    .assert_value();
    assert_eq!(selected.len(), 1);

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        assert!(matches!(
            select_connection_requirements(requirements, |_| Some(
                std::ffi::OsString::from_vec(vec![0xff])
            )),
            Err(NativeV2CliError::Environment(name)) if name == field
        ));
    }

    let directory = tempfile::tempdir().assert_value();
    let missing = directory.path().join("missing.json");
    assert!(matches!(
        read_json::<serde_json::Value>("contract", &missing),
        Err(NativeV2CliError::Read {
            kind: "contract",
            ..
        })
    ));
    let invalid = directory.path().join("invalid.json");
    std::fs::write(&invalid, "not-json").assert_value();
    assert!(matches!(
        read_json::<serde_json::Value>("contract", &invalid),
        Err(NativeV2CliError::Json {
            kind: "contract",
            ..
        })
    ));
}

#[tokio::test]
async fn wave8_cli_contract_validate_only_preflight_materializes_without_backend_contact() {
    let directory = tempfile::tempdir().assert_value();
    let input = directory.path().join("input.json");
    let runtime = directory.path().join("runtime.json");
    std::fs::write(
        &input,
        serde_json::json!({"task":"validate this run"}).to_string(),
    )
    .assert_value();
    std::fs::write(
        &runtime,
        serde_json::json!({
            "harness":"codex",
            "provider":"openai",
            "size":"small",
            "nodes":{"worker":{"kind":"agent","model":"provider-model"}}
        })
        .to_string(),
    )
    .assert_value();
    let run = RunCommand {
        cwd: None,
        source_connection: None,
        source_forge: None,

        environment: None,
        target: None,
        title: RunTitle::new("Validate only").assert_value(),
        selection: RunSelection::Inline {
            graph: RunGraph::Template {
                template: BuiltinGraphTemplate::SingleWorker,
                delivery: TemplateDelivery::None,
                ignore_pr_feedback: false,
            },
            runtime: RunRuntime::Exact(runtime),
        },
        input: input.into(),
        repository: None,
        branch: None,
        revision: None,
        detach: false,
        validate_only: true,
        submission_key: None,
    };
    let mut output = Vec::new();
    assert_eq!(
        try_execute_native_v2_preflight_with_environment(
            &NativeV2CliCommand::Run(Box::new(run.clone())),
            &mut output,
            |_| None,
        )
        .await
        .assert_value(),
        Some(CliOutcome::Completed)
    );
    assert_eq!(output, b"{\"valid\":true}\n");

    for command in [
        NativeV2CliCommand::Run(Box::new(RunCommand {
            validate_only: false,
            ..run.clone()
        })),
        NativeV2CliCommand::Run(Box::new(RunCommand {
            selection: RunSelection::Profile(None),
            ..run
        })),
        NativeV2CliCommand::TemplateList,
    ] {
        assert!(
            try_execute_native_v2_preflight_with_environment(&command, &mut Vec::new(), |_| None)
                .await
                .assert_value()
                .is_none()
        );
    }
}

fn named_validation_command(
    directory: &std::path::Path,
    template: BuiltinGraphTemplate,
    extra: &[&str],
) -> RunCommand {
    let input = directory.join("validation-input.json");
    let runtime = directory.join("validation-runtime.json");
    std::fs::write(&input, template_validation_input(template).to_string()).assert_value();
    std::fs::write(
        &runtime,
        serde_json::json!({
            "harness":"codex", "provider":"openai", "model":"validation-model",
            "connections":{"openai":["OPENAI_API_KEY"]}
        })
        .to_string(),
    )
    .assert_value();
    let mut args = [
        "run",
        "--title",
        "Validate named delivery",
        "--target",
        "unconfigured-target",
        "--template",
        template.name(),
        "--validate-only",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    args.extend([
        OsString::from("--cwd"),
        directory.join("missing-workspace").into_os_string(),
        OsString::from("--input"),
        input.into_os_string(),
        OsString::from("--uniform-runtime-config"),
        runtime.into_os_string(),
    ]);
    args.extend(extra.iter().map(OsString::from));
    let NativeV2CliCommand::Run(run) =
        crate::native_v2_cli::parse_native_v2_args(args).assert_value()
    else {
        panic!("run command")
    };
    *run
}

fn template_validation_input(template: BuiltinGraphTemplate) -> serde_json::Value {
    if !matches!(
        template,
        BuiltinGraphTemplate::IssueReview | BuiltinGraphTemplate::PrReview
    ) {
        return serde_json::json!({"task":"Validate without a source or credential"});
    }
    let mut input = serde_json::json!({
        "url":"https://ghe.example.test/team/project/issues/17",
        "context":{"title":"Proposal","body":"Review prepared context","reviewScope":{}}
    });
    if template == BuiltinGraphTemplate::PrReview {
        input["url"] = serde_json::json!("https://ghe.example.test/team/project/pull/17");
        input["context"]["reviewScope"] = serde_json::json!({
            "baseRepository":"team/project","baseBranch":"main","baseRevision":"a".repeat(40),
            "headRepository":"team/project","headBranch":"feature","headRevision":"b".repeat(40)
        });
        input["context"]["linkedIssues"] = serde_json::json!([]);
    }
    input
}

async fn assert_source_neutral_named_validation(run: RunCommand) {
    let mut output = Vec::new();
    let command = NativeV2CliCommand::Run(Box::new(run));
    assert_eq!(
        try_execute_native_v2_preflight_with_environment(&command, &mut output, |name| {
            panic!("validate-only must not read credential field {name}")
        })
        .await
        .assert_value(),
        Some(CliOutcome::Completed)
    );
    assert_eq!(output, b"{\"valid\":true}\n");
}

#[tokio::test]
async fn named_pr_and_ship_validate_only_ignore_explicit_source_and_credentials() {
    let directory = tempfile::tempdir().assert_value();
    for delivery in ["--pr", "--ship"] {
        let revision = "a".repeat(40);
        let run = named_validation_command(
            directory.path(),
            BuiltinGraphTemplate::SoftwareChange,
            &[
                delivery,
                "--repository",
                "https://ghe.example.test/team/project.git",
                "--branch",
                "main",
                "--revision",
                &revision,
                "--source-provider",
                "source.github",
                "--source-base-url",
                "https://ghe.example.test",
            ],
        );
        assert_source_neutral_named_validation(run).await;
    }
}

#[tokio::test]
async fn named_validate_only_accepts_every_supported_template_without_a_source() {
    let directory = tempfile::tempdir().assert_value();
    for &template in BuiltinGraphTemplate::all() {
        for delivery in [
            TemplateDelivery::None,
            TemplateDelivery::Push,
            TemplateDelivery::PullRequest,
            TemplateDelivery::Merge,
            TemplateDelivery::Comment,
        ] {
            if template.materialize(delivery).is_err() {
                continue;
            }
            let run = named_validation_command(
                directory.path(),
                template,
                &["--delivery", delivery.name()],
            );
            assert!(run.repository.is_none() && run.branch.is_none() && run.revision.is_none());
            assert_source_neutral_named_validation(run).await;
        }
    }
}

#[test]
fn github_tokens_reject_empty_oversized_and_control_bearing_values() {
    assert_eq!(
        validate_github_token("ghp_token".to_owned()).assert_value(),
        "ghp_token"
    );
    for rejected in [
        String::new(),
        "x".repeat(4_097),
        "ghp_bad\0token".to_owned(),
    ] {
        assert!(matches!(
            validate_github_token(rejected),
            Err(NativeV2CliError::GitHubToken)
        ));
    }
}

#[test]
fn run_environment_inputs_distinguish_empty_missing_and_malformed_files() {
    assert!(matches!(read_run_environment(&None), Ok(None)));
    assert!(matches!(
        read_run_environment(&Some(crate::native_v2_cli::RunEnvironmentInput::Empty)),
        Ok(Some(environment)) if environment == Default::default()
    ));

    let directory = tempfile::tempdir().assert_value();
    let missing = directory.path().join("missing.json");
    assert!(matches!(
        read_run_environment(&Some(crate::native_v2_cli::RunEnvironmentInput::File(
            missing
        ))),
        Err(NativeV2CliError::Read {
            kind: "run environment",
            ..
        })
    ));
    let malformed = directory.path().join("malformed.json");
    std::fs::write(&malformed, b"{not json").assert_value();
    assert!(matches!(
        read_run_environment(&Some(crate::native_v2_cli::RunEnvironmentInput::File(
            malformed
        ))),
        Err(NativeV2CliError::Json {
            kind: "run environment",
            ..
        })
    ));
}

#[test]
fn connection_selection_keeps_present_utf8_fields_and_rejects_non_utf8_values() {
    let selected = select_connection_requirements(
        BTreeMap::from([(
            ConnectionKey::new("provider").assert_value(),
            [
                EnvironmentVariableName::new("ABSENT").assert_value(),
                EnvironmentVariableName::new("OPENAI_API_KEY").assert_value(),
            ]
            .into_iter()
            .collect(),
        )]),
        |name| (name == "OPENAI_API_KEY").then(|| OsString::from("secret")),
    )
    .assert_value();
    assert_eq!(selected.len(), 1);
    assert_eq!(
        selected
            .values()
            .next()
            .assert_value()
            .as_map()
            .get(&EnvironmentVariableName::new("OPENAI_API_KEY").assert_value()),
        Some(&"secret".to_owned())
    );

    #[cfg(unix)]
    {
        let error = select_connection_requirements(
            BTreeMap::from([(
                ConnectionKey::new("provider").assert_value(),
                [EnvironmentVariableName::new("OPENAI_API_KEY").assert_value()]
                    .into_iter()
                    .collect(),
            )]),
            |_| {
                use std::os::unix::ffi::OsStringExt as _;
                Some(OsString::from_vec(vec![b'n', 0xff]))
            },
        )
        .expect_err("non-UTF-8 environment values must not be submitted");
        assert!(
            matches!(error, NativeV2CliError::Environment(name) if name.as_str() == "OPENAI_API_KEY")
        );
    }
}

fn intent_command(selection: RunSelection) -> RunCommand {
    RunCommand {
        environment: None,
        target: None,
        cwd: None,
        source_connection: None,
        source_forge: None,
        title: RunTitle::new("Intent validation").assert_value(),
        input: "input.json".into(),
        selection,
        repository: None,
        branch: None,
        revision: None,
        detach: false,
        validate_only: false,
        submission_key: None,
    }
}

fn resolved_profile(profile: GraphProfile) -> ResolvedRunProfile {
    let mut graph = BuiltinGraphTemplate::SingleWorker
        .materialize(TemplateDelivery::None)
        .assert_value();
    graph.profile = profile;
    let runtime = uniform(UniformHarness::Codex, UniformProvider::OpenAi)
        .materialize(&graph, PullRequestFeedback::Consider, None)
        .assert_value();
    ResolvedRunProfile {
        graph,
        runtime,
        remote_selector: None,
    }
}

#[test]
fn prepare_intent_rejects_non_full_profiles_and_mismatched_initial_input() {
    let command = intent_command(RunSelection::Profile(None));
    let profile = resolved_profile(GraphProfile::SingleWorker);
    let error = prepare_intent(&command, profile, serde_json::json!({"task":"x"}), None)
        .expect_err("non-full graph profile must be rejected");
    assert!(
        matches!(error, NativeV2CliError::Usage(message) if message.contains("openengine.graph.full/v1"))
    );

    let profile = resolved_profile(GraphProfile::Full);
    let error = prepare_intent(&command, profile, serde_json::json!("not an object"), None)
        .expect_err("initial input must match the authored schema");
    assert!(matches!(error, NativeV2CliError::InitialInput(_)));
}

#[test]
fn prepare_intent_preserves_an_explicit_submission_key() {
    let mut command = intent_command(RunSelection::Profile(None));
    command.submission_key = Some(IdempotencyKey::new("cli-retry-key").assert_value());
    let intent = prepare_intent(
        &command,
        resolved_profile(GraphProfile::Full),
        serde_json::json!({"task":"x"}),
        Some(Default::default()),
    )
    .assert_value();
    assert_eq!(intent.submission_key, command.submission_key.assert_value());
    assert_eq!(intent.initial_input, serde_json::json!({"task":"x"}));
}

#[test]
fn template_binding_insertion_owns_comment_and_git_delivery_nodes() {
    let mut runtime = RuntimePlan::Pi {
        provider: PiProvider::Anthropic,
        size: RunSize::Small,
        active_memory: Default::default(),
        global_agents: Default::default(),
        nodes: BTreeMap::new(),
    };
    let delivery = openengine_cluster_protocol::NodeName::new("delivery").assert_value();
    let comment = openengine_cluster_protocol::NodeName::new("comment").assert_value();
    let push = git_delivery_binding(
        PullRequestFeedback::Consider,
        crate::native_v2_delivery::DeliveryMode::Push,
    )
    .assert_value();
    let merge = git_delivery_binding(
        PullRequestFeedback::Consider,
        crate::native_v2_delivery::DeliveryMode::MergeV3,
    )
    .assert_value();
    let comment_binding = NodeRuntimeBinding::CommentDelivery {
        connections: DeclaredConnections::default(),
    };

    insert_template_binding(&mut runtime, delivery.clone(), push, false).assert_value();
    let encoded =
        serde_json::to_value(runtime.nodes().get(&delivery).assert_value()).assert_value();
    assert!(
        encoded.get("pullRequestFeedback").is_none(),
        "consider is the implicit pull-request feedback default"
    );
    assert!(
        encoded.get("connections").is_none(),
        "push delivery needs no GitHub connection"
    );

    insert_template_binding(&mut runtime, delivery.clone(), merge, true).assert_value();
    assert_eq!(
        serde_json::to_value(runtime.nodes().get(&delivery).assert_value())
            .assert_value()
            .get("pullRequestFeedback"),
        Some(&serde_json::json!("ignore"))
    );
    assert!(matches!(
        runtime.nodes().get(&delivery).assert_value(),
        NodeRuntimeBinding::GitDelivery {
            pull_request_feedback: PullRequestFeedback::Ignore,
            ..
        }
    ));

    insert_template_binding(
        &mut runtime,
        comment.clone(),
        comment_binding.clone(),
        false,
    )
    .assert_value();
    insert_template_binding(
        &mut runtime,
        comment.clone(),
        comment_binding.clone(),
        false,
    )
    .assert_value();
    assert!(
        insert_template_binding(&mut runtime, delivery.clone(), comment_binding, false)
            .assert_error()
            .to_string()
            .contains("comment_delivery")
    );
    assert!(
        insert_template_binding(
            &mut runtime,
            comment,
            git_delivery_binding(
                PullRequestFeedback::Consider,
                crate::native_v2_delivery::DeliveryMode::MergeV3,
            )
            .assert_value(),
            false,
        )
        .assert_error()
        .to_string()
        .contains("git_delivery")
    );
}

fn named_comment_request(url: &str) -> (RunCommand, PreparedRunRequest) {
    let graph = BuiltinGraphTemplate::IssueReview
        .materialize(TemplateDelivery::Comment)
        .assert_value();
    let runtime = uniform(UniformHarness::Codex, UniformProvider::OpenAi)
        .materialize(&graph, PullRequestFeedback::Consider, None)
        .assert_value();
    let NativeV2CliCommand::Run(run) = crate::native_v2_cli::parse_native_v2_args(
        [
            "run",
            "--title",
            "Comment",
            "--target",
            "prod",
            "--input",
            "input.json",
            "--profile",
            "review",
        ]
        .into_iter()
        .map(OsString::from),
    )
    .assert_value() else {
        panic!("run command")
    };
    let request = PreparedRunRequest {
        run_id: openengine_cluster_protocol::RunId::new("source-less-comment"),
        intent: TargetRunIntent {
            title: run.title.clone(),
            graph,
            initial_input: serde_json::json!({
                "url":url,
                "context":{"title":"Issue","body":"Review this","reviewScope":{}}
            }),
            runtime,
            environment: None,
            branch: None,
            submission_key: IdempotencyKey::new("source-less-comment").assert_value(),
        },
        connections: RunConnectionValues::new(),
        github_token: None,
        git_credentials: None,
        cwd: None,
        source_connection: None,
        source: None,
        profile: None,
    };
    (*run, request)
}

const NAMED_COMMENT_CASES: [(&str, &str, &str); 2] = [
    (
        "https://ghe.example.test/team/project/issues/17",
        "github",
        "GH_TOKEN",
    ),
    (
        "https://gitlab.example.test/team/project/-/issues/17",
        "gitlab",
        "GITLAB_TOKEN",
    ),
];

#[test]
fn named_source_less_comments_materialize_the_canonical_connection_used_by_resume() {
    for (url, key, field) in NAMED_COMMENT_CASES {
        let (run, mut request) = named_comment_request(url);
        let queried = std::cell::RefCell::new(Vec::new());
        let available = |name: &str| {
            queried.borrow_mut().push(name.to_owned());
            Some(OsString::from(if name == field {
                "canonical-token"
            } else {
                "unused-alias"
            }))
        };
        materialize_named_delivery_access(&run, &mut request, &available).assert_value();
        assert_eq!(queried.borrow().as_slice(), &[field]);
        let key = ConnectionKey::new(key).assert_value();
        let field = EnvironmentVariableName::new(field).assert_value();
        let required = request.intent.runtime.connection_requirements();
        assert_eq!(
            required[&key],
            std::collections::BTreeSet::from([field.clone()])
        );
        assert_eq!(
            request.connections[&key].as_map()[&field],
            "canonical-token"
        );
        let resumed = select_connection_requirements(
            required
                .into_iter()
                .map(|(key, fields)| (key, fields.into_iter().collect()))
                .collect(),
            available,
        )
        .assert_value();
        assert_eq!(resumed, request.connections);
        assert!(request.source.is_none());
    }
}

#[test]
fn named_delivery_does_not_upload_local_cli_token_aliases() {
    for (url, _, field) in NAMED_COMMENT_CASES {
        let (run, mut request) = named_comment_request(url);
        let queried = std::cell::RefCell::new(Vec::new());
        materialize_named_delivery_access(&run, &mut request, &|name: &str| {
            queried.borrow_mut().push(name.to_owned());
            [
                "GH_ENTERPRISE_TOKEN",
                "GITHUB_ENTERPRISE_TOKEN",
                "GITLAB_ACCESS_TOKEN",
                "OAUTH_TOKEN",
            ]
            .contains(&name)
            .then(|| OsString::from("local-only-token"))
        })
        .assert_value();
        assert_eq!(queried.borrow().as_slice(), &[field]);
        assert!(request.connections.is_empty());
    }
}

#[test]
fn named_delivery_preserves_explicit_values_and_does_not_repair_partial_connections() {
    let (run, mut request) = named_comment_request(NAMED_COMMENT_CASES[0].0);
    let key = ConnectionKey::new("company-ghe").assert_value();
    let binding = request
        .intent
        .runtime
        .nodes_mut()
        .values_mut()
        .find(|binding| matches!(binding, NodeRuntimeBinding::CommentDelivery { .. }))
        .assert_value();
    *binding = serde_json::from_value(serde_json::json!({
        "kind":"comment_delivery","connections":{key.as_str():["GH_TOKEN","OTHER"]}
    }))
    .assert_value();
    let provided = StaticConnectionValues::new(BTreeMap::from([(
        EnvironmentVariableName::new("GH_TOKEN").assert_value(),
        "explicit-token".to_owned(),
    )]))
    .assert_value();
    request.connections.insert(key.clone(), provided.clone());
    materialize_named_delivery_access(&run, &mut request, &|name| {
        panic!("explicit connection unexpectedly read {name}")
    })
    .assert_value();
    assert_eq!(request.connections[&key], provided);
    let submission = crate::native_v2_local::local_submission(&request.intent, None);
    assert!(
        crate::native_v2_supervisor::RunEnvironment::exact_for_submission(
            &submission,
            request.connections,
        )
        .is_err()
    );
}
