use super::*;
use crate::native_v2_claude::input::TurnSignal;

fn signals(records: &[Value]) -> (Vec<TurnSignal>, bool) {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut transcript = ClaudeTranscript::new(Vec::new()).reporting_input(sender);
    let mut bytes = Vec::new();
    for record in records {
        append_event(&mut bytes, record.clone());
    }
    transcript.push(&bytes);
    let mut seen = Vec::new();
    while let Ok(signal) = receiver.try_recv() {
        seen.push(signal);
    }
    // Stdout has more to say until it ends; then the channel closes so that stdin can.
    let open = !receiver.is_closed();
    transcript.finish_stream();
    assert!(receiver.is_closed());
    (seen, open)
}

#[test]
fn replayed_user_messages_and_results_reach_the_stdin_side_in_order() {
    let text = "<steer>\n[id] line one\nline \"two\"\n</steer>";
    let (seen, open) = signals(&[
        json!({"type":"system","subtype":"init","session_id":"s"}),
        json!({"type":"user","isReplay":true,"session_id":"s",
               "message":{"role":"user","content":[{"type":"text","text":"the prompt"}]}}),
        // A tool result is a user record too, and is no message from stdin.
        json!({"type":"user","session_id":"s",
               "message":{"role":"user","content":[{"type":"tool_result","content":"<steer>"}]}}),
        json!({"type":"user","isReplay":true,"session_id":"s",
               "message":{"role":"user","content":text}}),
        success(json!({"response":"done"}), "s"),
    ]);
    assert!(open);
    assert_eq!(
        seen,
        [
            TurnSignal::Replayed("the prompt".to_owned()),
            TurnSignal::Replayed(text.to_owned()),
            TurnSignal::Result,
        ]
    );
}

#[test]
fn a_transcript_without_a_stdin_side_ignores_replays() {
    let mut transcript = ClaudeTranscript::new(Vec::new());
    let mut bytes = Vec::new();
    append_event(
        &mut bytes,
        json!({"type":"user","isReplay":true,"message":{"role":"user","content":"x"}}),
    );
    append_event(&mut bytes, success(json!({"response":"done"}), "s"));
    assert!(transcript.push(&bytes).is_empty());
    assert!(transcript.is_success());
}
