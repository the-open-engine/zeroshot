//! Tests for `native_v2_pi/transcript.rs`.
//!
//! Kept outside the production-named source file so production coverage totals exclude them
//! (`AGENTS.md`, `COVERAGE_IGNORE_REGEX` in `.github/workflows/coverage.yml`).

use super::{PiTranscript, assistant_text};
use crate::native_v2_runner::LiveOutputStream;
use serde_json::json;

fn header(id: &str) -> String {
    json!({"type": "session", "version": 3, "id": id, "cwd": "/w"}).to_string()
}

fn assistant(text: &str) -> serde_json::Value {
    json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "stopReason": "stop"
        }
    })
}

#[test]
fn the_final_assistant_message_is_the_response() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(format!("{}\n{}\n", header("s1"), assistant("done")).as_bytes());
    let attempt = transcript.finish(None).unwrap();
    let super::PiAttempt::Complete(result) = attempt else {
        panic!("expected a complete attempt");
    };
    assert_eq!(result.message, "done");
    assert_eq!(result.session_id.as_deref(), Some("s1"));
}

#[test]
fn deltas_stream_live_but_never_replace_the_authoritative_message() {
    let mut transcript = PiTranscript::new(Vec::new());
    let emissions = transcript.push(
        [
            header("s1").as_str(),
            &json!({"type": "message_update", "assistantMessageEvent": {"type": "text_start", "contentIndex": 0}}).to_string(),
            &json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "Hel"}}).to_string(),
            &json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "lo"}}).to_string(),
            "",
        ]
        .join("\n")
        .as_bytes(),
    );
    let streamed: String = emissions
        .iter()
        .filter(|e| e.stream == LiveOutputStream::Output)
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(streamed, "Hello");

    transcript.push(assistant("Hello world").to_string().as_bytes());
    let super::PiAttempt::Complete(result) = transcript.finish(None).unwrap() else {
        panic!("expected a complete attempt");
    };
    assert_eq!(result.message, "Hello world");
}

#[test]
fn a_delta_repeating_earlier_text_is_still_emitted() {
    // A fragment must not be dropped just because its characters appeared earlier: a model
    // repeats words constantly, and dropping them renders the live transcript as word salad.
    let mut transcript = PiTranscript::new(Vec::new());
    let emissions = transcript.push(
        [
            header("s1").as_str(),
            &json!({"type": "message_update", "assistantMessageEvent": {"type": "text_start", "contentIndex": 0}}).to_string(),
            &json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "Let"}}).to_string(),
            &json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": " me check"}}).to_string(),
            &json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "Let"}}).to_string(),
            &json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": " me check"}}).to_string(),
            "",
        ]
        .join("\n")
        .as_bytes(),
    );
    let streamed: String = emissions
        .iter()
        .filter(|e| e.stream == LiveOutputStream::Output)
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(streamed, "Let me checkLet me check");
}

#[test]
fn the_last_assistant_message_is_the_response_not_the_pre_tool_text() {
    // Pi emits one assistant message per tool-use step. The node's response must be the final
    // answer, never the narration that preceded a tool call.
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        [
            &json!({"type": "message_end", "message": {"role": "assistant", "stopReason": "toolUse",
                    "content": [{"type": "text", "text": "Let me check the file."}]}}).to_string(),
            &json!({"type": "message_end", "message": {"role": "assistant", "stopReason": "stop",
                    "content": [{"type": "text", "text": "The answer is hello."}]}}).to_string(),
            "",
        ]
        .join("\n")
        .as_bytes(),
    );
    let super::PiAttempt::Complete(result) = transcript.finish(None).unwrap() else {
        panic!("expected a complete attempt");
    };
    assert_eq!(result.message, "The answer is hello.");
}

