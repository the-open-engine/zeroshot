use super::*;

#[tokio::test]
async fn rejected_limit_with_malformed_result_retains_continuation_and_crash_behavior() {
    for result in [
        r#"{"type":"result"}"#,
        r#"{"type":"result","subtype":17,"is_error":false}"#,
    ] {
        let script = r#"
set -eu
cat >/dev/null
printf 'started\n' >> starts
printf '%s\n' '{"type":"system","subtype":"api_retry","error":"rate_limit"}'
printf '%s\n' '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1893456000}}'
printf '%s\n' 'MALFORMED_RESULT'
exit 1
"#
        .replace("MALFORMED_RESULT", result);
        let (workspace, mut handle, mut durable) =
            scripted_failure("claude-malformed-quota-result", &script).await;
        let (captured, result) = tokio::join!(collect_durable(&mut durable), handle.completion());
        assert!(matches!(result, Err(NodeRunnerError::DriverDetail(_))));
        assert_eq!(workspace.read("starts").lines().count(), 2);
        let logs = captured.logs.join("\n");
        assert_eq!(logs.matches("continuing once").count(), 1);
        assert!(!logs.contains("usage quota exhausted"));
    }
}

#[tokio::test]
async fn rejected_quota_prevents_continuation_and_retains_usage_and_safe_diagnostics() {
    let script = r#"
set -eu
cat >/dev/null
printf 'started\n' >> starts
printf '%s\n' '{"type":"system","subtype":"api_retry","error":"rate_limit"}'
printf '%s\n' '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1893456000}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"limited sentinel-secret","model_usage_since_last_result":{"model":{"inputTokens":11,"outputTokens":4}}}'
exit 1
"#;
    let (workspace, mut handle, mut durable) = scripted_failure("claude-quota", script).await;
    let (captured, result) = tokio::join!(collect_durable(&mut durable), handle.completion());
    let Err(NodeRunnerError::QuotaExhausted(detail)) = result else {
        panic!("expected quota exhaustion")
    };
    assert!(detail.contains("usage quota exhausted"));
    assert!(detail.contains("2030-01-01T00:00:00Z"));
    assert!(detail.contains("zeroshot resume"));
    assert!(!detail.contains("sentinel-secret"));
    assert_eq!(workspace.read("starts").lines().count(), 1);
    assert!(!captured.logs.join("\n").contains("continuing once"));
    assert!(!captured.logs.join("\n").contains("sentinel-secret"));
    assert!(
        captured
            .usages
            .iter()
            .flatten()
            .any(|usage| usage.input_tokens.get() == 11 && usage.output_tokens.get() == 4)
    );
}

#[tokio::test]
async fn authentication_signals_prevent_continuation_and_redact_details() {
    for signal in [
        r#"{"type":"system","subtype":"api_retry","error":"authentication_failed"}"#,
        r#"{"type":"assistant","error":"authentication_failed","message":{"content":[]}}"#,
        r#"{"type":"system","subtype":"api_retry","error":"overloaded"}"#,
    ] {
        let script = format!(
            r#"
set -eu
cat >/dev/null
printf 'started\n' >> starts
printf '%s\n' '{signal}'
printf '%s\n' '{{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":401,"result":"rejected sentinel-secret"}}'
exit 1
"#
        );
        let (workspace, mut handle, mut durable) =
            scripted_failure("claude-auth-rejection", &script).await;
        let (captured, result) = tokio::join!(collect_durable(&mut durable), handle.completion());
        assert!(
            matches!(result, Err(NodeRunnerError::AuthenticationRequired(ref detail))
            if detail.contains("rejected") && !detail.contains("sentinel-secret"))
        );
        assert_eq!(workspace.read("starts").lines().count(), 1);
        assert!(!captured.logs.join("\n").contains("continuing once"));
    }
}

#[tokio::test]
async fn native_model_marker_on_failed_turn_prevents_continuation() {
    let script = r#"
set -eu
cat >/dev/null
printf 'started\n' >> starts
printf '%s\n' '[claude-code:unrecognized_model] {"model":"issue1400-invalid-model","query_source":"sdk"}' >&2
printf '%s\n' '{"type":"system","subtype":"api_retry","error":"overloaded"}'
printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"not accepted sentinel-secret"}'
exit 1
"#;
    let (workspace, mut handle, mut durable) =
        scripted_failure("claude-model-rejection", script).await;
    let (captured, result) = tokio::join!(collect_durable(&mut durable), handle.completion());
    assert!(
        matches!(result, Err(NodeRunnerError::ModelUnavailable(ref detail))
        if detail.contains("issue1400-invalid-model") && !detail.contains("sentinel-secret"))
    );
    assert_eq!(workspace.read("starts").lines().count(), 1);
    assert!(!captured.logs.join("\n").contains("continuing once"));
}

#[test]
fn successful_native_result_ignores_model_warning_and_recovered_authentication() {
    use crate::native_v2_capsule::provider_process::turn::ProviderTranscript;
    let mut transcript = crate::native_v2_claude::transcript::ClaudeTranscript::new(Vec::new());
    transcript.observe_stderr(br#"[claude-code:unrecognized_model] {"model":"custom-alias"}"#);
    transcript.push(
        b"{\"type\":\"system\",\"subtype\":\"api_retry\",\"error\":\"authentication_failed\"}\n",
    );
    transcript.push(
        b"{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"{\\\"response\\\":null}\"}\n",
    );
    assert!(matches!(
        transcript.finish(None),
        Ok(crate::native_v2_capsule::provider_process::ProviderAttempt::Complete(_))
    ));
}

#[test]
fn authentication_retry_signal_never_authorizes_continuation() {
    use crate::native_v2_capsule::provider_process::turn::ProviderTranscript;
    let mut transcript = crate::native_v2_claude::transcript::ClaudeTranscript::new(Vec::new());
    transcript.push(
        b"{\"type\":\"system\",\"subtype\":\"api_retry\",\"error\":\"authentication_failed\"}\n",
    );
    transcript.push(b"{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true,\"result\":\"HTTP 401\"}\n");
    assert!(matches!(
        transcript.finish(None),
        Err(NodeRunnerError::AuthenticationRequired(_))
    ));
}

#[test]
fn model_warning_alone_cannot_reclassify_a_cut_or_malformed_stream() {
    use crate::native_v2_capsule::provider_process::turn::ProviderTranscript;
    for bytes in [b"{\"type\":\"result\"".as_slice(), b"not JSON\n".as_slice()] {
        let mut transcript = crate::native_v2_claude::transcript::ClaudeTranscript::new(Vec::new());
        transcript.observe_stderr(br#"[claude-code:unrecognized_model] {"model":"custom-alias"}"#);
        transcript.push(bytes);
        transcript.finish_stream();
        assert!(matches!(
            transcript.finish(None),
            Ok(crate::native_v2_capsule::provider_process::ProviderAttempt::Failed(_))
        ));
    }
}
