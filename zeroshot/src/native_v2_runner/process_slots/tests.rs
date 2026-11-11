use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

#[test]
fn released_sessions_recycle_slots_and_uncertain_cleanup_quarantines_ownership() {
    let pool = ProcessSlots::default();
    let first = pool.acquire().assert_value();
    let second = pool.clone().acquire().assert_value();
    assert_ne!(first.slot, second.slot);
    first.quarantine();
    first.release();
    second.release();
    let third = pool.acquire().assert_value();
    assert_eq!(third.slot, second.slot);
    assert_ne!(third.home, second.home);
    assert_ne!(third.home, first.home);
    assert_ne!(third.slot, first.slot);
}