#[test]
fn a_recovered_retry_settles_as_success() {
    // Pi auto-retries a failed model turn. A later successful message must clear the earlier
    // error, since the recovered answer is the authoritative result.
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        [
            &json!({"type": "message_end", "message": {"role": "assistant", "content": [],
                    "stopReason": "error", "errorMessage": "529 overloaded"}})
            .to_string(),
            &json!({"type": "auto_retry_start", "attempt": 1}).to_string(),
            &json!({"type": "message_end", "message": {"role": "assistant", "stopReason": "stop",
                    "content": [{"type": "text", "text": "Recovered answer"}]}})
            .to_string(),
            "",
        ]
        .join("\n")
        .as_bytes(),
    );
    let super::PiAttempt::Complete(result) = transcript.finish(None).unwrap() else {
        panic!("expected a recovered success");
    };
    assert_eq!(result.message, "Recovered answer");
}

#[test]
fn a_failure_after_the_final_answer_still_fails() {
    // The converse: a failure reported after the answer is authoritative and must fail.
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        [
            &json!({"type": "message_end", "message": {"role": "assistant", "stopReason": "stop",
                    "content": [{"type": "text", "text": "Partial answer"}]}})
            .to_string(),
            &json!({"type": "message_end", "message": {"role": "assistant", "content": [],
                    "stopReason": "aborted", "errorMessage": "aborted by user"}})
            .to_string(),
            "",
        ]
        .join("\n")
        .as_bytes(),
    );
    assert!(matches!(
        transcript.finish(None).unwrap(),
        super::PiAttempt::Failed(_)
    ));
}

#[test]
fn a_failed_stop_reason_settles_as_a_retryable_provider_failure() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        json!({
            "type": "message_end",
            "message": {"role": "assistant", "content": [], "stopReason": "error",
                        "errorMessage": "529 overloaded"}
        })
        .to_string()
        .as_bytes(),
    );
    let super::PiAttempt::Failed(failure) = transcript.finish(None).unwrap() else {
        panic!("expected a failure");
    };
    assert!(failure.retryable);
    assert!(failure.diagnostic.contains("529"));
}

#[test]
fn a_zero_exit_provider_failure_reports_the_provider_detail() {
    // Pi exits zero even when its retries are exhausted, so the process tail holds only startup
    // noise. The provider's own error must be the diagnostic, not the harmless stderr warning.
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        json!({"type": "message_end", "message": {"role": "assistant", "content": [],
                "stopReason": "error",
                "errorMessage": "500 {\"type\":\"server_error\",\"message\":\"Endpoint is unavailable.\"}"}})
        .to_string()
        .as_bytes(),
    );
    let super::PiAttempt::Failed(failure) = transcript
        .finish(Some(
            "stderr: Warning: No project session found with id 'zs-x'",
        ))
        .unwrap()
    else {
        panic!("expected a failure");
    };
    assert!(failure.diagnostic.contains("Endpoint is unavailable"));
    assert!(!failure.diagnostic.contains("No project session"));
}

#[test]
fn a_process_failure_wins_over_a_complete_message() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(assistant("done").to_string().as_bytes());
    let super::PiAttempt::Failed(failure) = transcript.finish(Some("pi exited with 1")).unwrap()
    else {
        panic!("expected a failure");
    };
    assert!(!failure.retryable);
    assert!(failure.diagnostic.contains("exited with 1"));
}

#[test]
fn no_terminal_event_settles_as_a_non_retryable_failure() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(b"{\"type\":\"agent_start\"}\n");
    let super::PiAttempt::Failed(failure) = transcript.finish(None).unwrap() else {
        panic!("expected a failure");
    };
    assert!(!failure.retryable);
}

#[test]
fn unknown_and_malformed_records_are_ignored_before_the_terminal_event() {
    let mut transcript = PiTranscript::new(Vec::new());
    let stream = [
        header("s1").as_str(),
        "not json at all",
        "{\"type\":\"some_future_event\",\"weird\":{\"nested\":true}}",
        &assistant("ok").to_string(),
        "",
    ]
    .join("\n");
    transcript.push(stream.as_bytes());
    let super::PiAttempt::Complete(result) = transcript.finish(None).unwrap() else {
        panic!("expected a complete attempt");
    };
    assert_eq!(result.message, "ok");
}

