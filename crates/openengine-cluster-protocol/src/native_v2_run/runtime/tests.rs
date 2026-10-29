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
    let NodeRuntimeBinding::Agent {
        lane: Some(lane), ..
    } = &binding
    else {
        panic!("expected an explicit agent lane");
    };
    assert_eq!(lane.harness_name(), "claude");
    assert_eq!(lane.provider_name(), "anthropic");
    assert_eq!(serde_json::to_string(&binding).unwrap(), wire);
}

#[test]
fn agent_without_a_lane_serializes_without_a_lane_key() {
    let binding = NodeRuntimeBinding::Agent {
        lane: None,
        model: crate::ModelId::new("worker-model").unwrap(),
        effort: None,
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
fn lane_defers_pair_compatibility_but_rejects_unknown_or_missing_fields() {
    for (harness, provider) in [
        ("codex", "anthropic"),
        ("claude", "openai"),
        ("copilot", "openai"),
        ("contributed", "private-provider"),
    ] {
        let binding: NodeRuntimeBinding = serde_json::from_value(serde_json::json!({
            "kind":"agent", "lane":{"harness":harness,"provider":provider}, "model":"opaque-model"
        }))
        .unwrap();
        assert!(matches!(
            binding,
            NodeRuntimeBinding::Agent { lane: Some(_), .. }
        ));
    }
    for (lane, expected) in [
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
