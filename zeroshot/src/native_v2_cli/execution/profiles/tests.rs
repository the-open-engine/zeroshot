use openengine_cluster_protocol::ProfileRuntimePlan;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

#[test]
fn only_target_owned_profiles_store_contained_provider_requirements() {
    let runtime: ProfileRuntimePlan = serde_json::from_value(json!({
        "harness":"codex",
        "provider":"openai",
        "size":"medium",
        "nodes":{"worker":{"kind":"agent","model":"provider-model"}}
    }))
    .assert_value();
    let mut local = runtime.clone();
    materialize_stored_provider_access(&mut local, false).assert_value();
    assert!(
        local
            .map_environment(|_| None)
            .connection_requirements()
            .is_empty()
    );

    let mut contained = runtime;
    materialize_stored_provider_access(&mut contained, true).assert_value();
    assert_eq!(
        serde_json::to_value(
            contained
                .map_environment(|_| None)
                .connection_requirements()
        )
        .assert_value(),
        json!({"openai":["OPENAI_API_KEY"]})
    );
}
