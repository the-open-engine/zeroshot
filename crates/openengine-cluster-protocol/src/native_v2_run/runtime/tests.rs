use super::*;

#[test]
fn pr_feedback_defaults_to_consider_and_runtime_can_select_ignore() {
    let default: NodeRuntimeBinding =
        serde_json::from_value(serde_json::json!({"kind":"git_delivery"})).unwrap();
    assert!(matches!(
        default,
        NodeRuntimeBinding::GitDelivery {
            pull_request_feedback: PullRequestFeedback::Consider,
            ..
        }
    ));
    assert_eq!(
        serde_json::to_value(default).unwrap(),
        serde_json::json!({"kind":"git_delivery"})
    );

    let ignored: NodeRuntimeBinding = serde_json::from_value(serde_json::json!({
        "kind":"git_delivery",
        "pullRequestFeedback":"ignore"
    }))
    .unwrap();
    assert!(matches!(
        ignored,
        NodeRuntimeBinding::GitDelivery {
            pull_request_feedback: PullRequestFeedback::Ignore,
            ..
        }
    ));
}

#[test]
fn agent_lane_round_trips_exactly() {
    let wire = r#"{"kind":"agent","lane":{"harness":"claude","provider":"anthropic"},"model":"review-model"}"#;
    let binding: NodeRuntimeBinding = serde_json::from_str(wire).unwrap();
    assert!(matches!(
        binding,
        NodeRuntimeBinding::Agent {
            lane: Some(crate::RuntimeLane::Claude {
                provider: crate::ClaudeProvider::Anthropic
            }),
            ..
        }
    ));
    assert_eq!(serde_json::to_string(&binding).unwrap(), wire);
}

#[test]
fn agent_without_a_lane_serializes_without_a_lane_key() {
    let binding = NodeRuntimeBinding::Agent {
        lane: None,
        model: crate::ModelId::new("worker-model").unwrap(),
        effort: None,
        context_window: None,
        max_tokens: None,
        session_scope: SessionScope::Execution,
        connections: DeclaredConnections::empty(),
    };
    let wire = r#"{"kind":"agent","model":"worker-model"}"#;
    assert_eq!(serde_json::to_string(&binding).unwrap(), wire);
    assert_eq!(
        serde_json::from_str::<NodeRuntimeBinding>(wire).unwrap(),
        binding
    );
}