#[test]
fn a_complete_final_record_without_a_newline_is_accepted() {
    let mut transcript = PiTranscript::new(Vec::new());
    // Records are LF framed, so only the final record may lack its terminator: the process can
    // exit before writing it.
    transcript.push(format!("{}\n", header("s1")).as_bytes());
    transcript.push(assistant("tail").to_string().as_bytes());
    let super::PiAttempt::Complete(result) = transcript.finish(None).unwrap() else {
        panic!("expected a complete attempt");
    };
    assert_eq!(result.message, "tail");
    assert_eq!(result.session_id.as_deref(), Some("s1"));
}

#[test]
fn an_unterminated_trailing_fragment_is_kept_bounded_and_settles_as_a_failure() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(format!("{}\n", header("s1")).as_bytes());
    // A truncated final record is not a complete turn, so it never becomes a response.
    transcript.push(b"{\"type\":\"message_end\",\"mess");
    let super::PiAttempt::Failed(failure) = transcript.finish(None).unwrap() else {
        panic!("expected a failure");
    };
    assert!(!failure.retryable);
}

#[test]
fn usage_is_reported_and_replaced_by_the_latest_cumulative_value() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        format!(
            "{}\n",
            json!({"type": "message_update",
                   "usage": {"input": 10, "output": 2, "cacheRead": 1, "cacheWrite": 0, "totalTokens": 12},
                   "assistantMessageEvent": {"type": "start"}})
        )
        .as_bytes(),
    );
    transcript.push(
        format!(
            "{}\n",
            json!({"type": "message_update",
                   "usage": {"input": 10, "output": 7, "cacheRead": 1, "cacheWrite": 0, "totalTokens": 18},
                   "assistantMessageEvent": {"type": "done", "reason": "stop"}})
        )
        .as_bytes(),
    );
    let usage = transcript.token_usage().unwrap();
    assert_eq!(usage.input_tokens.get(), 10);
    assert_eq!(usage.output_tokens.get(), 7);
    assert_eq!(
        usage.cache_read_input_tokens.map(|count| count.get()),
        Some(1)
    );
}

#[test]
fn credentials_are_redacted_from_diagnostics() {
    let mut transcript = PiTranscript::new(vec!["sk-secret-value".to_owned()]);
    transcript.push(
        json!({"type": "message_end",
               "message": {"role": "assistant", "content": [], "stopReason": "error",
                           "errorMessage": "rejected sk-secret-value"}})
        .to_string()
        .as_bytes(),
    );
    let super::PiAttempt::Failed(failure) = transcript.finish(None).unwrap() else {
        panic!("expected a failure");
    };
    assert!(!failure.diagnostic.contains("sk-secret-value"));
}

#[test]
fn assistant_text_handles_string_and_block_content() {
    assert_eq!(
        assistant_text(&json!({"content": "plain"})).as_deref(),
        Some("plain")
    );
    assert_eq!(
        assistant_text(&json!({"content": [
            {"type": "thinking", "text": "ignored"},
            {"type": "text", "text": "a"},
            {"type": "text", "text": "b"}
        ]}))
        .as_deref(),
        Some("ab")
    );
    assert_eq!(assistant_text(&json!({"content": []})), None);
}

/// The largest token count the wire contract admits. Two of these no longer fit, which is how the
/// transcript's fail-closed accumulation is exercised.
const SAFE_TOKEN_COUNT: u64 = 9_007_199_254_740_991;

fn assistant_message_end(stop_reason: &str, usage: serde_json::Value) -> String {
    json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [],
            "stopReason": stop_reason,
            "usage": usage,
        }
    })
    .to_string()
}

