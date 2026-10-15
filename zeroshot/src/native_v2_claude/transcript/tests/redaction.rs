use super::*;

const VALUE: &str = "sk-test-0123456789abcdef";

fn redacting() -> ClaudeTranscript {
    ClaudeTranscript::new(vec![VALUE.to_owned()])
}

fn text(text: &str) -> Value {
    delta("text_delta", "text", text)
}

fn thinking(text: &str) -> Value {
    delta("thinking_delta", "thinking", text)
}

fn stream_event(event: Value) -> Value {
    json!({"type":"stream_event","event":event})
}

/// Pushes each event as its own chunk and returns every emission in order.
fn push_events(transcript: &mut ClaudeTranscript, events: Vec<Value>) -> Vec<ClaudeEmission> {
    let mut emissions = Vec::new();
    for event in events {
        let mut bytes = Vec::new();
        append_event(&mut bytes, event);
        emissions.extend(transcript.push(&bytes));
    }
    emissions
}

#[test]
fn a_value_split_between_thinking_and_text_is_redacted_with_the_block_where_it_ends() {
    let emissions = push_events(
        &mut redacting(),
        vec![
            thinking("token sk-test-01234"),
            block_stop(),
            text("56789abcdef ok"),
            block_stop(),
        ],
    );
    assert_eq!(
        emitted(&emissions),
        [
            (LiveOutputStream::System, "token "),
            (LiveOutputStream::Output, "[REDACTED] ok"),
        ]
    );

    let emissions = push_events(
        &mut redacting(),
        vec![
            text("token sk-test-01234"),
            thinking("56789abcdef ok"),
            block_stop(),
        ],
    );
    assert_eq!(
        emitted(&emissions),
        [
            (LiveOutputStream::Output, "token "),
            (LiveOutputStream::System, "[REDACTED] ok"),
        ]
    );
}

#[test]
fn a_value_split_across_streamed_messages_is_redacted() {
    let mut events = Vec::new();
    for (message, part) in [
        ("msg-1", "token sk-test-"),
        ("msg-2", "0123456789abcdef ok"),
    ] {
        events.extend([
            stream_event(json!({"type":"message_start","message":{"id":message}})),
            text(part),
            block_stop(),
            stream_event(json!({"type":"message_stop"})),
        ]);
    }

    assert_eq!(
        emitted(&push_events(&mut redacting(), events)),
        [
            (LiveOutputStream::Output, "token "),
            (LiveOutputStream::Output, "[REDACTED] ok"),
        ]
    );
}

#[test]
fn a_record_between_deltas_does_not_publish_a_value_prefix() {
    let emissions = push_events(
        &mut redacting(),
        vec![
            text("token sk-test-01234"),
            json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}),
            text("56789abcdef ok"),
            block_stop(),
        ],
    );
    assert_eq!(
        emitted(&emissions),
        [
            (
                LiveOutputStream::System,
                "Claude rate limit: status=allowed"
            ),
            (LiveOutputStream::Output, "token "),
            (LiveOutputStream::Output, "[REDACTED] ok"),
        ]
    );
}

#[test]
fn a_value_split_across_assistant_fallback_blocks_is_redacted() {
    let emissions = push_events(
        &mut redacting(),
        vec![
            json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"token sk-test-"},
                {"type":"text","text":"01234"},
                {"type":"text","text":"56789abcdef ok"}
            ]}}),
            success(json!("done"), "session-1"),
        ],
    );
    assert_eq!(
        emitted(&emissions),
        [
            (LiveOutputStream::Output, "token "),
            (LiveOutputStream::Output, "[REDACTED] ok"),
        ]
    );
}

#[test]
fn only_the_last_line_of_a_block_that_could_begin_a_value_waits_for_more_text() {
    let mut transcript = redacting();
    let emissions = push_events(
        &mut transcript,
        vec![
            text("Done.\nLet me check the files"),
            block_stop(),
            stream_event(json!({"type":"message_stop"})),
        ],
    );
    assert_eq!(emitted(&emissions), [(LiveOutputStream::Output, "Done.\n")]);

    let emissions = push_events(&mut transcript, vec![text("Found them.\n"), block_stop()]);
    assert_eq!(
        emitted(&emissions),
        [
            (LiveOutputStream::Output, "Let me check the files"),
            (LiveOutputStream::Output, "Found them.\n"),
        ]
    );
}

#[test]
fn held_text_is_published_when_the_turn_ends() {
    let held = || vec![text("token sk-test-"), block_stop()];

    let mut transcript = redacting();
    assert!(push_events(&mut transcript, held()).is_empty());
    let emissions = push_events(&mut transcript, vec![success(json!("done"), "session-1")]);
    assert_eq!(
        emitted(&emissions),
        [(LiveOutputStream::Output, "token sk-test-")]
    );
    assert!(transcript.finish_stream().is_empty());

    let mut transcript = redacting();
    assert!(push_events(&mut transcript, held()).is_empty());
    assert_eq!(
        emitted(&transcript.finish_stream()),
        [(LiveOutputStream::Output, "token sk-test-")]
    );
}

#[test]
fn empty_streamed_blocks_do_not_accumulate_held_parts() {
    let mut transcript = redacting();
    assert!(push_events(&mut transcript, vec![text("token sk-test-"), block_stop()]).is_empty());
    for _ in 0..1_024 {
        assert!(
            push_events(
                &mut transcript,
                vec![text(""), block_stop(), thinking(""), block_stop()],
            )
            .is_empty()
        );
    }
    assert_eq!(transcript.pending.text, "token sk-test-");
    assert_eq!(transcript.pending.parts.len(), 1);
    let emissions = push_events(
        &mut transcript,
        vec![text("0123456789abcdef ok"), block_stop()],
    );
    assert_eq!(
        emitted(&emissions),
        [
            (LiveOutputStream::Output, "token "),
            (LiveOutputStream::Output, "[REDACTED] ok"),
        ]
    );
}

#[test]
fn empty_assistant_blocks_do_not_accumulate_held_parts() {
    let mut content = vec![json!({"type":"text","text":"token sk-test-"})];
    content.extend((0..1_024).map(|_| json!({"type":"text","text":""})));
    let mut transcript = redacting();
    let event = json!({"type":"assistant","message":{"content":content}});
    assert!(push_events(&mut transcript, vec![event]).is_empty());
    assert_eq!(transcript.pending.text, "token sk-test-");
    assert_eq!(transcript.pending.parts.len(), 1);
    assert_eq!(
        emitted(&transcript.finish_stream()),
        [(LiveOutputStream::Output, "token sk-test-")]
    );

    content.push(json!({"type":"text","text":"0123456789abcdef ok"}));
    let emissions = push_events(
        &mut redacting(),
        vec![json!({"type":"assistant","message":{"content":content}})],
    );
    assert_eq!(
        emitted(&emissions),
        [
            (LiveOutputStream::Output, "token "),
            (LiveOutputStream::Output, "[REDACTED] ok"),
        ]
    );
}
