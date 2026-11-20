use openengine_cluster_protocol::{RunSteerParams, RunVector, RunVectorSummary};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

#[test]
fn steer_node_is_optional_and_survives_full_and_summary_projections() {
    let params = json!({"runId":"run", "text":"direction", "mode":"standing"});
    let vector = json!({"id":"steer", "runId":"run", "createdAt":1762905600000_u64,
        "kind":"steer", "text":"direction", "mode":"standing"});
    for node in [None, Some("e2e_tester")] {
        let mut params = params.clone();
        let mut wire = vector.clone();
        if let Some(node) = node {
            params["node"] = json!(node);
            wire["node"] = json!(node);
        }
        let decoded: RunSteerParams = serde_json::from_value(params.clone()).assert_value();
        assert_eq!(serde_json::to_value(decoded).assert_value(), params);
        let decoded: RunVector = serde_json::from_value(wire.clone()).assert_value();
        decoded.validate().assert_value();
        assert_eq!(serde_json::to_value(&decoded).assert_value(), wire);
        let summary = serde_json::to_value(RunVectorSummary::from(decoded)).assert_value();
        assert_eq!(summary.get("node"), wire.get("node"));
        assert!(summary.get("text").is_none());
    }
}

#[test]
fn steer_node_rejects_invalid_identifiers_and_network_combination() {
    let validator = jsonschema::validator_for(
        &serde_json::to_value(schemars::schema_for!(RunSteerParams)).assert_value(),
    )
    .assert_value();
    for node in ["".to_owned(), "bad/name".to_owned(), "a".repeat(129)] {
        let params = json!({"runId":"run", "text":"direction", "node":node});
        assert!(serde_json::from_value::<RunSteerParams>(params.clone()).is_err());
        assert!(!validator.is_valid(&params));
    }
    let vector: RunVector = serde_json::from_value(json!({
        "id":"steer", "runId":"run", "createdAt":1762905600000_u64,
        "kind":"steer", "text":"direction", "node":"worker",
        "network":{"networks":["team"],"origin":{
            "node":"sender","nodeInstance":7,"execution":"execution/7"}}
    }))
    .assert_value();
    assert!(vector.validate().is_err());
}
