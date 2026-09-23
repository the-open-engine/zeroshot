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

#[test]
fn profile_name_is_a_compact_cli_safe_identifier() {
    for valid in ["default", "software-change", "team_v2.1"] {
        assert!(RunProfileName::new(valid).is_ok());
    }
    for invalid in ["", "-leading", "has space", "has/slash"] {
        assert!(RunProfileName::new(invalid).is_err());
    }
}
