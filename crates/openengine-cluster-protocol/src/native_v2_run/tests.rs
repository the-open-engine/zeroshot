use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn model_ids_allow_bedrock_application_inference_profile_arns() {
    let profile = format!(
        "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/{}",
        "profile".repeat(20)
    );
    assert!(profile.len() > 128);
    assert!(ModelId::new(profile).is_ok());
}

#[test]
fn model_id_length_matches_json_schema_character_semantics() {
    assert!(ModelId::new("é".repeat(2_048)).is_ok());
    assert!(ModelId::new("é".repeat(2_049)).is_err());
}

#[test]
fn node_connections_reject_ambiguous_or_empty_shapes() {
    let token = EnvironmentVariableName::new("TOKEN").assert_value();
    let environment = DeclaredEnvironment::new([token.clone()]).assert_value();
    let ambiguous = DeclaredConnections::new([
        (
            ConnectionKey::new("left").assert_value(),
            environment.clone(),
        ),
        (ConnectionKey::new("right").assert_value(), environment),
    ]);
    assert!(ambiguous.is_err());
    assert!(DeclaredConnections::single("empty", DeclaredEnvironment::empty()).is_err());
}

#[test]
fn node_connection_wire_shape_is_keyed_and_secret_free() {
    let binding = serde_json::from_value::<NodeRuntimeBinding>(serde_json::json!({
        "kind": "agent",
        "model": "gpt-5.6",
        "connections": {
            "provider": ["API_KEY"]
        }
    }));
    assert!(binding.is_ok());
    let encoded = serde_json::to_value(binding.assert_value()).assert_value();
    assert_eq!(
        encoded.pointer("/connections/provider/0"),
        Some(&serde_json::json!("API_KEY"))
    );
    assert!(encoded.get("env").is_none());
}

fn lane(harness: &str, provider: &str) -> RuntimeLane {
    RuntimeLane::new(harness, provider).assert_value()
}

fn every_lane() -> Vec<RuntimeLane> {
    [
        ("claude", "anthropic"),
        ("claude", "bedrock"),
        ("claude", "gateway"),
        ("claude", "openrouter"),
        ("codex", "bedrock"),
        ("codex", "gateway"),
        ("codex", "openai"),
        ("codex", "openrouter"),
        ("copilot", "github"),
        ("opencode", "custom/provider"),
        ("pi", "anthropic"),
        ("pi", "bedrock"),
        ("pi", "gateway"),
        ("pi", "openai"),
        ("pi", "openrouter"),
    ]
    .into_iter()
    .map(|(harness, provider)| lane(harness, provider))
    .collect()
}

#[test]
fn lane_names_and_display_agree_with_the_serde_wire_names() {
    for lane in every_lane() {
        let wire = serde_json::to_value(&lane).assert_value();
        assert_eq!(
            wire,
            serde_json::json!({
                "harness": lane.harness_name(), "provider": lane.provider_name(),
            })
        );
        assert_eq!(
            lane.to_string(),
            format!("{}/{}", lane.harness_name(), lane.provider_name())
        );
        assert_eq!(
            serde_json::from_value::<RuntimeLane>(wire).assert_value(),
            lane
        );
    }
}

#[test]
fn lanes_order_by_opaque_harness_then_provider_names() {
    let sorted = every_lane()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(sorted.into_iter().collect::<Vec<_>>(), every_lane());
}

#[test]
fn runtime_ids_are_bounded_opaque_values_and_do_not_select_an_adapter() {
    assert!(HarnessId::new("contributed/tool").is_ok());
    assert!(ProviderId::new("private-provider:region").is_ok());
    for invalid in [
        "".to_owned(),
        "bad\nname".to_owned(),
        "bad\0name".to_owned(),
        "x".repeat(129),
        "é".repeat(65),
    ] {
        assert!(HarnessId::new(invalid.clone()).is_err());
        assert!(ProviderId::new(invalid).is_err());
    }
    assert!(HarnessId::new("é".repeat(64)).is_ok());
    let wire = serde_json::json!({
        "harness":"contributed-tool", "provider":"private-provider", "size":"small",
        "nodes":{"worker":{"kind":"agent","model":"opaque-model"}}
    });
    let plan: RuntimePlan = serde_json::from_value(wire.clone()).assert_value();
    assert_eq!(serde_json::to_value(&plan).assert_value(), wire);
    assert_eq!(
        plan.lanes().into_iter().collect::<Vec<_>>(),
        [lane("contributed-tool", "private-provider")]
    );
}

#[test]
fn core_provider_conversion_round_trips_without_runtime_compatibility_policy() {
    for value in [
        CodexProvider::OpenAi,
        CodexProvider::OpenRouter,
        CodexProvider::Gateway,
        CodexProvider::Bedrock,
    ] {
        assert_eq!(value.name().parse::<CodexProvider>().assert_value(), value);
        assert_eq!(ProviderId::from(value).as_str(), value.name());
    }
    for value in [
        ClaudeProvider::Anthropic,
        ClaudeProvider::OpenRouter,
        ClaudeProvider::Gateway,
        ClaudeProvider::Bedrock,
    ] {
        assert_eq!(value.name().parse::<ClaudeProvider>().assert_value(), value);
        assert_eq!(ProviderId::from(value).as_str(), value.name());
    }
    for value in [
        PiProvider::Anthropic,
        PiProvider::OpenAi,
        PiProvider::OpenRouter,
        PiProvider::Gateway,
        PiProvider::Bedrock,
        PiProvider::Github,
    ] {
        assert_eq!(value.name().parse::<PiProvider>().assert_value(), value);
        assert_eq!(ProviderId::from(value).as_str(), value.name());
    }
    for value in [
        CopilotProvider::Github,
        CopilotProvider::OpenAi,
        CopilotProvider::Anthropic,
        CopilotProvider::OpenRouter,
        CopilotProvider::Gateway,
        CopilotProvider::Bedrock,
    ] {
        assert_eq!(
            value.name().parse::<CopilotProvider>().assert_value(),
            value
        );
        assert_eq!(ProviderId::from(value).as_str(), value.name());
    }
    assert!("anthropic".parse::<CodexProvider>().is_err());
    assert!("openai".parse::<ClaudeProvider>().is_err());
    assert!("unsupported".parse::<CopilotProvider>().is_err());
    assert!("unsupported".parse::<PiProvider>().is_err());
}

