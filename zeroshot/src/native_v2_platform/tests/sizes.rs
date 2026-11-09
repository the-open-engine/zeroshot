use openengine_cluster_protocol::{RunSizeDescriptor, TargetRunSizesDiscovery};
use openengine_cluster_testkit::assertions::AssertError;

use super::*;

fn catalog(id: &str) -> TargetRunSizesDiscovery {
    let size = RunSize::new(id).assert_value();
    TargetRunSizesDiscovery::new(
        Some(size.clone()),
        vec![RunSizeDescriptor::new(size, "Target-owned resources").assert_value()],
    )
    .assert_value()
}

#[tokio::test]
async fn default_size_is_pinned_and_original_request_replays_after_catalog_removal() {
    let harness = harness(Behavior::Hang).await;
    let controller = harness.controller.with_run_sizes(catalog("custom-big-box"));
    let mut request = request(Value::Null);
    request.submission.runtime.size = None;
    let original = request.clone();
    let first = submit_test_request(&controller, request)
        .await
        .assert_value();
    harness.driver.wait_started().await;
    let stored = harness
        .ledger
        .get(&first.run_id)
        .await
        .assert_value()
        .assert_value();
    assert_eq!(stored.snapshot.size.as_str(), "custom-big-box");
    assert_eq!(
        stored.admitted.runtime.size().map(RunSize::as_str),
        Some("custom-big-box")
    );
    let controller =
        controller.with_run_sizes(TargetRunSizesDiscovery::new(None, Vec::new()).assert_value());
    let replay = submit_test_request(&controller, original)
        .await
        .assert_value();
    assert!(replay.deduped);
    assert_eq!(replay.run_id, first.run_id);
    assert_eq!(harness.allocator.allocation_count(), 1);
    controller
        .force(RunForceParams {
            run_id: first.run_id,
        })
        .await
        .assert_value();
}

#[tokio::test]
async fn unoffered_size_is_rejected_before_ledger_or_capsule_allocation() {
    let harness = harness(Behavior::Complete).await;
    let controller = harness.controller.with_run_sizes(catalog("custom-big-box"));
    let mut request = request(Value::Null);
    request.submission.runtime.size = Some(RunSize::new("not-offered").assert_value());
    let error = submit_test_request(&controller, request)
        .await
        .assert_error();
    assert!(
        error
            .to_string()
            .contains("does not offer run size not-offered")
    );
    assert!(harness.ledger.list().await.assert_value().is_empty());
    assert_eq!(harness.allocator.allocation_count(), 0);
}
