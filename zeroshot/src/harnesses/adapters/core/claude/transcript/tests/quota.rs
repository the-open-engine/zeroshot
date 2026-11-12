use super::*;

fn limit(status: &str, reset: Value) -> Value {
    json!({"type":"rate_limit_event","rate_limit_info":{
        "status":status,"rateLimitType":"five_hour","resetsAt":reset,
        "overageStatus":"rejected"
    }})
}

fn transcript(events: Vec<Value>) -> ClaudeTranscript {
    let mut transcript = ClaudeTranscript::new(Vec::new());
    let mut bytes = Vec::new();
    for event in events {
        append_event(&mut bytes, event);
    }
    for chunk in bytes.chunks(7) {
        transcript.push(chunk);
    }
    transcript.finish_stream();
    transcript
}

#[test]
fn rejected_limit_requires_a_failed_result_and_preserves_available_reset_information() {
    for (reset, expected) in [
        (json!(1893456000), Some("2030-01-01T00:00:00Z")),
        (json!(1893456000000_i64), Some("2030-01-01T00:00:00Z")),
        (json!(1893456000123_i64), Some("1893456000123")),
        (json!("5:08 PM"), Some("5:08 PM")),
        (Value::Null, None),
        (json!({"invalid":"reset"}), None),
        (json!(-1), Some("-1")),
        (json!(1.5), Some("1.5")),
        (json!(1e100), Some("1e+100")),
        (json!(""), None),
        (json!(true), None),
    ] {
        // Claude can use subtype=success with is_error=true for account exhaustion.
        let mut failed = failure("native limit hint", "session-1");
        failed["subtype"] = json!("success");
        let transcript = transcript(vec![limit("rejected", reset), failed]);
        let Err(NodeRunnerError::QuotaExhausted(detail)) = transcript.finish(None) else {
            panic!("expected a typed quota failure")
        };
        assert!(detail.contains("usage quota exhausted"));
        assert!(detail.contains("type=five_hour"));
        assert!(detail.contains("native limit hint"));
        assert!(detail.contains("zeroshot resume RUN_ID"));
        match expected {
            Some(expected) => assert!(detail.contains(&format!("resetsAt={expected}"))),
            None => assert!(!detail.contains("resetsAt=")),
        }
        if expected == Some("5:08 PM") {
            assert!(!detail.contains("T00:"));
            assert!(!detail.contains("UTC"));
        }
    }
}

#[test]
fn missing_limit_type_and_reset_do_not_hide_a_confirmed_rejection() {
    let transcript = transcript(vec![
        json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}),
        failure("Try again at 5:08 PM", "session-1"),
    ]);
    assert!(
        matches!(transcript.finish(None), Err(NodeRunnerError::QuotaExhausted(detail))
        if detail.contains("Try again at 5:08 PM") && !detail.contains("resetsAt="))
    );
}

#[test]
fn warnings_allowed_limits_and_message_text_leave_transient_failures_retryable() {
    let retry = json!({"type":"system","subtype":"api_retry","error":"rate_limit"});
    for events in [
        vec![limit("allowed", json!(1893456000))],
        vec![limit("allowed_warning", json!(1893456000))],
        vec![
            limit("rejected", Value::Null),
            limit("allowed", Value::Null),
        ],
        vec![
            limit("rejected", Value::Null),
            success(json!("done"), "session-1"),
        ],
        vec![json!({"type":"assistant","message":{"content":[{
            "type":"text","text":limit("rejected", json!(1893456000)).to_string()
        }]}})],
        vec![json!({"type":"user","message":{"content":[{
            "type":"tool_result","content":limit("rejected", json!(1893456000))
        }]}})],
        Vec::new(),
    ] {
        let mut events = events;
        events.push(retry.clone());
        events.push(failure(
            "usage limit reached; try again at 5:08 PM",
            "session-1",
        ));
        assert!(
            matches!(transcript(events).finish(None), Ok(ClaudeAttempt::Failed(failure))
            if failure.retryable)
        );
    }
}

#[test]
fn recovered_success_keeps_its_result() {
    for events in [
        vec![
            limit("rejected", Value::Null),
            success(json!("done"), "session-1"),
        ],
        vec![
            limit("rejected", Value::Null),
            failure("limited", "session-1"),
            success(json!("done"), "session-1"),
        ],
    ] {
        assert!(matches!(
            transcript(events).finish(None),
            Ok(ClaudeAttempt::Complete(_))
        ));
    }
}

#[test]
fn rejected_event_without_a_failed_native_result_is_not_quota_exhaustion() {
    let mut transcript = transcript(vec![limit("rejected", json!(1893456000))]);
    transcript.push(b"not JSON\n{\"type\":\"result\"");
    transcript.finish_stream();
    assert!(matches!(
        transcript.finish(None),
        Ok(ClaudeAttempt::Failed(_))
    ));
}

#[test]
fn malformed_result_metadata_cannot_confirm_quota_exhaustion() {
    for malformed in [
        json!({"type":"result"}),
        json!({"type":"result","subtype":17,"is_error":false}),
        json!({"type":"result","subtype":null,"is_error":true}),
        json!({"type":"result","subtype":"unknown","is_error":true}),
    ] {
        for earlier_failure in [false, true] {
            let mut events = vec![limit("rejected", json!(1893456000))];
            if earlier_failure {
                events.push(failure("earlier native failure", "session-1"));
            }
            events.push(malformed.clone());
            let transcript = transcript(events);
            assert!(
                !transcript
                    .failure_diagnostic(None)
                    .assert_value()
                    .contains("usage quota exhausted")
            );
            assert!(matches!(
                transcript.finish(None),
                Ok(ClaudeAttempt::Failed(_))
            ));
        }
    }
}

#[test]
fn native_error_result_subtypes_confirm_quota_exhaustion() {
    for subtype in [
        "error_during_execution",
        "error_max_turns",
        "error_max_budget_usd",
        "error_max_structured_output_retries",
    ] {
        let transcript = transcript(vec![
            limit("rejected", Value::Null),
            json!({"type":"result","subtype":subtype,"is_error":false,"errors":["limited"]}),
        ]);
        assert!(matches!(
            transcript.finish(None),
            Err(NodeRunnerError::QuotaExhausted(_))
        ));
    }
}

#[test]
fn quota_diagnostic_survives_unconfirmed_cleanup_and_is_redacted() {
    let mut transcript = transcript(vec![
        limit("rejected", json!(1893456000)),
        failure("native sentinel-secret\u{1b} detail", "session-1"),
    ]);
    transcript.pending = ProviderText::new(&["sentinel-secret".to_owned()]);
    let diagnostic = transcript
        .failure_diagnostic(Some("cleanup failed"))
        .assert_value();
    assert!(diagnostic.contains("usage quota exhausted"));
    assert!(diagnostic.contains("2030-01-01T00:00:00Z"));
    assert!(diagnostic.contains("zeroshot resume"));
    assert!(diagnostic.contains("cleanup failed"));
    assert!(!diagnostic.contains("sentinel-secret"));
    assert!(!diagnostic.contains('\u{1b}'));
}
