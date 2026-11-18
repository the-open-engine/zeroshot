use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

#[path = "tests/unicode.rs"]
mod unicode;

#[test]
fn retry_progress_includes_stream_error_message_and_preserves_failure() {
    for (extra, expected) in [
        (
            json!({"message":"connection reset"}),
            "Codex stream error: connection reset",
        ),
        (
            json!({"error":{"message":"stream cut"}}),
            "Codex stream error: stream cut",
        ),
        (json!({}), "Codex stream error"),
        (json!({"message":null}), "Codex stream error"),
        (json!({"message":42}), "Codex stream error"),
        (json!({"message":"  "}), "Codex stream error"),
    ] {
        let mut event = extra;
        event["type"] = json!("error");
        let bytes = format!("{event}\n");
        let mut decoder = CodexOutputDecoder::new();
        let emissions = decoder.push(bytes.as_bytes());
        assert_eq!(emissions.len(), 1);
        let mut logs = Vec::new();
        emissions[0].emit(&ProviderText::new(&[]), &mut logs);
        assert_eq!(
            (logs[0].stream, logs[0].text.as_str()),
            (LiveOutputStream::System, expected)
        );
        let output = decoder.finish();
        assert!(output.final_message().is_err());
        assert!(
            output.failure_message().assert_value().contains(
                expected
                    .strip_prefix("Codex stream error: ")
                    .unwrap_or("Codex stream error without provider detail")
            )
        );
    }
}

#[test]
fn retry_progress_redacts_before_bounding_without_shortening_final_failure() {
    let secret = "private-provider-token";
    for message in [
        format!("connection reset {secret}"),
        format!("{} {secret} {} {secret}", "é".repeat(235), "界".repeat(600)),
    ] {
        let mut decoder = CodexOutputDecoder::new();
        let event = json!({"type":"error", "message":message});
        let emissions = decoder.push(format!("{event}\n").as_bytes());
        assert_eq!(emissions.len(), 1);
        let mut logs = Vec::new();
        emissions[0].emit(&ProviderText::new(&[secret.to_owned()]), &mut logs);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].stream, LiveOutputStream::System);
        let line = &logs[0].text;
        assert!(
            line.len() <= crate::native_v2_capsule::provider_process::text::LIVE_LINE_BYTES,
            "{} bytes",
            line.len()
        );
        assert!(line.starts_with("Codex stream error: "));
        assert!(line.contains("[REDACTED]"));
        assert!(!line.contains("private"));
        assert_eq!(decoder.finish().failure_message(), Some(message.as_str()));
    }
}

fn output_with_provisional_usage(event_type: &str, usage: Option<Value>) -> CodexOutput {
    let mut terminal = json!({"type":event_type});
    if event_type == "turn.failed" {
        terminal
            .as_object_mut()
            .assert_value()
            .insert("error".to_owned(), json!({"message":"terminal failure"}));
    }
    if let Some(usage) = usage {
        terminal
            .as_object_mut()
            .assert_value()
            .insert("usage".to_owned(), usage);
    }
    let mut bytes = Vec::new();
    for event in [
        json!({
            "type":"error",
            "message":"temporary",
            "usage":{
                "input_tokens":11,
                "output_tokens":7,
                "cached_input_tokens":5,
                "cache_write_input_tokens":3
            }
        }),
        json!({"type":"item.completed","item":{"type":"agent_message","text":"done"}}),
        terminal,
    ] {
        serde_json::to_writer(&mut bytes, &event).assert_value();
        bytes.push(b'\n');
    }
    CodexOutput::parse(&bytes)
}

fn assert_usage(output: &CodexOutput, expected: [u64; 4]) {
    let usage = output.token_usage().assert_value();
    assert_eq!(
        [
            usage.input_tokens.get(),
            usage.output_tokens.get(),
            usage.cache_read_input_tokens.assert_value().get(),
            usage.cache_creation_input_tokens.assert_value().get(),
        ],
        expected
    );
}

