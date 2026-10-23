use openengine_cluster_testkit::assertions::AssertValue;

use crate::execution::process::{HostedProcessPool, HostedProcessScope};

use super::ActiveRunProcessPools;

#[test]
fn active_leases_are_disjoint_and_the_released_slot_is_reused() {
    let pools = pools();
    let first = pools.acquire().assert_value();
    let second = pools.acquire().assert_value();
    let first_uid = writer_uid(first.process_pool());
    let second_uid = writer_uid(second.process_pool());
    assert_ne!(first_uid, second_uid);

    drop(first);
    let replacement = pools.acquire().assert_value();
    assert_eq!(writer_uid(replacement.process_pool()), first_uid);
    assert_eq!(writer_uid(second.process_pool()), second_uid);
}

#[test]
fn exhausted_identity_space_rejects_the_lease() {
    assert!(
        ActiveRunProcessPools::new(
            HostedProcessPool::new(10_002, 10_002, u32::MAX - 1).assert_value(),
        )
        .is_err()
    );
}

#[test]
fn contained_auxiliary_leases_reserve_a_second_complete_execution_range() {
    let pools = pools();
    let first = pools.acquire().assert_value();
    let second = pools.acquire().assert_value();
    let primary_limit = openengine_cluster_server::graph_verifier::FULL_V1_MAX_NODE_EXECUTIONS;
    let expected_width = u32::try_from(primary_limit * 2 * 4 + 3).assert_value();
    assert_eq!(
        writer_uid(second.process_pool()) - writer_uid(first.process_pool()),
        expected_width
    );
    for identity in [primary_limit, primary_limit + 1, primary_limit * 2] {
        first
            .process_pool()
            .identity(HostedProcessScope::VerifierExecution(identity))
            .assert_value();
    }
}

fn pools() -> ActiveRunProcessPools {
    ActiveRunProcessPools::new(HostedProcessPool::new(10_002, 10_002, 20_000).assert_value())
        .assert_value()
}

fn writer_uid(pool: HostedProcessPool) -> u32 {
    pool.identity(HostedProcessScope::Writer)
        .assert_value()
        .uid()
}
