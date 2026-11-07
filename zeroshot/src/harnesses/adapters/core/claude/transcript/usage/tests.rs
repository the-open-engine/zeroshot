use openengine_cluster_protocol::MAX_SAFE_GENERATION;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

fn count(value: u64) -> Option<TokenCount> {
    Some(TokenCount::new(value).assert_value())
}

fn snapshot(input: u64, output: u64) -> UsageSnapshot {
    UsageSnapshot {
        input_tokens: count(input),
        output_tokens: count(output),
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    }
}

#[test]
fn coverage_contract_usage_snapshots_merge_partial_fields_and_reject_incomplete_totals() {
    let mut merged = UsageSnapshot::default();
    merged.merge(UsageSnapshot {
        input_tokens: count(3),
        output_tokens: count(5),
        cache_read_input_tokens: count(7),
        cache_creation_input_tokens: count(11),
    });
    let complete = merged.complete().assert_value();
    assert_eq!(
        (complete.input_tokens.get(), complete.output_tokens.get()),
        (3, 5)
    );
    assert_eq!(complete.cache_read_input_tokens.assert_value().get(), 7);
    assert_eq!(
        complete.cache_creation_input_tokens.assert_value().get(),
        11
    );
    assert!(!UsageSnapshot::default().has_value());
    assert!(
        UsageSnapshot {
            cache_read_input_tokens: count(1),
            ..UsageSnapshot::default()
        }
        .has_value()
    );
    assert!(
        UsageSnapshot {
            input_tokens: count(1),
            ..UsageSnapshot::default()
        }
        .complete()
        .is_none()
    );

    let mut provisional = ProvisionalUsage::default();
    provisional.add_anonymous(snapshot(2, 3));
    provisional.add_anonymous(snapshot(5, 7));
    let total = provisional.total().assert_value();
    assert_eq!(
        (total.input_tokens.get(), total.output_tokens.get()),
        (7, 10)
    );
    provisional.add_anonymous(UsageSnapshot::default());
    assert!(provisional.total().is_none());
    provisional.add_anonymous(snapshot(1, 1));

    let mut overflow = ProvisionalUsage::default();
    overflow.add_anonymous(snapshot(MAX_SAFE_GENERATION, 1));
    overflow.add_anonymous(snapshot(1, 1));
    assert!(overflow.total().is_none());
}

fn stated(input: u64, output: u64) -> Value {
    json!({"model": {
        "inputTokens": input,
        "outputTokens": output,
        "cacheReadInputTokens": 0,
        "cacheCreationInputTokens": 0
    }})
}

fn counts(usage: Option<TokenUsageDelta>) -> Option<(u64, u64)> {
    usage.map(|usage| (usage.input_tokens.get(), usage.output_tokens.get()))
}

#[test]
fn coverage_contract_result_usage_sums_stated_values_and_fails_closed() {
    let none = ProvisionalUsage::default();
    let mut usage = ResultUsage::default();
    assert_eq!(counts(usage.total(&none)), None);
    usage.record(Some(&json!({})));
    assert_eq!(counts(usage.total(&none)), Some((0, 0)));
    usage.record(Some(&stated(3, 5)));
    usage.record(Some(&json!({})));
    usage.record(Some(&stated(7, 11)));
    assert_eq!(counts(usage.total(&none)), Some((10, 16)));

    let mut later = ProvisionalUsage::default();
    later.add_anonymous(snapshot(2, 1));
    assert_eq!(counts(usage.total(&later)), Some((12, 17)));
    later.invalidate();
    assert_eq!(counts(usage.total(&later)), None);

    for invalid in [
        None,
        Some(json!(null)),
        Some(json!({"model": {"inputTokens": 1}})),
        Some(json!({"model": {"input_tokens": 1, "output_tokens": 2}})),
        Some(stated(MAX_SAFE_GENERATION, 1)),
    ] {
        let mut usage = ResultUsage::default();
        usage.record(Some(&stated(1, 1)));
        usage.record(invalid.as_ref());
        usage.record(Some(&stated(1, 1)));
        assert_eq!(counts(usage.total(&none)), None, "{invalid:?}");
    }
}
