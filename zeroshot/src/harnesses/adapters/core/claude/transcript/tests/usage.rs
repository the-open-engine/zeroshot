use super::*;

fn usage(input: u64, output: u64, cache_read: u64, cache_creation: u64) -> Value {
    json!({
        "input_tokens":input,
        "output_tokens":output,
        "cache_read_input_tokens":cache_read,
        "cache_creation_input_tokens":cache_creation
    })
}

fn message_start(message_id: &str, usage: Value) -> Value {
    json!({
        "type":"stream_event",
        "session_id":"session-1",
        "event":{
            "type":"message_start",
            "message":{"id":message_id,"usage":usage}
        }
    })
}

fn assistant_usage(message_id: &str, usage: Value) -> Value {
    json!({
        "type":"assistant",
        "session_id":"session-1",
        "message":{"id":message_id,"content":[],"usage":usage}
    })
}

fn raw_message_usage(output_tokens: Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_event(&mut bytes, message_start("message-1", usage(23, 0, 17, 5)));
    append_event(
        &mut bytes,
        json!({
            "type":"stream_event",
            "session_id":"session-1",
            "event":{
                "type":"message_delta",
                "delta":{"stop_reason":"end_turn"},
                "usage":{"output_tokens":output_tokens}
            }
        }),
    );
    bytes
}

fn usage_values(usage: Option<TokenUsageDelta>) -> [u64; 4] {
    let usage = usage.assert_value();
    [
        usage.input_tokens.get(),
        usage.output_tokens.get(),
        usage.cache_read_input_tokens.assert_value().get(),
        usage.cache_creation_input_tokens.assert_value().get(),
    ]
}

#[test]
fn assistant_usage_sums_unique_messages_and_replaces_repeated_snapshots() {
    let mut bytes = Vec::new();
    append_event(
        &mut bytes,
        json!({
            "type":"stream_event",
            "event":{
                "type":"content_block_delta",
                "delta":{"type":"text_delta","text":"visible once"}
            }
        }),
    );
    for (id, input, output, cache_read, cache_creation) in [
        ("message-1", 11, 2, 6, 2),
        ("message-1", 11, 4, 6, 2),
        ("message-2", 7, 3, 5, 1),
    ] {
        append_event(
            &mut bytes,
            assistant_usage(id, usage(input, output, cache_read, cache_creation)),
        );
    }

    let decoded = decode(&bytes, 19, Some("provider process exited with status 17"));
    assert_eq!(usage_values(decoded.usage), [18, 7, 11, 3]);
    assert_eq!(
        decoded
            .emissions
            .iter()
            .map(|emission| emission.text.as_str())
            .collect::<String>(),
        "visible once"
    );
    assert!(
        failed_attempt(decoded.attempt)
            .diagnostic
            .contains("without a terminal result")
    );
}

#[test]
fn raw_message_usage_survives_a_missing_terminal_result() {
    let bytes = raw_message_usage(json!(9));

    let decoded = decode(&bytes, 7, Some("provider process exited with status 17"));
    assert_eq!(usage_values(decoded.usage), [23, 9, 17, 5]);
    assert!(
        failed_attempt(decoded.attempt)
            .diagnostic
            .contains("without a terminal result")
    );
}

#[test]
fn malformed_raw_usage_is_not_reported_as_complete() {
    let bytes = raw_message_usage(json!("many"));

    assert!(decode(&bytes, 17, Some("provider exited")).usage.is_none());
}

#[test]
fn unidentifiable_message_start_usage_after_the_last_result_is_incomplete_but_turn_can_succeed() {
    for message in [
        json!({"usage":usage(23, 9, 17, 5)}),
        json!({"id":null,"usage":usage(23, 9, 17, 5)}),
        json!({"id":"","usage":usage(23, 9, 17, 5)}),
        json!({"id":17,"usage":usage(23, 9, 17, 5)}),
    ] {
        let mut bytes = Vec::new();
        append_event(&mut bytes, success(json!("done"), "session-1"));
        append_event(
            &mut bytes,
            json!({
                "type":"stream_event",
                "session_id":"session-1",
                "event":{
                    "type":"message_start",
                    "message":message
                }
            }),
        );

        let decoded = decode(&bytes, 13, None);
        assert_eq!(completed_response(decoded.attempt), json!("done"));
        assert!(decoded.usage.is_none());
    }
}

