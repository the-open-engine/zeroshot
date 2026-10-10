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

fn every_lane() -> [RuntimeLane; 9] {
    [
        RuntimeLane::Copilot {
            provider: CopilotProvider::Github,
        },
        RuntimeLane::Codex {
            provider: CodexProvider::OpenAi,
        },
        RuntimeLane::Codex {
            provider: CodexProvider::OpenRouter,
        },
        RuntimeLane::Codex {
            provider: CodexProvider::Gateway,
        },
        RuntimeLane::Codex {
            provider: CodexProvider::Bedrock,
        },
        RuntimeLane::Claude {
            provider: ClaudeProvider::Anthropic,
        },
        RuntimeLane::Claude {
            provider: ClaudeProvider::OpenRouter,
        },
        RuntimeLane::Claude {
            provider: ClaudeProvider::Gateway,
        },
        RuntimeLane::Claude {
            provider: ClaudeProvider::Bedrock,
        },
    ]
}

#[test]
fn lane_names_and_display_agree_with_the_serde_wire_names() {
    for lane in every_lane() {
        let wire = serde_json::to_value(lane).assert_value();
        assert_eq!(
            wire,
            serde_json::json!({
                "harness": lane.harness_name(),
                "provider": lane.provider_name(),
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
    assert_eq!(
        RuntimeLane::Claude {
            provider: ClaudeProvider::Anthropic
        }
        .to_string(),
        "claude/anthropic"
    );
}

#[test]
fn lanes_order_by_harness_then_provider_declaration() {
    let sorted = every_lane()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        sorted.into_iter().collect::<Vec<_>>(),
        every_lane().to_vec()
    );
}

const CODEX_OPENAI: RuntimeLane = RuntimeLane::Codex {
    provider: CodexProvider::OpenAi,
};
const CLAUDE_ANTHROPIC: RuntimeLane = RuntimeLane::Claude {
    provider: ClaudeProvider::Anthropic,
};

fn codex_plan(nodes: serde_json::Value) -> RuntimePlan {
    serde_json::from_value(serde_json::json!({
        "harness": "codex",
        "provider": "openai",
        "size": "small",
        "nodes": nodes
    }))
    .assert_value()
}

fn mixed_lane_plan() -> RuntimePlan {
    codex_plan(serde_json::json!({
        "worker": {"kind": "agent", "model": "worker-model"},
        "review": {
            "kind": "agent",
            "lane": {"harness": "claude", "provider": "anthropic"},
            "model": "review-model"
        },
        "same": {
            "kind": "agent",
            "lane": {"harness": "codex", "provider": "openai"},
            "model": "same-model"
        },
        "deliver": {"kind": "git_delivery"}
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
    assert_eq!(plan.lane(), CODEX_OPENAI);
    assert_eq!(effective_lane_of(&plan, "worker"), Some(CODEX_OPENAI));
    assert_eq!(effective_lane_of(&plan, "review"), Some(CLAUDE_ANTHROPIC));
    assert_eq!(effective_lane_of(&plan, "same"), Some(CODEX_OPENAI));
    assert_eq!(effective_lane_of(&plan, "deliver"), None);
    assert_eq!(
        plan.lanes().into_iter().collect::<Vec<_>>(),
        [CODEX_OPENAI, CLAUDE_ANTHROPIC]
    );
    assert!(plan.has_lane_overrides());
}

#[test]
fn lane_overrides_are_reported_even_when_they_repeat_the_run_level_lane() {
    let plain = codex_plan(serde_json::json!({
        "worker": {"kind": "agent", "model": "worker-model"},
        "deliver": {"kind": "git_delivery"}
    }));
    assert!(!plain.has_lane_overrides());
    assert_eq!(
        plain.lanes().into_iter().collect::<Vec<_>>(),
        [CODEX_OPENAI]
    );

    let repeated = codex_plan(serde_json::json!({
        "same": {
            "kind": "agent",
            "lane": {"harness": "codex", "provider": "openai"},
            "model": "same-model"
        }
    }));
    assert!(repeated.has_lane_overrides());
    assert_eq!(
        repeated.lanes().into_iter().collect::<Vec<_>>(),
        [CODEX_OPENAI]
    );

    let delivery_only = codex_plan(serde_json::json!({"deliver": {"kind": "git_delivery"}}));
    assert!(!delivery_only.has_lane_overrides());
    assert!(delivery_only.lanes().is_empty());
}

#[test]
fn node_edits_through_nodes_mut_are_visible_through_nodes() {
    let mut plan = codex_plan(serde_json::json!({
        "worker": {"kind": "agent", "model": "worker-model"}
    }));
    let worker = crate::NodeName::new("worker").assert_value();
    let Some(NodeRuntimeBinding::Agent { lane, .. }) = plan.nodes_mut().get_mut(&worker) else {
        panic!("worker must be an agent binding");
    };
    *lane = Some(CLAUDE_ANTHROPIC);
    assert!(matches!(
        plan.nodes().get(&worker),
        Some(NodeRuntimeBinding::Agent {
            lane: Some(CLAUDE_ANTHROPIC),
            ..
        })
    ));
    assert_eq!(plan.lane(), CODEX_OPENAI);
    assert_eq!(effective_lane_of(&plan, "worker"), Some(CLAUDE_ANTHROPIC));
    assert!(plan.has_lane_overrides());
    assert_eq!(
        plan.lanes().into_iter().collect::<Vec<_>>(),
        [CLAUDE_ANTHROPIC]
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
