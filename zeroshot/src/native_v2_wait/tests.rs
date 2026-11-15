use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
#[test]
fn deadline_contract_and_engine_bound() {
    let now = UnixTimestampMillis::new(1_762_905_600_000).assert_value();
    for input in [
        json!({"durationMs":0}),
        json!({"durationMs":-1}),
        json!({"dueAt":"2000-01-01T00:00:00Z"}),
    ] {
        assert!(deadline(&input, now).assert_value() <= now);
    }
    assert_eq!(
        deadline(&json!({"durationMs":MAX_WAIT_MILLIS}), now)
            .assert_value()
            .get(),
        now.get() + MAX_WAIT_MILLIS
    );
    for input in [
        json!({}),
        json!({"dueAt":"2026-12-01T09:00:00"}),
        json!({"durationMs":1,"dueAt":"2000-01-01T00:00:00Z"}),
        json!({"durationMs":MAX_WAIT_MILLIS+1}),
    ] {
        assert!(deadline(&input, now).is_err(), "{input}");
    }
}
