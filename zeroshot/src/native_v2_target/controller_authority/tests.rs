use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};
use zeroshot_engine::native_v2_target_authority::TargetAuthentication;

use super::*;

const REFUSAL: &str =
    "target does not support per-node runtime lanes (openengine.node-runtime-lanes/v1)";

fn descriptor(document: TargetDiscoveryDocument) -> ControllerDescriptor {
    let origin = Url::parse("http://127.0.0.1:8080").assert_value();
    build_controller_descriptor(&origin, document, TargetAuthentication::None).assert_value()
}

fn without_marker() -> ControllerDescriptor {
    descriptor(TargetDiscoveryDocument::direct(TargetAuthentication::None))
}

fn with_marker() -> ControllerDescriptor {
    descriptor(
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_node_runtime_lanes(),
    )
}

fn runtime(worker: Value) -> RuntimePlan {
    serde_json::from_value(json!({
        "harness": "codex",
        "provider": "openai",
        "size": "small",
        "nodes": { "worker": worker }
    }))
    .assert_value()
}

fn lane_bearing(harness: &str, provider: &str) -> RuntimePlan {
    runtime(json!({
        "kind": "agent",
        "lane": { "harness": harness, "provider": provider },
        "model": "opaque-model"
    }))
}

fn lane_free() -> RuntimePlan {
    runtime(json!({ "kind": "agent", "model": "opaque-model" }))
}

#[test]
fn lane_bearing_plans_are_refused_by_targets_without_the_exact_marker() {
    let mut other_kind =
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_node_runtime_lanes();
    other_kind
        .extensions
        .node_runtime_lanes
        .as_mut()
        .assert_value()
        .kind = "openengine.node-runtime-lanes/v2".to_owned();
    for controller in [without_marker(), descriptor(other_kind)] {
        for runtime in [
            lane_bearing("claude", "anthropic"),
            lane_bearing("codex", "openai"),
        ] {
            assert_eq!(
                require_node_runtime_lanes(&controller, &runtime)
                    .assert_error()
                    .to_string(),
                REFUSAL
            );
        }
    }
}

#[test]
fn lane_bearing_plans_are_accepted_by_targets_with_the_marker() {
    require_node_runtime_lanes(&with_marker(), &lane_bearing("claude", "anthropic")).assert_value();
}

#[test]
fn lane_free_plans_are_accepted_by_every_target() {
    for controller in [without_marker(), with_marker()] {
        require_node_runtime_lanes(&controller, &lane_free()).assert_value();
    }
}

#[test]
fn target_harness_catalog_checks_default_and_effective_lanes_with_legacy_fallback() {
    use openengine_cluster_protocol::{RuntimeLane, TargetRuntimeHarnessesDiscovery};
    let codex = RuntimeLane::new("codex", "openai").assert_value();
    let opencode = RuntimeLane::new("opencode", "openai").assert_value();
    let only_core = descriptor(
        TargetDiscoveryDocument::direct(TargetAuthentication::None)
            .with_node_runtime_lanes()
            .with_runtime_harnesses(
                TargetRuntimeHarnessesDiscovery::new(vec![codex.clone()]).assert_value(),
            ),
    );
    let mut custom_default = lane_bearing("codex", "openai");
    custom_default.harness = opencode.harness.clone();
    for controller in [without_marker(), only_core] {
        let mut plan = lane_free();
        plan.harness = opencode.harness.clone();
        assert!(require_runtime_capabilities(&controller, &plan).is_err());
    }
    let core_only = descriptor(
        TargetDiscoveryDocument::direct(TargetAuthentication::None)
            .with_node_runtime_lanes()
            .with_runtime_harnesses(
                TargetRuntimeHarnessesDiscovery::new(vec![codex.clone()]).assert_value(),
            ),
    );
    assert!(
        require_runtime_capabilities(&core_only, &custom_default)
            .assert_error()
            .to_string()
            .contains("opencode/openai")
    );
    assert!(require_runtime_capabilities(&core_only, &lane_bearing("opencode", "openai")).is_err());
    require_runtime_capabilities(&without_marker(), &lane_free()).assert_value();
    let enabled = descriptor(
        TargetDiscoveryDocument::direct(TargetAuthentication::None)
            .with_node_runtime_lanes()
            .with_runtime_harnesses(
                TargetRuntimeHarnessesDiscovery::new(vec![codex, opencode]).assert_value(),
            ),
    );
    require_runtime_capabilities(&enabled, &custom_default).assert_value();
    require_runtime_capabilities(&enabled, &lane_bearing("opencode", "openai")).assert_value();
}

#[test]
fn future_harness_catalog_versions_do_not_enable_contributions() {
    use openengine_cluster_protocol::{RuntimeLane, TargetRuntimeHarnessesDiscovery};
    let mut catalog = TargetRuntimeHarnessesDiscovery::new(vec![
        RuntimeLane::new("opencode", "openai").assert_value(),
    ])
    .assert_value();
    catalog.kind = "openengine.runtime-harnesses/v2".to_owned();
    let controller = descriptor(
        TargetDiscoveryDocument::direct(TargetAuthentication::None)
            .with_node_runtime_lanes()
            .with_runtime_harnesses(catalog),
    );
    assert!(controller.runtime_harnesses.is_none());
    assert!(
        require_runtime_capabilities(&controller, &lane_bearing("opencode", "openai")).is_err()
    );
    require_runtime_capabilities(&controller, &lane_free()).assert_value();
}

#[test]
fn decision_plans_require_their_exact_discovery_marker() {
    let plan = runtime(
        json!({"kind":"decision","api":"systemone","model":"opaque-model","minProbability":0.85,
        "uncertainLabel":"uncertain","connections":{"decisions":["DECISION_ENDPOINT","DECISION_API_KEY"]}}),
    );
    let document =
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_decision_models();
    require_runtime_capabilities(&descriptor(document.clone()), &plan).assert_value();
    assert!(require_runtime_capabilities(&without_marker(), &plan).is_err());
    let mut changed = document;
    changed
        .extensions
        .decision_models
        .as_mut()
        .assert_value()
        .kind = "openengine.decision-models/v2".to_owned();
    assert!(require_runtime_capabilities(&descriptor(changed), &plan).is_err());
    require_runtime_capabilities(&without_marker(), &lane_free()).assert_value();
}
