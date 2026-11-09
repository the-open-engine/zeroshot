use openengine_cluster_protocol::{RunSize, RunSizeDescriptor};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::*;

fn catalog(default: &str) -> TargetRunSizesDiscovery {
    let id = RunSize::new(default).assert_value();
    TargetRunSizesDiscovery::new(
        Some(id.clone()),
        vec![RunSizeDescriptor::new(id, "Target-owned compute resources").assert_value()],
    )
    .assert_value()
}

fn runtime() -> RuntimePlan {
    serde_json::from_value(json!({"harness":"codex","provider":"openai","nodes":{}})).assert_value()
}

#[test]
fn omitted_size_uses_the_target_default_and_arbitrary_offered_ids_are_preserved() {
    let mut plan = runtime();
    resolve_runtime_size(&mut plan, &catalog("custom-big-box")).assert_value();
    assert_eq!(plan.size().map(RunSize::as_str), Some("custom-big-box"));
    let mut explicit = runtime();
    explicit.size = Some(RunSize::new("custom-big-box").assert_value());
    resolve_runtime_size(&mut explicit, &catalog("custom-big-box")).assert_value();
    assert_eq!(explicit.size(), plan.size());
}

#[test]
fn unsupported_sizes_and_empty_catalogs_are_target_admission_errors() {
    let mut plan = runtime();
    plan.size = Some(RunSize::new("not-offered").assert_value());
    assert!(
        resolve_runtime_size(&mut plan, &catalog("custom-big-box"))
            .assert_error()
            .to_string()
            .contains("does not offer run size not-offered")
    );
    assert_eq!(plan.size().map(RunSize::as_str), Some("not-offered"));
    let mut omitted = runtime();
    let empty = TargetRunSizesDiscovery::new(None, Vec::new()).assert_value();
    assert!(resolve_runtime_size(&mut omitted, &empty).is_err());
    assert!(omitted.size().is_none());
}