#[test]
fn caller_model_limits_are_optional_bounded_and_round_trip() {
    let wire =
        r#"{"kind":"agent","model":"worker-model","contextWindow":1000000,"maxTokens":32000}"#;
    let binding: NodeRuntimeBinding = serde_json::from_str(wire).unwrap();
    assert_eq!(binding.context_window(), Some(1_000_000));
    assert_eq!(binding.max_tokens(), Some(32_000));
    binding.validate_model_limits().unwrap();
    assert_eq!(serde_json::to_string(&binding).unwrap(), wire);

    // Absent limits keep every existing runtime file unchanged and delegate to Pi's defaults.
    let absent: NodeRuntimeBinding =
        serde_json::from_str(r#"{"kind":"agent","model":"worker-model"}"#).unwrap();
    assert_eq!(absent.context_window(), None);
    assert_eq!(absent.max_tokens(), None);
    assert_eq!(
        serde_json::to_string(&absent).unwrap(),
        r#"{"kind":"agent","model":"worker-model"}"#
    );

    // Direct Rust construction must be inspected at admission rather than assumed valid.
    let invalid = NodeRuntimeBinding::Agent {
        lane: None,
        model: crate::ModelId::new("worker-model").unwrap(),
        effort: None,
        context_window: Some(0),
        max_tokens: Some(MAX_MODEL_MAX_TOKENS + 1),
        session_scope: SessionScope::Execution,
        connections: DeclaredConnections::empty(),
    };
    let error = invalid.validate_model_limits().unwrap_err();
    assert!(
        error.to_string().contains("contextWindow") && error.to_string().contains("1..=10000000"),
        "{error}"
    );

    for (field, invalid_value) in [
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
        let mut wire = serde_json::json!({"kind": "agent", "model": "worker-model"});
        wire[field] = invalid_value;
        let error = serde_json::from_value::<NodeRuntimeBinding>(wire).unwrap_err();
        assert!(error.to_string().contains(field), "{field}: {error}");
        assert!(
            error.to_string().contains("1..=10000000"),
            "{field}: {error}"
        );
    }
}

#[test]
fn lane_is_rejected_on_git_delivery() {
    let error = serde_json::from_value::<NodeRuntimeBinding>(serde_json::json!({
        "kind": "git_delivery",
        "lane": {"harness": "claude", "provider": "anthropic"}
    }))
    .unwrap_err();
    assert!(
        error.to_string().contains("unknown field `lane`"),
        "{error}"
    );
}

#[test]
fn lane_rejects_incompatible_pairs_and_unknown_or_missing_fields() {
    for (lane, expected) in [
        (
            serde_json::json!({"harness": "codex", "provider": "anthropic"}),
            "unknown variant `anthropic`",
        ),
        (
            serde_json::json!({"harness": "claude", "provider": "openai"}),
            "unknown variant `openai`",
        ),
        (
            serde_json::json!({"harness": "copilot", "provider": "openai"}),
            "unknown variant `openai`",
        ),
        (
            serde_json::json!({"harness": "claude", "provider": "anthropic", "model": "m"}),
            "unknown field `model`",
        ),
        (
            serde_json::json!({"harness": "claude"}),
            "missing field `provider`",
        ),
    ] {
        let error = serde_json::from_value::<NodeRuntimeBinding>(serde_json::json!({
            "kind": "agent",
            "lane": lane,
            "model": "review-model"
        }))
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{lane}: {error}");
    }
}

#[test]
fn declared_runtime_inputs_enforce_unique_bounded_connection_authority() {
    let token = EnvironmentVariableName::new("TOKEN").unwrap();
    let one = DeclaredEnvironment::new([token.clone()]).unwrap();
    assert!(one.contains(&token));

    let too_many_connections = (0..=MAX_DECLARED_CONNECTIONS)
        .map(|index| {
            (
                ConnectionKey::new(format!("connection-{index}")).unwrap(),
                one.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert!(DeclaredConnections::new(too_many_connections).is_err());

    let duplicate = ConnectionKey::new("duplicate").unwrap();
    assert!(
        DeclaredConnections::new([(duplicate.clone(), one.clone()), (duplicate, one.clone()),])
            .is_err()
    );

    let sixty_four = DeclaredEnvironment::new(
        (0..MAX_DECLARED_ENVIRONMENT_NAMES)
            .map(|index| EnvironmentVariableName::new(format!("NAME_{index}")).unwrap()),
    )
    .unwrap();
    let overflow =
        DeclaredEnvironment::new([EnvironmentVariableName::new("OVERFLOW").unwrap()]).unwrap();
    assert!(
        DeclaredConnections::new([
            (ConnectionKey::new("bulk").unwrap(), sixty_four),
            (ConnectionKey::new("overflow").unwrap(), overflow),
        ])
        .is_err()
    );

    let accepted = DeclaredConnections::single("provider", one).unwrap();
    assert_eq!(accepted.as_map().len(), 1);
    assert!(
        accepted
            .as_map()
            .contains_key(&ConnectionKey::new("provider").unwrap())
    );
}

#[test]
fn caller_model_limits_reject_null_float_and_units_and_skip_other_bindings() {
    // Null behaves exactly like an omitted field, so an explicit JSON null stays caller-friendly.
    let nulls: NodeRuntimeBinding = serde_json::from_value(serde_json::json!({
        "kind": "agent",
        "model": "worker-model",
        "contextWindow": null,
        "maxTokens": null
    }))
    .unwrap();
    assert_eq!(nulls.context_window(), None);
    assert_eq!(nulls.max_tokens(), None);

    // Fractions are rejected by the typed visitor rather than truncated or rounded.
    for field in ["contextWindow", "maxTokens"] {
        let mut wire = serde_json::json!({"kind": "agent", "model": "worker-model"});
        wire[field] = serde_json::json!(1.5);
        let error = serde_json::from_value::<NodeRuntimeBinding>(wire).unwrap_err();
        assert!(error.to_string().contains(field), "{field}: {error}");
    }

    // Both inclusive boundaries are accepted and round-trip through serialization.
    for (context_window, max_tokens) in [
        (MIN_MODEL_CONTEXT_WINDOW, MIN_MODEL_MAX_TOKENS),
        (MAX_MODEL_CONTEXT_WINDOW, MAX_MODEL_MAX_TOKENS),
    ] {
        let binding = NodeRuntimeBinding::Agent {
            lane: None,
            model: crate::ModelId::new("worker-model").unwrap(),
            effort: None,
            context_window: Some(context_window),
            max_tokens: Some(max_tokens),
            session_scope: SessionScope::Execution,
            connections: DeclaredConnections::empty(),
        };
        binding.validate_model_limits().unwrap();
        let round_trip: NodeRuntimeBinding =
            serde_json::from_str(&serde_json::to_string(&binding).unwrap()).unwrap();
        assert_eq!(round_trip, binding);
    }

    // Non-agent bindings have no limits to validate.
    for binding in [
        NodeRuntimeBinding::GitDelivery {
            pull_request_feedback: PullRequestFeedback::Consider,
            connections: DeclaredConnections::empty(),
        },
        NodeRuntimeBinding::CommentDelivery {
            connections: DeclaredConnections::empty(),
        },
    ] {
        binding.validate_model_limits().unwrap();
        assert_eq!(binding.context_window(), None);
        assert_eq!(binding.max_tokens(), None);
    }
}
