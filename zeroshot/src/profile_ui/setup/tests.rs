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
    let result = crate::workspace::profile_setup(json!({"runtime":runtime})).assert_value();
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
        crate::workspace::profile_setup(json!({"runtime":result["runtime"]})).assert_value()
    );
}

#[test]
fn authored_alternative_endpoints_and_custom_connections_survive_setup() {
    let result = crate::workspace::profile_setup(json!({"runtime":{
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
        crate::workspace::profile_setup(json!({"runtime":{
            "harness":"unknown", "provider":"unknown", "nodes":{}
        }}))
        .is_err()
    );
    assert!(
        crate::workspace::profile_setup(json!({"runtime":{
            "harness":"codex", "provider":"anthropic", "nodes":{}
        }}))
        .is_err()
    );
    assert!(
        crate::workspace::profile_setup(json!({"runtime":{
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

#[test]
fn catalog_advertises_gateway_protocol_choices_only_for_selectable_lanes() {
    use crate::harnesses::HarnessRegistry;

    let core: &[&str] = &["copilot", "pi"];
    let local: &[&str] = if cfg!(feature = "contributed-harnesses") {
        &["copilot", "hermes", "opencode", "pi"]
    } else {
        core
    };
    let fields = json!(["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"]);
    let options = json!({
        "GATEWAY_API": {
            "description": "Wire protocol spoken by the gateway endpoint.",
            "allowedValues": ["openai-responses", "openai-completions", "anthropic-messages"],
        }
    });
    for (registry, expected_harnesses) in [
        (HarnessRegistry::core(), core),
        (HarnessRegistry::local(), local),
    ] {
        let catalog = crate::workspace::catalog_with_harnesses(&registry).assert_value();
        let mut actual_harnesses = Vec::new();
        for lane in catalog["setupLanes"].as_array().assert_value() {
            if let Some(actual_options) = lane.get("fieldOptions") {
                assert_eq!(lane["provider"], "gateway");
                assert_eq!(lane["connectionKey"], "gateway");
                assert_eq!(lane["requiredFields"], fields);
                assert_eq!(lane["acceptedFieldSets"], json!([fields]));
                assert_eq!(*actual_options, options);
                actual_harnesses.push(lane["harness"].as_str().assert_value());
            }
        }
        assert_eq!(actual_harnesses, expected_harnesses);
    }
}

#[test]
fn setup_materializes_access_through_the_selected_custom_registry() {
    use crate::harnesses::{
        HarnessDescriptor, HarnessError, HarnessRegistration, HarnessRegistry, HarnessSource,
        ProviderAccessContract,
    };
    let mut registry = HarnessRegistry::core();
    registry
        .register(HarnessRegistration {
            cli_version: None,
            descriptor: HarnessDescriptor {
                id: "fixture",
                label: "Fixture",
                providers: &["openai"],
                executable: "fixture",
                source: HarnessSource::Contributed,
            },
            build: |_, _| Err(HarnessError::Configuration("not executed".into())),
            access: |_| {
                Ok(ProviderAccessContract::new(
                    false,
                    "custom_auth",
                    &["CUSTOM_PROVIDER_TOKEN"],
                    &[&["CUSTOM_PROVIDER_TOKEN"]],
                ))
            },
            normalize_local: |_, _| Ok(()),
            validate_config: |_| Ok(()),
        })
        .assert_value();
    let request = json!({"runtime": {
        "harness":"fixture", "provider":"openai", "nodes": {
            "worker":{"kind":"agent", "model":"caller-owned"}
        }
    }});
    let prepared =
        crate::workspace::profile_setup_with_harnesses(request.clone(), &registry).assert_value();
    assert_eq!(
        prepared["requirements"],
        json!({"custom_auth":["CUSTOM_PROVIDER_TOKEN"]})
    );
    assert!(crate::workspace::profile_setup(request).is_err());
}