#[test]
fn assistant_text_requires_content_of_a_supported_shape() {
    // A message with no content at all, a content value that is neither a string nor an array of
    // blocks, and a text block that carries no text are all unusable as an answer.
    assert_eq!(assistant_text(&json!({"role": "assistant"})), None);
    assert_eq!(assistant_text(&json!({"content": 5})), None);
    assert_eq!(
        assistant_text(&json!({"content": [{"type": "text"}]})),
        None
    );
    assert_eq!(
        assistant_text(&json!({"content": [
            {"type": "text", "text": "ok"},
            {"type": "text"}
        ]}))
        .as_deref(),
        Some("ok")
    );
}

#[test]
fn ignored_record_shapes_do_not_disturb_the_authoritative_answer() {
    // Record shapes a future Pi version may add, plus empty and non-assistant records, must all be
    // ignored: none may set the session identity, emit a hidden fragment, or invent a usage total.
    let mut transcript = PiTranscript::new(Vec::new());
    let stream = [
        "",
        "   ",
        r#"{"other":true}"#,
        r#"{"type":"session"}"#,
        r#"{"type":"message_start","message":{"role":"user"}}"#,
        r#"{"type":"message_end"}"#,
        r#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#,
        r#"{"type":"message_update"}"#,
        r#"{"type":"message_update","assistantMessageEvent":{}}"#,
        r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":""}}"#,
        &json!({"type": "compaction_start"}).to_string(),
        &json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "settled"}],
                "stopReason": "stop"
            }
        })
        .to_string(),
        "",
    ]
    .join("\n");
    let emissions = transcript.push(stream.as_bytes());
    let emitted: Vec<&str> = emissions.iter().map(|e| e.text.as_str()).collect();
    // Only the compaction notice survives: empty deltas and a text block without text do not.
    assert_eq!(emitted, ["Pi compacted the session context"]);
    assert!(transcript.token_usage().is_none());
    let super::PiAttempt::Complete(result) = transcript.finish(None).unwrap() else {
        panic!("expected the settled answer");
    };
    assert_eq!(result.message, "settled");
    // A session record that carried no identifier must not invent one.
    assert_eq!(result.session_id, None);
}

#[test]
fn usage_that_is_not_a_complete_counted_object_is_ignored() {
    // A usage value that is not an object, one missing a required counter, and one whose counter
    // exceeds the wire bound must all leave the turn with no usage total rather than zero usage.
    let mut transcript = PiTranscript::new(Vec::new());
    let stream = [
        json!({"type": "message_update", "usage": "not an object",
               "assistantMessageEvent": {"type": "start"}})
        .to_string(),
        json!({"type": "message_update", "usage": {"input": 5},
               "assistantMessageEvent": {"type": "start"}})
        .to_string(),
        json!({"type": "message_update", "usage": {"input": SAFE_TOKEN_COUNT + 1, "output": 3},
               "assistantMessageEvent": {"type": "start"}})
        .to_string(),
        String::new(),
    ]
    .join("\n");
    transcript.push(stream.as_bytes());
    assert!(transcript.token_usage().is_none());
}

#[test]
fn usage_that_overflows_the_safe_range_keeps_the_committed_total() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        format!(
            "{}\n",
            assistant_message_end("toolUse", json!({"input": SAFE_TOKEN_COUNT, "output": 1}))
        )
        .as_bytes(),
    );
    transcript.push(
        format!(
            "{}\n",
            json!({"type": "message_update", "usage": {"input": SAFE_TOKEN_COUNT, "output": 1},
                   "assistantMessageEvent": {"type": "start"}})
        )
        .as_bytes(),
    );
    // The in-flight message cannot be added to the committed total, so the total stands.
    let usage = transcript.token_usage().expect("committed usage survives");
    assert_eq!(usage.input_tokens.get(), SAFE_TOKEN_COUNT);
    transcript.push(
        format!(
            "{}\n",
            json!({"type": "message_start", "message": {"role": "assistant"}})
        )
        .as_bytes(),
    );
    // Committing the overflowing message must also keep the earlier total.
    let usage = transcript
        .token_usage()
        .expect("committed usage survives the commit");
    assert_eq!(usage.input_tokens.get(), SAFE_TOKEN_COUNT);
}

