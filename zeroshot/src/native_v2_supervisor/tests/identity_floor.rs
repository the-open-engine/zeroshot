use super::*;
use super::checkpoint_tests::prerequisite;

fn route_graph() -> GraphSpec {
    graph(
        sequence(
            vec![
                step("prepare", 10_000),
                verifier("review", 10_000),
                json!({"kind":"choice", "name":"route", "state":null_type(),
            "branches":[{"when":signal_guard("review", "accepted"), "node":step("sender", 10_000)}],
            "otherwise":step("receiver", 10_000), "promotedStatePaths":[]}),
                succeed("done"),
            ],
            null_type(),
        ),
        null_type(),
    )
}

fn dispatched(reduction: crate::full_v1_reducer::Reduction) -> (u64, u64, String) {
    reduction
        .decisions
        .into_iter()
        .find_map(|decision| match decision {
            Decision::Dispatch {
                node_instance,
                execution,
                occurrence,
                ..
            } => Some((
                node_instance.get(),
                execution.get(),
                occurrence.node.to_string(),
            )),
            _ => None,
        })
        .assert_value()
}

#[tokio::test]
async fn earlier_checkpoint_retains_instances_and_allocates_alternate_route_above_source_ids() {
    let harness = harness(route_graph(), Value::Null, FakeDriver::default()).await;
    let original = stored_run(&harness.ledger).await;
    let mut source = original.snapshot.clone();
    source.execution_seed = vec![
        prerequisite(1, "prepare", success_for(NodeRole::Worker)),
        prerequisite(2, "review", verifier_outcome("accepted")),
        prerequisite(3, "sender", success_for(NodeRole::Worker)),
    ];
    let floor = source.reserved_node_instance();
    assert_eq!(floor, 3);
    let mut rollback = original.snapshot.clone();
    rollback.node_instance_floor = floor;
    rollback.execution_seed = source.execution_seed[..1].to_vec();
    assert_eq!(
        dispatched(reduce(&original.admitted, &rollback).assert_value()),
        (4, 2, "review".into())
    );
    let mut review = prerequisite(2, "review", verifier_outcome("rejected"));
    review.node_instance = crate::full_v1_reducer::NodeInstanceId::new(4).assert_value();
    rollback.execution_seed.push(review);
    assert_eq!(
        dispatched(reduce(&original.admitted, &rollback).assert_value()),
        (5, 3, "receiver".into())
    );
    assert_eq!(rollback.execution_seed[0].node_instance.get(), 1);
    rollback.execution_seed.clear();
    assert_eq!(
        dispatched(reduce(&original.admitted, &rollback).assert_value()),
        (4, 1, "prepare".into())
    );
    assert_eq!(harness.driver.max_active(), 0);
}

#[tokio::test]
async fn node_instance_floor_overflow_fails_without_allocating_or_charging_graph_work() {
    let harness = harness(route_graph(), Value::Null, FakeDriver::default()).await;
    let mut source = stored_run(&harness.ledger).await;
    source.snapshot.node_instance_floor = u64::MAX;
    assert!(matches!(
        reduce(&source.admitted, &source.snapshot),
        Err(NativeV2SupervisorError::InvalidState)
    ));
    assert_eq!(harness.driver.max_active(), 0);
}
