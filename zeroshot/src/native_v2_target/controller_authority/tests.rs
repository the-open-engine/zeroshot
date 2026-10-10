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