#[test]
fn overflowing_cache_counters_keep_the_committed_total() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        format!(
            "{}\n",
            assistant_message_end(
                "toolUse",
                json!({"input": 1, "output": 1, "cacheRead": SAFE_TOKEN_COUNT,
                       "cacheWrite": SAFE_TOKEN_COUNT})
            )
        )
        .as_bytes(),
    );
    transcript.push(
        format!(
            "{}\n",
            json!({"type": "message_update",
                   "usage": {"input": 1, "output": 1, "cacheRead": SAFE_TOKEN_COUNT,
                             "cacheWrite": SAFE_TOKEN_COUNT},
                   "assistantMessageEvent": {"type": "start"}})
        )
        .as_bytes(),
    );
    let usage = transcript.token_usage().expect("committed usage survives");
    assert_eq!(usage.input_tokens.get(), 1);
    assert_eq!(
        usage.cache_read_input_tokens.map(|count| count.get()),
        Some(SAFE_TOKEN_COUNT)
    );
}

#[test]
fn cache_counters_accumulate_when_every_message_reports_them() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        format!(
            "{}\n",
            assistant_message_end(
                "toolUse",
                json!({"input": 2, "output": 3, "cacheRead": 5, "cacheWrite": 7})
            )
        )
        .as_bytes(),
    );
    transcript.push(
        format!(
            "{}\n",
            json!({"type": "message_update",
                   "usage": {"input": 11, "output": 13, "cacheRead": 17, "cacheWrite": 19},
                   "assistantMessageEvent": {"type": "start"}})
        )
        .as_bytes(),
    );
    let usage = transcript.token_usage().expect("committed usage");
    assert_eq!(usage.input_tokens.get(), 13);
    assert_eq!(usage.output_tokens.get(), 16);
    assert_eq!(
        usage.cache_read_input_tokens.map(|count| count.get()),
        Some(22)
    );
    assert_eq!(
        usage.cache_creation_input_tokens.map(|count| count.get()),
        Some(26)
    );
}

#[test]
fn one_oversized_record_is_dropped_without_losing_the_answer() {
    // The framing guard retains at most one unfinished record. A record past its bound is dropped
    // whole, and the following valid record must still settle the turn.
    let mut transcript = PiTranscript::new(Vec::new());
    let mut oversized = vec![
        b'x';
        crate::native_v2_capsule::provider_json_lines::MAX_PROVIDER_JSONL_RECORD_BYTES
            + 1
    ];
    oversized.push(b'\n');
    transcript.push(&oversized);
    transcript.push(format!("{}\n", assistant("after the drop")).as_bytes());
    let super::PiAttempt::Complete(result) = transcript.finish(None).unwrap() else {
        panic!("expected the surviving answer");
    };
    assert_eq!(result.message, "after the drop");
}

#[test]
fn redactions_are_deduplicated_and_applied_longest_first() {
    // Sorting matters: a short redaction applied first would leave the prefix of a longer one in
    // the diagnostic, so the longer value has to win.
    let mut transcript = PiTranscript::new(vec![
        "secret".to_owned(),
        String::new(),
        "sk-secret".to_owned(),
        "secret".to_owned(),
    ]);
    transcript.push(
        json!({"type": "message_end",
               "message": {"role": "assistant", "content": [], "stopReason": "error",
                           "errorMessage": "sk-secret and secret"}})
        .to_string()
        .as_bytes(),
    );
    let super::PiAttempt::Failed(failure) = transcript.finish(None).unwrap() else {
        panic!("expected a failure");
    };
    assert!(
        !failure.diagnostic.contains("sk-"),
        "{}",
        failure.diagnostic
    );
    assert!(
        !failure.diagnostic.contains("secret"),
        "{}",
        failure.diagnostic
    );
}