#[test]
fn normalizes_worker_and_verifier_messages() {
    let worker = CodexOutput::parse(
        concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"thread-1\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",",
            "\"text\":\"{\\\"answer\\\":42}\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,",
            "\"cached_input_tokens\":1,\"cache_write_input_tokens\":3,",
            "\"output_tokens\":2}}\n",
        )
        .as_bytes(),
    );
    assert_eq!(worker.final_message().assert_value(), r#"{"answer":42}"#);
    let usage = worker.token_usage().assert_value();
    assert_eq!(usage.input_tokens.get(), 1);
    assert_eq!(usage.output_tokens.get(), 2);
    assert_eq!(usage.cache_read_input_tokens.assert_value().get(), 1);
    assert_eq!(usage.cache_creation_input_tokens.assert_value().get(), 3);

    let verifier_events = concat!(
        "{\"type\":\"thread.started\",\"thread_id\":\"thread-2\"}\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",",
        "\"text\":\"{\\\"output\\\":{\\\"ok\\\":true},",
        "\\\"signals\\\":{\\\"decision\\\":\\\"pass\\\"},",
        "\\\"diagnostic\\\":null}\"}}\n",
        "{\"type\":\"turn.completed\"}\n",
    );
    let verifier = CodexOutput::parse(verifier_events.as_bytes());
    assert_eq!(
        serde_json::from_str::<Value>(verifier.final_message().assert_value()).assert_value(),
        json!({
            "output": { "ok": true },
            "signals": { "decision": "pass" },
            "diagnostic": null
        })
    );
}

