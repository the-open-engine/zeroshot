use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

#[test]
fn requirements_cover_effective_lanes_and_custom_worker_access() {
    let runtime = json!({
        "harness":"claude", "provider":"anthropic", "nodes":{
            "inspect":{"kind":"agent", "model":"caller-model", "effort":"high"},
            "verify":{"kind":"agent", "model":"other-model",
                "lane":{"harness":"codex", "provider":"openai"}},
            "classify":{"kind":"decision", "api":"systemone", "model":"choice-model",
                "minProbability":0.85, "uncertainLabel":"uncertain",
                "connections":{"decisions":["DECISION_API_KEY", "DECISION_ENDPOINT"]}},
            "deliver":{"kind":"comment_delivery", "connections":{"forge":["GH_TOKEN"]}}
        }
    });
    let result = apply(json!({"runtime":runtime})).assert_value();
    assert_eq!(
        result["requirements"],
        json!({
            "anthropic":["ANTHROPIC_API_KEY"], "openai":["OPENAI_API_KEY"],
            "decisions":["DECISION_API_KEY", "DECISION_ENDPOINT"], "forge":["GH_TOKEN"]
        })
    );
    assert_eq!(
        result["runtime"]["nodes"]["inspect"]["model"],
        "caller-model"
    );
    assert_eq!(result["runtime"]["nodes"]["inspect"]["effort"], "high");
    assert_eq!(
        result["runtime"]["nodes"]["verify"]["lane"],
        runtime["nodes"]["verify"]["lane"]
    );
    assert_eq!(
        result["runtime"]["nodes"]["classify"],
        runtime["nodes"]["classify"]
    );
    assert_eq!(
        result,
        apply(json!({"runtime":result["runtime"]})).assert_value()
    );
}

#[test]
fn authored_alternative_endpoints_and_custom_connections_survive_setup() {
    let result = apply(json!({"runtime":{
        "harness":"codex", "provider":"foundry", "nodes":{
            "work":{"kind":"agent", "model":"account-deployment", "connections":{
                "azure":["AZURE_FOUNDRY_BASE_URL", "AZURE_FOUNDRY_API_KEY"],
                "service":["PROJECT_SERVICE_API_KEY"]
            }}
        }
    }}))
    .assert_value();
    assert_eq!(
        result["requirements"],
        json!({
            "azure":["AZURE_FOUNDRY_API_KEY", "AZURE_FOUNDRY_BASE_URL"],
            "service":["PROJECT_SERVICE_API_KEY"]
        })
    );
    assert_eq!(
        result["runtime"]["nodes"]["work"]["model"],
        "account-deployment"
    );
}

#[test]
fn setup_rejects_unknown_lanes_and_unexpected_request_fields() {
    assert!(
        apply(json!({"runtime":{
            "harness":"unknown", "provider":"unknown", "nodes":{}
        }}))
        .is_err()
    );
    assert!(
        apply(json!({"runtime":{
            "harness":"codex", "provider":"anthropic", "nodes":{}
        }}))
        .is_err()
    );
    assert!(
        apply(json!({"runtime":{
        "harness":"codex", "provider":"openai", "nodes":{}
    }, "secret":"not-an-allowed-carrier"}))
        .is_err()
    );
}

#[test]
fn setup_choices_reuse_registered_provider_access_contracts() {
    let registry = crate::harnesses::HarnessRegistry::core();
    let choices = lanes(&registry).assert_value();
    assert_eq!(choices.len(), registry.lanes().len());
    let gateway = choices
        .iter()
        .find(|choice| choice["harness"] == "codex" && choice["provider"] == "gateway")
        .assert_value();
    assert_eq!(gateway["connectionKey"], "gateway");
    assert_eq!(
        gateway["requiredFields"],
        json!(["GATEWAY_BASE_URL", "GATEWAY_API_KEY"])
    );
    assert!(
        !gateway["acceptedFieldSets"]
            .as_array()
            .assert_value()
            .is_empty()
    );
}
