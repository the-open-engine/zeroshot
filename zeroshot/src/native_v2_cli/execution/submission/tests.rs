use openengine_cluster_protocol::{
    ConnectionKey, EnvironmentVariableName, GraphProfile, ModelId, NodeRuntimeBinding,
    PullRequestFeedback, RunConnectionRequirements, RunSize, RunTitle, SessionScope,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

fn uniform(harness: UniformHarness, provider: UniformProvider) -> UniformRuntimePlan {
    UniformRuntimePlan {
        harness,
        provider,
        size: RunSize::Small,
        model: ModelId::new("provider-owned-model").assert_value(),
        effort: None,
        session_scope: SessionScope::Execution,
        connections: None,
    }
}

#[test]
fn wave6_cli_contract_uniform_runtime_and_template_bindings_preserve_authority() {
    let graph = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::Merge)
        .assert_value();
    assert_eq!(graph.profile, GraphProfile::Full);
    let runtime = uniform(UniformHarness::Codex, UniformProvider::OpenAi)
        .materialize(&graph, PullRequestFeedback::Ignore)
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
            git_delivery_binding(PullRequestFeedback::Consider).assert_value(),
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
            &NativeV2CliCommand::Run(run.clone()),
            &mut output,
            |_| None,
        )
        .await
        .assert_value(),
        Some(CliOutcome::Completed)
    );
    assert_eq!(output, b"{\"valid\":true}\n");

    for command in [
        NativeV2CliCommand::Run(RunCommand {
            validate_only: false,
            ..run.clone()
        }),
        NativeV2CliCommand::Run(RunCommand {
            selection: RunSelection::Profile(None),
            ..run
        }),
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