#[test]
fn malformed_terminal_usage_is_unavailable_without_rejecting_the_turn() {
    let output = CodexOutput::parse(
        br#"{"type":"error","message":"temporary","usage":{"input_tokens":11,"output_tokens":7}}
{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
{"type":"turn.completed","usage":{"input_tokens":"many","output_tokens":2}}
"#,
    );
    assert!(output.token_usage().is_none());
}

#[test]
fn empty_terminal_usage_preserves_provisional_usage_for_every_terminal_outcome() {
    for event_type in ["turn.completed", "turn.failed"] {
        for usage in [None, Some(Value::Null), Some(json!({}))] {
            assert_usage(
                &output_with_provisional_usage(event_type, usage),
                [11, 7, 5, 3],
            );
        }
    }
}

#[test]
fn valid_terminal_usage_replaces_provisional_usage_for_every_terminal_outcome() {
    for event_type in ["turn.completed", "turn.failed"] {
        let output = output_with_provisional_usage(
            event_type,
            Some(json!({
                "input_tokens":19,
                "output_tokens":13,
                "cached_input_tokens":8,
                "cache_write_input_tokens":2
            })),
        );
        assert_usage(&output, [19, 13, 8, 2]);
    }
}

#[test]
fn malformed_terminal_usage_invalidates_provisional_usage_for_every_terminal_outcome() {
    for event_type in ["turn.completed", "turn.failed"] {
        for usage in [
            json!("invalid"),
            json!({"input_tokens":"many","output_tokens":2}),
            json!({"input_tokens":1}),
        ] {
            assert!(
                output_with_provisional_usage(event_type, Some(usage))
                    .token_usage()
                    .is_none()
            );
        }
    }
}

#[test]
fn reports_missing_terminal_or_final_message_with_actionable_detail() {
    assert_eq!(
        CodexOutput::parse(br#"{"type":"thread.started","thread_id":"thread-1"}"#)
            .failure_message(),
        Some("Codex output ended without a terminal turn event")
    );
    assert_eq!(
        CodexOutput::parse(br#"{"type":"turn.completed"}"#).failure_message(),
        Some("Codex turn completed without a final agent message")
    );
}

#[test]
fn retains_terminal_failure_detail_for_the_retry_policy() {
    let failed = CodexOutput::parse(
        concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"thread-1\"}\n",
            "{\"type\":\"error\",\"message\":\"temporary\",\"usage\":{",
            "\"input_tokens\":21,\"output_tokens\":13}}\n",
            "{\"type\":\"turn.failed\",\"usage\":{\"input_tokens\":8,",
            "\"cached_input_tokens\":5,\"cache_write_input_tokens\":2,",
            "\"output_tokens\":3},\"error\":{\"message\":\"service unavailable\"}}\n",
        )
        .as_bytes(),
    );

    assert_eq!(failed.thread_id.as_deref(), Some("thread-1"));
    assert_eq!(failed.failure_message(), Some("service unavailable"));
    let usage = failed.token_usage().assert_value();
    assert_eq!(usage.cache_read_input_tokens.assert_value().get(), 5);
    assert_eq!(usage.cache_creation_input_tokens.assert_value().get(), 2);
}

#[test]
fn retains_error_usage_provisionally_at_eof() {
    let output = CodexOutput::parse(
        concat!(
            "{\"type\":\"error\",\"message\":\"connection ended\",\"usage\":{",
            "\"input_tokens\":19,\"cached_input_tokens\":7,",
            "\"cache_write_input_tokens\":3,\"output_tokens\":5}}",
        )
        .as_bytes(),
    );

    assert_eq!(output.failure_message(), Some("connection ended"));
    let usage = output.token_usage().assert_value();
    assert_eq!(usage.input_tokens.get(), 19);
    assert_eq!(usage.output_tokens.get(), 5);
    assert_eq!(usage.cache_read_input_tokens.assert_value().get(), 7);
    assert_eq!(usage.cache_creation_input_tokens.assert_value().get(), 3);
}

#[test]
fn provisional_error_is_cleared_by_completion_and_latest_message_wins() {
    let output = CodexOutput::parse(
        br#"{"type":"error","message":"temporary transport retry","usage":{"input_tokens":31,"output_tokens":17}}
{"type":"item.completed","item":{"type":"agent_message","text":"old"}}
{"type":"item.completed","item":{"type":"agent_message","text":"final"}}
{"type":"turn.completed","usage":{"input_tokens":3,"output_tokens":4}}
{"type":"item.completed","item":{"type":"agent_message","text":"trailing"}}
{"type":"turn.failed","error":{"message":"trailing failure"}}
"#,
    );

    assert_eq!(output.final_message().assert_value(), "final");
    assert_eq!(output.failure_message(), None);
    let usage = output.token_usage().assert_value();
    assert_eq!(usage.input_tokens.get(), 3);
    assert_eq!(usage.output_tokens.get(), 4);
}

#[test]
fn provisional_error_survives_eof_and_terminal_failure_prefers_its_detail() {
    let ended = CodexOutput::parse(br#"{"type":"error","message":"temporary transport retry"}"#);
    assert_eq!(ended.failure_message(), Some("temporary transport retry"));

    let failed = CodexOutput::parse(
        br#"{"type":"error","message":"temporary transport retry"}
{"type":"turn.failed","error":"terminal service failure"}
{"type":"turn.completed"}
"#,
    );
    assert_eq!(failed.failure_message(), Some("terminal service failure"));
}

#[test]
fn malformed_ancillary_records_do_not_discard_a_valid_result() {
    let mut bytes = b"not json\n".to_vec();
    bytes.extend_from_slice(&[0xff, b'\n']);
    bytes.extend_from_slice(
        br#"{}
{"type":"thread.started"}
{"type":"item.completed"}
{"type":"future.event","payload":true}
{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
{"type":"turn.completed"}"#,
    );
    let output = CodexOutput::parse(&bytes);

    assert_eq!(output.final_message().assert_value(), "done");
    assert_eq!(output.failure_message(), None);
    assert_eq!(output.malformed_records, 5);
    assert_eq!(output.oversized_records, 0);
}

#[test]
fn malformed_records_are_reported_when_required_output_is_missing() {
    let output = CodexOutput::parse(b"not json\n{\"type\":\"turn.completed\"}\n");
    assert_eq!(
        output.failure_message(),
        Some(
            "Codex turn completed without a final agent message; ignored 1 malformed and 0 oversized output records"
        )
    );
}

#[test]
fn conflicting_thread_ids_wait_for_completion_and_preserve_terminal_evidence() {
    let output = CodexOutput::parse(
        br#"{"type":"thread.started","thread_id":"thread-1"}
{"type":"thread.started","thread_id":"thread-2"}
{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
{"type":"turn.completed","usage":{"input_tokens":8,"output_tokens":3}}
{"type":"item.completed","item":{"type":"agent_message","text":"trailing"}}
{"type":"turn.failed","usage":{"input_tokens":999,"output_tokens":999},"error":{"message":"trailing failure"}}
"#,
    );

    assert_eq!(output.thread_id.as_deref(), Some("thread-1"));
    assert_eq!(
        output.failure_message(),
        Some("Codex output contained conflicting thread IDs")
    );
    assert_eq!(output.final_message().assert_value(), "done");
    let usage = output.token_usage().assert_value();
    assert_eq!(usage.input_tokens.get(), 8);
    assert_eq!(usage.output_tokens.get(), 3);
}

#[test]
fn conflicting_thread_ids_preserve_failed_turn_usage_but_win_the_outcome() {
    let output = CodexOutput::parse(
        br#"{"type":"thread.started","thread_id":"thread-1"}
{"type":"thread.started","thread_id":"thread-2"}
{"type":"turn.failed","usage":{"input_tokens":13,"output_tokens":5},"error":{"message":"provider terminal detail"}}
"#,
    );

    assert_eq!(output.thread_id.as_deref(), Some("thread-1"));
    assert_eq!(
        output.failure_message(),
        Some("Codex output contained conflicting thread IDs; provider terminal detail")
    );
    let usage = output.token_usage().assert_value();
    assert_eq!(usage.input_tokens.get(), 13);
    assert_eq!(usage.output_tokens.get(), 5);
}

#[test]
fn decoder_handles_split_crlf_and_an_unterminated_terminal_record() {
    let mut decoder = CodexOutputDecoder::new();
    assert!(
        decoder
            .push(b"{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_")
            .is_empty()
    );
    let emissions = decoder.push(b"message\",\"text\":\"done\"}}\r\n{\"type\":\"turn.completed\"}");
    assert_eq!(emissions.len(), 1);
    let output = decoder.finish();
    assert_eq!(output.final_message().assert_value(), "done");
    assert_eq!(output.failure_message(), None);
}

#[test]
fn projects_provider_items_to_semantic_attach_messages() {
    let mut decoder = CodexOutputDecoder::new();
    let emissions = decoder.push(
        concat!(
            "{\"type\":\"item.updated\",\"item\":{\"type\":\"reasoning\",\"text\":\"checking tests\"}}\n",
            "{\"type\":\"item.started\",\"item\":{\"type\":\"reasoning\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",",
            "\"command\":\"cargo test\",\"aggregated_output\":\"ok\",",
            "\"exit_code\":0,\"status\":\"completed\"}}\n",
            "{\"type\":\"item.started\",\"item\":{\"type\":\"command_execution\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"file_change\",",
            "\"changes\":[{\"path\":\"src/lib.rs\",\"kind\":\"update\"}],",
            "\"status\":\"completed\"}}\n",
            "{\"type\":\"item.updated\",\"item\":{\"type\":\"file_change\",",
            "\"changes\":[{\"path\":7}]}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"mcp_tool_call\",",
            "\"server\":\"github\",\"tool\":\"get_pr\",",
            "\"result\":{\"number\":7},\"status\":\"completed\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"mcp_tool_call\",",
            "\"server\":\"github\",\"tool\":\"merge_pr\",",
            "\"error\":{\"message\":\"policy blocked\"}}}\n",
            "{\"type\":\"item.started\",\"item\":{\"type\":\"web_search\",",
            "\"query\":\"Rust coverage\"}}\n",
            "{\"type\":\"item.updated\",\"item\":{\"type\":\"web_search\"}}\n",
            "{\"type\":\"item.updated\",\"item\":{\"type\":\"todo_list\",",
            "\"items\":[{\"text\":\"inspect\",\"completed\":true},",
            "{\"text\":\"test\",\"completed\":false},{\"text\":7}]}}\n",
            "{\"type\":\"item.started\",\"item\":{\"type\":\"todo_list\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"error\"}}\n",
            "{\"type\":\"item.updated\",\"item\":{\"type\":\"future_activity\"}}\n",
        )
        .as_bytes(),
    );
    let mut logs = Vec::new();
    for emission in emissions {
        emission.emit(&ProviderText::new(&[]), &mut logs);
    }
    let messages = logs.iter().map(|log| log.text.as_str()).collect::<Vec<_>>();
    for expected in [
        "Codex reasoning updated: checking tests",
        "Codex reasoning started",
        "Codex plan updated: done: inspect, pending: test",
        "Codex plan started: empty",
        "Codex activity error: unknown error",
        "Codex activity updated: future_activity",
    ] {
        assert!(messages.contains(&expected), "{messages:?}");
    }
    let text = messages.join("\n");
    for expected in [
        "command_execution",
        "cargo test",
        "exit=0",
        "src/lib.rs",
        "update",
        "github.get_pr",
        "number",
        "github.merge_pr",
        "policy blocked",
        "Rust coverage",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(!text.contains("unknown changes"));
    assert!(!text.contains("unknown query"));
}

#[test]
fn settled_and_failed_decoders_ignore_trailing_data_and_keep_actionable_detail() {
    let mut settled = CodexOutputDecoder::new();
    settled.push(
        br#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
{"type":"turn.completed"}
"#,
    );
    assert!(
        settled
            .push(br#"{"type":"turn.failed","error":{"message":"late"}}"#)
            .is_empty()
    );
    assert_eq!(settled.finish().failure_message(), None);

    let mut failed = CodexOutputDecoder::new();
    assert!(failed.accept_line(ProviderJsonLine::Oversized).is_empty());
    assert!(failed.push(b" \n").is_empty());
    assert!(
        failed
            .push(
                br#"{"type":"item.started","item":{"type":"agent_message"}}
"#
            )
            .is_empty()
    );
    assert_eq!(
        failed
            .push(
                br#"{"type":"turn.failed"}
"#
            )
            .len(),
        1
    );
    let failed = failed.finish();
    assert_eq!(
        failed.failure_message(),
        Some(
            "Codex turn failed without provider detail; ignored 0 malformed and 1 oversized output records"
        )
    );

    let nested = CodexOutput::parse(br#"{"type":"error","error":{"message":"nested failure"}}"#);
    assert_eq!(nested.failure_message(), Some("nested failure"));

    let mut merged = CodexOutput::provider_failure(" first ".to_owned());
    merged.merge_failure_detail("".to_owned());
    merged.merge_failure_detail("first".to_owned());
    merged.merge_failure_detail("second".to_owned());
    assert_eq!(merged.failure_message(), Some("first; second"));
}

#[test]
fn tool_activity_keeps_identity_and_omits_cumulative_updates() {
    let mut decoder = CodexOutputDecoder::new();
    let mut logs = Vec::new();
    for (phase, status, output) in [
        ("started", "in_progress", ""),
        ("updated", "in_progress", "partial-output"),
        ("completed", "completed", "tool-result"),
    ] {
        let event = json!({"type":format!("item.{phase}"),"item":{"id":"tool-1","type":"command_execution","command":"check-tool","status":status,"aggregated_output":output,"exit_code":0}});
        for emission in decoder.push(format!("{event}\n").as_bytes()) {
            emission.emit(&ProviderText::new(&[]), &mut logs);
        }
    }
    assert_eq!(logs.len(), 2, "one call and one completion");
    let text = logs
        .iter()
        .map(|log| log.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in ["tool-1", "check-tool", "tool-result"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("partial-output"));
}

#[test]
fn tool_activity_flushes_an_unterminated_completion_without_inventing_status() {
    let event = json!({"type":"item.completed","item":{"id":"tool-eof",
        "type":"command_execution","command":"check-eof","aggregated_output":"eof-result"}});
    let mut decoder = CodexOutputDecoder::new();
    assert!(decoder.push(event.to_string().as_bytes()).is_empty());
    let mut logs = Vec::new();
    for emission in decoder.finish_stream() {
        emission.emit(&ProviderText::new(&[]), &mut logs);
    }
    assert_eq!(logs.len(), 2);
    assert!(logs[0].text.contains("check-eof"));
    assert!(logs[1].text.contains("id=tool-eof status=unknown"));
    assert!(logs[1].text.contains("eof-result"));
    assert!(decoder.finish_stream().is_empty());
}

#[test]
fn tool_activity_preserves_collab_calls_and_agent_results_without_duplicates() {
    for tool in ["spawn_agent", "send_input", "wait", "close_agent"] {
        for status in [Some("completed"), Some("failed"), None] {
            let mut item = json!({"id":"collab-call", "type":"collab_tool_call", "tool":tool,
                "sender_thread_id":"parent-thread", "receiver_thread_ids":["child-thread"],
                "prompt":"inspect-critical.rs", "agents_states":{
                    "child-thread":{"status":"completed", "message":"child-result"}}});
            if let Some(status) = status {
                item["status"] = json!(status);
            }
            let mut decoder = CodexOutputDecoder::new();
            let mut logs = Vec::new();
            for phase in ["started", "started", "updated", "completed", "completed"] {
                let mut snapshot = item.clone();
                if phase != "completed" {
                    snapshot["status"] = json!("in_progress");
                    snapshot["agents_states"] = json!({});
                }
                if phase == "updated" {
                    snapshot["prompt"] = json!("ignored-cumulative");
                }
                let event = json!({"type":format!("item.{phase}"), "item":snapshot});
                for emission in decoder.push(format!("{event}\n").as_bytes()) {
                    emission.emit(&ProviderText::new(&[]), &mut logs);
                }
            }
            assert_eq!(logs.len(), 2, "{tool}: one call and one completion");
            assert!(
                logs[0]
                    .text
                    .starts_with(&format!("Codex tool call: {tool} id=collab-call "))
            );
            for expected in ["parent-thread", "child-thread", "inspect-critical.rs"] {
                assert!(
                    logs[0].text.contains(expected),
                    "{tool}: missing {expected}"
                );
            }
            assert!(logs[1].text.starts_with(&format!(
                "Codex tool completed: {tool} id=collab-call status={}\n",
                status.unwrap_or("unknown")
            )));
            for expected in ["child-thread", "child-result", "\"status\":\"completed\""] {
                assert!(
                    logs[1].text.contains(expected),
                    "{tool}: missing {expected}"
                );
            }
            assert!(
                logs.iter()
                    .all(|log| !log.text.contains("ignored-cumulative"))
            );
        }
    }
}

#[test]
fn tool_activity_preserves_web_actions_with_query_fallback() {
    for action in [
        Some(json!({"type":"search", "queries":["first-query", "second-query"]})),
        Some(json!({"type":"open_page", "url":"https://example.test/page"})),
        Some(
            json!({"type":"find_in_page", "url":"https://example.test/page", "pattern":"full-pattern"}),
        ),
        Some(json!({"type":"other"})),
        None,
        Some(Value::Null),
    ] {
        let mut item = json!({"id":"web-call", "type":"web_search", "query":"summary-query ..."});
        if let Some(action) = &action {
            item["action"] = action.clone();
        }
        let mut decoder = CodexOutputDecoder::new();
        let mut logs = Vec::new();
        // Completion-only snapshots must retain the full action in both entries too.
        for _ in 0..2 {
            let event = json!({"type":"item.completed", "item":item});
            for emission in decoder.push(format!("{event}\n").as_bytes()) {
                emission.emit(&ProviderText::new(&[]), &mut logs);
            }
        }
        assert_eq!(logs.len(), 2);
        for log in logs {
            assert!(log.text.contains("id=web-call status=unknown"));
            let payload: Value =
                serde_json::from_str(log.text.split_once('\n').assert_value().1).assert_value();
            assert_eq!(payload["query"], "summary-query ...");
            assert_eq!(payload["action"], action.clone().unwrap_or(Value::Null));
        }
    }
}

#[test]
fn tool_activity_bounds_inputs_and_results_for_all_codex_tool_items() {
    let secret = "credential-\"\n界";
    for kind in [
        "command_execution",
        "file_change",
        "mcp_tool_call",
        "collab_tool_call",
        "web_search",
    ] {
        for status in ["completed", "failed"] {
            let payload = format!("HEAD {secret} {} {secret} TAIL", "é界".repeat(9000));
            let item = json!({"id":"native-item", "type":kind, "command":payload,
                "changes":[{"kind":"update","path":payload}], "query":payload,
                "action":{"type":"search", "queries":[payload]},
                "sender_thread_id":"parent-thread", "receiver_thread_ids":["child-thread"],
                "prompt":payload, "agents_states":{
                    "child-thread":{"status":"completed", "message":payload}},
                "server":"mcp", "tool":"lookup", "arguments":{"query":payload},
                "aggregated_output":payload, "result":{"content":payload},
                "error":{"message":payload}, "status":status, "exit_code":1});
            let mut decoder = CodexOutputDecoder::new();
            let mut logs = Vec::new();
            for phase in ["started", "started", "updated", "completed", "completed"] {
                let event = json!({"type":format!("item.{phase}"),"item":item});
                for emission in decoder.push(format!("{event}\n").as_bytes()) {
                    emission.emit(&ProviderText::new(&[secret.into()]), &mut logs);
                }
            }
            assert_eq!(logs.len(), 2);
            for log in logs {
                assert!(log.text.len() <= 8192);
                for expected in ["native-item", "HEAD", "TAIL", "[truncated]", "[REDACTED]"] {
                    assert!(log.text.contains(expected), "{kind}: {expected}");
                }
                assert!(!log.text.contains("credential-"));
            }
        }
    }
}