#[test]
fn unassociated_message_delta_usage_after_the_last_result_is_incomplete_but_turn_can_succeed() {
    let mut bytes = Vec::new();
    append_event(&mut bytes, success(json!("done"), "session-1"));
    append_event(
        &mut bytes,
        json!({
            "type":"stream_event",
            "session_id":"session-1",
            "event":{"type":"message_delta","usage":{"output_tokens":9}}
        }),
    );

    let decoded = decode(&bytes, 17, None);
    assert_eq!(completed_response(decoded.attempt), json!("done"));
    assert!(decoded.usage.is_none());
}

#[test]
fn assistant_wrapper_replaces_the_same_raw_message_usage() {
    let mut bytes = Vec::new();
    append_event(&mut bytes, message_start("message-1", usage(23, 0, 17, 5)));
    append_event(
        &mut bytes,
        assistant_usage("message-1", usage(23, 9, 17, 5)),
    );

    let decoded = decode(&bytes, 29, Some("provider exited"));
    assert_eq!(usage_values(decoded.usage), [23, 9, 17, 5]);
}

#[test]
fn stated_result_usage_replaces_streamed_usage_before_the_result() {
    let mut bytes = Vec::new();
    append_event(
        &mut bytes,
        json!({
            "type":"assistant",
            "session_id":"session-1",
            "message":{
                "content":[],
                "usage":{
                    "input_tokens":97,
                    "output_tokens":53,
                    "cache_read_input_tokens":41,
                    "cache_creation_input_tokens":29
                }
            }
        }),
    );
    append_event(&mut bytes, success(json!("done"), "session-1"));

    let decoded = decode(&bytes, 64, None);
    assert_eq!(completed_response(decoded.attempt), json!("done"));
    assert_eq!(usage_values(decoded.usage), [11, 4, 6, 2]);
}

#[test]
fn streamed_usage_after_the_last_result_is_added_to_the_stated_usage() {
    // Claude Code states calls after its last result nowhere, as when a process stops mid-turn.
    let mut bytes = Vec::new();
    append_event(&mut bytes, success(json!("early"), "session-1"));
    append_event(
        &mut bytes,
        assistant_usage("message-2", usage(23, 9, 17, 5)),
    );

    let decoded = decode(&bytes, 29, Some("provider process exited with status 17"));
    assert_eq!(usage_values(decoded.usage), [34, 13, 23, 7]);
}

#[test]
fn per_result_usage_sums_all_models_and_ignores_session_and_main_thread_totals() {
    let model = |offset: u64| {
        json!({
            "inputTokens":13 + offset,
            "outputTokens":7 + offset,
            "cacheReadInputTokens":11 + offset,
            "cacheCreationInputTokens":5 + offset,
            "costUSD":0.01
        })
    };
    let mut event = success(json!("done"), "session-1");
    event["usage"] = usage(999, 999, 999, 999);
    event["modelUsage"] = json!({"model-a":model(900),"model-b":model(900)});
    event["model_usage_since_last_result"] = json!({"model-a":model(0),"model-b":model(1)});
    let mut bytes = Vec::new();
    append_event(&mut bytes, event);

    let decoded = decode(&bytes, 31, None);
    assert_eq!(usage_values(decoded.usage), [27, 15, 23, 11]);
}

#[test]
fn results_without_valid_per_result_usage_leave_usage_unknown() {
    for since in [
        None,
        Some(json!(null)),
        Some(json!({"model-a":{"inputTokens":"many","outputTokens":7}})),
        Some(json!({"model-a":{"input_tokens":13,"output_tokens":7}})),
    ] {
        for (subtype, is_error, result) in [
            ("success", false, "done"),
            ("error_during_execution", true, "provider failed"),
        ] {
            let mut bytes = Vec::new();
            append_event(
                &mut bytes,
                assistant_usage("message-1", usage(23, 9, 17, 5)),
            );
            let mut terminal = json!({
                "type":"result",
                "subtype":subtype,
                "is_error":is_error,
                "session_id":"session-1",
                "result":result,
                "usage":usage(23, 9, 17, 5),
                "modelUsage":{"model-a":{"inputTokens":23,"outputTokens":9}}
            });
            if let Some(since) = &since {
                terminal["model_usage_since_last_result"] = since.clone();
            }
            if !is_error {
                terminal["structured_output"] = json!({"response":result});
            }
            append_event(&mut bytes, terminal);

            let decoded = decode(&bytes, 29, None);
            assert!(decoded.usage.is_none(), "{since:?} {subtype}");
            if is_error {
                assert_eq!(failed_attempt(decoded.attempt).diagnostic, result);
            } else {
                assert_eq!(completed_response(decoded.attempt), json!(result));
            }
        }
    }
}
