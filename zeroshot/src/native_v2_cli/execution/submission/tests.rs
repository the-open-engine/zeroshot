use openengine_cluster_protocol::{
    ConnectionKey, EnvironmentVariableName, GraphProfile, ModelId, NodeRuntimeBinding,
    PullRequestFeedback, RunConnectionRequirements, RunSize, RunTitle, SessionScope,
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
        input,
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
