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

#[test]
fn active_lease_delays_slot_reuse_and_closed_slots_refuse_new_leases() {
    let pool = ProcessSlots::default();
    let first = pool.acquire().assert_value();
    first.begin().assert_value();
    first.release();
    assert!(matches!(first.begin(), Err(NodeRunnerError::SessionLost)));
    assert_eq!(first.active.load(Ordering::SeqCst), 1);
    let second = pool.acquire().assert_value();
    assert_ne!(first.slot, second.slot);
    first.end();
    let third = pool.acquire().assert_value();
    assert_eq!(third.slot, first.slot);
    assert_ne!(third.home, first.home);
    assert!(matches!(first.begin(), Err(NodeRunnerError::SessionLost)));
    assert_eq!(first.active.load(Ordering::SeqCst), 0);
}