fn codex_plan(nodes: serde_json::Value) -> RuntimePlan {
    serde_json::from_value(serde_json::json!({
        "harness": "codex", "provider": "openai", "size": "small", "nodes": nodes
    }))
    .assert_value()
}

fn mixed_lane_plan() -> RuntimePlan {
    codex_plan(serde_json::json!({
        "worker": {"kind": "agent", "model": "worker-model"},
        "review": {"kind": "agent", "lane": {"harness": "claude", "provider": "anthropic"}, "model": "review-model"},
        "same": {"kind": "agent", "lane": {"harness": "codex", "provider": "openai"}, "model": "same-model"},
        "deliver": {"kind": "git_delivery"},
        "decision": {"kind": "decision", "api": "systemone", "model": "decision-model",
                     "minProbability": 0.5, "uncertainLabel": "uncertain", "connections": {}}
    }))
}

fn effective_lane_of(plan: &RuntimePlan, node: &str) -> Option<RuntimeLane> {
    let binding = plan
        .nodes()
        .get(&crate::NodeName::new(node).assert_value())
        .assert_value_with("node is bound");
    plan.effective_lane(binding)
}

#[test]
fn plan_resolves_each_binding_to_its_effective_lane() {
    let plan = mixed_lane_plan();
    assert_eq!(plan.lane(), lane("codex", "openai"));
    assert_eq!(
        effective_lane_of(&plan, "worker"),
        Some(lane("codex", "openai"))
    );
    assert_eq!(
        effective_lane_of(&plan, "review"),
        Some(lane("claude", "anthropic"))
    );
    assert_eq!(
        effective_lane_of(&plan, "same"),
        Some(lane("codex", "openai"))
    );
    assert_eq!(effective_lane_of(&plan, "deliver"), None);
    assert_eq!(effective_lane_of(&plan, "decision"), None);
    assert_eq!(
        plan.lanes().into_iter().collect::<Vec<_>>(),
        [lane("claude", "anthropic"), lane("codex", "openai")]
    );
    assert!(plan.has_lane_overrides());
}

#[test]
fn lane_overrides_are_reported_even_when_they_repeat_the_run_level_lane() {
    let plain = codex_plan(serde_json::json!({
        "worker": {"kind": "agent", "model": "worker-model"}, "deliver": {"kind": "git_delivery"}
    }));
    assert!(!plain.has_lane_overrides());
    assert_eq!(
        plain.lanes().into_iter().collect::<Vec<_>>(),
        [lane("codex", "openai")]
    );
    let repeated = codex_plan(serde_json::json!({
        "same": {"kind": "agent", "lane": {"harness": "codex", "provider": "openai"}, "model": "same-model"}
    }));
    assert!(repeated.has_lane_overrides());
    assert_eq!(
        repeated.lanes().into_iter().collect::<Vec<_>>(),
        [lane("codex", "openai")]
    );
    let delivery_only = codex_plan(serde_json::json!({"deliver": {"kind": "git_delivery"}}));
    assert!(!delivery_only.has_lane_overrides());
    assert!(delivery_only.lanes().is_empty());
}

#[test]
fn node_edits_through_nodes_mut_are_visible_through_nodes() {
    let mut plan =
        codex_plan(serde_json::json!({"worker": {"kind": "agent", "model": "worker-model"}}));
    let worker = crate::NodeName::new("worker").assert_value();
    let Some(NodeRuntimeBinding::Agent {
        lane: override_lane,
        ..
    }) = plan.nodes_mut().get_mut(&worker)
    else {
        panic!("worker must be an agent binding");
    };
    *override_lane = Some(lane("claude", "anthropic"));
    assert_eq!(plan.lane(), lane("codex", "openai"));
    assert_eq!(
        effective_lane_of(&plan, "worker"),
        Some(lane("claude", "anthropic"))
    );
    assert!(plan.has_lane_overrides());
    assert_eq!(
        plan.lanes().into_iter().collect::<Vec<_>>(),
        [lane("claude", "anthropic")]
    );
}

#[test]
fn runtime_plan_constructor_preserves_existing_wire_defaults() {
    let plan = RuntimePlan::new(
        lane("opencode", "openai"),
        RunSize::Medium,
        Default::default(),
    );
    assert_eq!(
        serde_json::to_value(plan).assert_value(),
        serde_json::json!({
            "harness":"opencode", "provider":"openai", "size":"medium", "nodes":{}
        })
    );
}

#[test]
fn profile_name_is_a_compact_cli_safe_identifier() {
    for valid in ["default", "software-change", "team_v2.1"] {
        assert!(RunProfileName::new(valid).is_ok());
    }
    for invalid in ["", "-leading", "has space", "has/slash"] {
        assert!(RunProfileName::new(invalid).is_err());
    }
}
