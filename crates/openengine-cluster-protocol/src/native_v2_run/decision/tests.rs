use super::*;
use crate::{NodeRuntimeBinding, RuntimePlan};
use serde_json::json;

fn binding() -> serde_json::Value {
    json!({"kind":"decision", "api":"systemone", "model":"caller/model", "minProbability":0.85,
        "uncertainLabel":"uncertain", "connections":{"decisions":["DECISION_API_KEY","DECISION_ENDPOINT"]}})
}

#[test]
fn decision_settings_round_trip_without_an_agent_lane() {
    let wire = binding();
    let binding: NodeRuntimeBinding = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&binding).unwrap(), wire);
    let runtime: RuntimePlan = serde_json::from_value(
        json!({"harness":"codex","provider":"openai","size":"small","nodes":{"triage":wire}}),
    )
    .unwrap();
    assert!(runtime.lanes().is_empty());
    assert_eq!(runtime.connection_requirements().len(), 1);
}

#[test]
fn invalid_probabilities_and_agent_only_fields_are_rejected() {
    for value in [json!(-0.01), json!(1.01), json!("0.85"), json!(null)] {
        let mut wire = binding();
        wire["minProbability"] = value;
        assert!(serde_json::from_value::<NodeRuntimeBinding>(wire).is_err());
    }
    for field in ["effort", "sessionScope", "lane", "endpoint", "apiKey"] {
        let mut wire = binding();
        wire[field] = json!("not-permitted");
        assert!(serde_json::from_value::<NodeRuntimeBinding>(wire).is_err());
    }
    let mut wire = binding();
    wire["api"] = json!("guess-from-model");
    assert!(serde_json::from_value::<NodeRuntimeBinding>(wire).is_err());
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(DecisionProbability::new(value).is_err());
    }
}
