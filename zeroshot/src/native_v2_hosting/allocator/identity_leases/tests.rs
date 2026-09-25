use openengine_cluster_testkit::assertions::AssertValue;

use crate::execution::process::{HostedProcessPool, HostedProcessScope};

use super::ActiveRunProcessPools;

#[test]
fn active_leases_are_disjoint_and_the_released_slot_is_reused() {
    let pools =
        ActiveRunProcessPools::new(HostedProcessPool::new(10_002, 10_002, 20_000).assert_value())
            .assert_value();
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

fn writer_uid(pool: HostedProcessPool) -> u32 {
    pool.identity(HostedProcessScope::Writer)
        .assert_value()
        .uid()
}
