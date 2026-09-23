use openengine_cluster_protocol::MAX_SAFE_GENERATION;
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn coverage_contract_event_kinds_and_usage_counts_reject_unknown_or_out_of_range_values() {
    for kind in [
        "assistant.message",
        "assistant.usage",
        "session.error",
        "permission.requested",
        "tool.execution_start",
        "tool.execution_complete",
    ] {
        assert!(known_event(kind));
    }
    assert!(!known_event("future.event"));

    let data = json!({
        "inputTokens": 4,
        "outputTokens": 6,
        "cacheReadTokens": null,
    });
    assert_eq!(count(&data, "inputTokens").assert_value().get(), 4);
    assert_eq!(
        optional_count(&data, "cacheReadTokens").assert_value(),
        None
    );
    assert_eq!(
        optional_count(&data, "cacheWriteTokens").assert_value(),
        None
    );
    assert!(count(&json!({"inputTokens": "many"}), "inputTokens").is_err());
    assert!(
        count(
            &json!({"inputTokens": MAX_SAFE_GENERATION + 1}),
            "inputTokens",
        )
        .is_err()
    );
}

#[test]
fn coverage_contract_event_chunking_preserves_utf8_and_the_exact_text() {
    let text = format!("{}é-tail", "a".repeat(8 * 1024 - 1));
    let (first, rest) = split_output_chunk(&text);
    assert_eq!(first.len(), 8 * 1024 - 1);
    assert!(rest.starts_with('é'));
    assert_eq!(format!("{first}{rest}"), text);
    assert_eq!(split_output_chunk("short"), ("short", ""));
}
