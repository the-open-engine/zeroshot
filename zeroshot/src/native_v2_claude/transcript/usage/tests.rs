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

#[test]
fn coverage_contract_terminal_usage_aliases_and_optional_counts_fail_closed() {
    assert!(matches!(
        parse_terminal_usage(
            Some(&json!({"model": {"inputTokens": 1, "outputTokens": 2}})),
            Some(&json!({"different": {"input_tokens": 1, "output_tokens": 2}})),
            None
        ),
        TerminalUsage::Malformed
    ));
    assert!(matches!(
        parse_terminal_usage(None, None, Some(&json!(null))),
        TerminalUsage::Empty
    ));
    assert!(matches!(
        parse_terminal_usage(None, None, Some(&json!({}))),
        TerminalUsage::Empty
    ));
    assert!(matches!(
        parse_terminal_usage(
            None,
            Some(&json!({"model": {"input_tokens": 4, "output_tokens": 6}})),
            None,
        ),
        TerminalUsage::Valid(usage)
            if usage.input_tokens.get() == 4 && usage.output_tokens.get() == 6
    ));

    let keys = UsageKeys {
        input: UsageKey::aliased("input", "input_alt"),
        output: UsageKey::single("output"),
        cache_read: UsageKey::single("read"),
        cache_creation: UsageKey::single("write"),
    };
    assert!(
        parse_usage_snapshot(
            Some(&json!({"input": 1, "input_alt": 2, "output": 3})),
            keys,
        )
        .is_none()
    );
    let parsed = parse_usage_snapshot(Some(&json!({"input_alt": 1, "output": 3, "read": 4})), keys)
        .assert_value()
        .complete()
        .assert_value();
    assert_eq!(parsed.input_tokens.get(), 1);
    assert_eq!(parsed.cache_read_input_tokens.assert_value().get(), 4);
    assert!(parsed.cache_creation_input_tokens.is_none());

    assert_eq!(add_optional_tokens(count(1), None), Some(None));
    assert!(add_optional_tokens(count(MAX_SAFE_GENERATION), count(1)).is_none());
}
