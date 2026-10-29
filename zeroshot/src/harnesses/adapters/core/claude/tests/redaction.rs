//! Known credentials stay redacted in the durable node output that the supervisor persists as run
//! log records, including when Claude splits one across adjacent output blocks.

use super::*;
use crate::native_v2_runner::DurableNodeEvent;

const CREDENTIAL: &str = "sk-ant-synthetic-boundary-credential-0042";
const HEAD: &str = "sk-ant-synthetic-";
const TAIL: &str = "boundary-credential-0042";
const SESSION: &str = "redaction-session";
const MESSAGE: &str = "msg_redaction";
const MODEL: &str = "claude-haiku-4-5";

fn stream_event(event: Value) -> Value {
    json!({
        "type":"stream_event",
        "session_id":SESSION,
        "parent_tool_use_id":null,
        "event":event
    })
}

fn assistant(blocks: &[String]) -> Value {
    let content = blocks
        .iter()
        .map(|text| json!({"type":"text","text":text}))
        .collect::<Vec<_>>();
    json!({
        "type":"assistant",
        "session_id":SESSION,
        "parent_tool_use_id":null,
        "message":{
            "id":MESSAGE,
            "type":"message",
            "role":"assistant",
            "model":MODEL,
            "content":content,
            "stop_reason":"end_turn",
            "stop_sequence":null,
            "usage":{"input_tokens":11,"output_tokens":4}
        }
    })
}

/// One streamed assistant message whose text blocks receive the given deltas, followed by the
/// complete assistant message Claude Code reports for it.
fn streamed_message(blocks: &[&[&str]]) -> Vec<Value> {
    let mut events = vec![stream_event(json!({
        "type":"message_start",
        "message":{
            "id":MESSAGE,
            "type":"message",
            "role":"assistant",
            "model":MODEL,
            "content":[],
            "stop_reason":null,
            "stop_sequence":null,
            "usage":{"input_tokens":11,"output_tokens":1}
        }
    }))];
    for (index, deltas) in blocks.iter().enumerate() {
        events.push(stream_event(json!({
            "type":"content_block_start",
            "index":index,
            "content_block":{"type":"text","text":""}
        })));
        for text in *deltas {
            events.push(stream_event(json!({
                "type":"content_block_delta",
                "index":index,
                "delta":{"type":"text_delta","text":text}
            })));
        }
        events.push(stream_event(
            json!({"type":"content_block_stop","index":index}),
        ));
    }
    events.push(stream_event(json!({
        "type":"message_delta",
        "delta":{"stop_reason":"end_turn","stop_sequence":null},
        "usage":{"output_tokens":4}
    })));
    events.push(stream_event(json!({"type":"message_stop"})));
    let texts = blocks
        .iter()
        .map(|deltas| deltas.concat())
        .collect::<Vec<_>>();
    events.push(assistant(&texts));
    events
}

/// Runs one Claude turn whose provider declares `CREDENTIAL` as its Anthropic API key and returns
/// the text of every durable output record in order, exactly as the supervisor's durable log
/// bridge receives it.
async fn durable_output_records(label: &str, events: Vec<Value>) -> Vec<String> {
    let workspace = TestDirectory::new(label);
    let mut transcript = Vec::new();
    for event in std::iter::once(json!({
        "type":"system",
        "subtype":"init",
        "session_id":SESSION,
        "model":MODEL
    }))
    .chain(events)
    .chain(std::iter::once(json!({
        "type":"result",
        "subtype":"success",
        "is_error":false,
        "session_id":SESSION,
        "result":"done",
        "structured_output":{"response":"done"},
        "usage":{"input_tokens":11,"output_tokens":4}
    }))) {
        transcript.extend(serde_json::to_vec(&event).assert_value());
        transcript.push(b'\n');
    }
    workspace.write(
        "provider-output.ndjson",
        &String::from_utf8(transcript).assert_value(),
    );
    workspace.write(
        "fake-claude.sh",
        &format!(
            "set -eu\ncat >/dev/null\ntest \"${{ANTHROPIC_API_KEY-}}\" = '{CREDENTIAL}'\n\
             exec /bin/cat provider-output.ndjson\n"
        ),
    );

    let (runner, binding) =
        anthropic_runner(&workspace, MODEL, None, SessionScope::Execution).await;
    let mut handle = runner
        .start(request(binding, 1, &[(ANTHROPIC_KEY, CREDENTIAL)]))
        .await
        .assert_value();
    let mut output = handle.take_initial_output().assert_value();
    let records = async {
        let mut records = Vec::new();
        loop {
            match output.recv().await {
                Ok(DurableNodeEvent::Output { output, .. }) => records.push(output.text),
                Ok(DurableNodeEvent::TokenUsage(_)) => {}
                Err(_) => return records,
            }
        }
    };
    let (completion, records) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(handle.completion(), records)
    })
    .await
    .assert_value_with("Claude turn and durable output drain deadline");
    let completion = completion.assert_value_with("Claude turn completion");
    assert!(
        matches!(
            &completion.outcome,
            WorkerOutcome::Verified { output, .. } if *output == json!("done")
        ),
        "unexpected outcome: {:?}",
        completion.outcome
    );
    records
}

fn assert_credential_redacted(records: &[String]) {
    let joined = records.concat();
    let compact = joined
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(
        !compact.contains(CREDENTIAL),
        "durable output records can be reassembled into the credential: {records:?}"
    );
    assert!(
        records
            .iter()
            .all(|record| !record.contains(HEAD) && !record.contains(TAIL)),
        "a durable output record retains a credential fragment: {records:?}"
    );
    assert!(
        joined.contains("Key ") && joined.contains("[REDACTED]") && joined.contains(" done."),
        "durable output lost its redacted text: {records:?}"
    );
}

#[tokio::test]
async fn control_a_credential_split_across_deltas_of_one_streamed_block_is_redacted() {
    let records = durable_output_records(
        "claude-redaction-one-streamed-block",
        streamed_message(&[&["Key sk-ant-synthetic-", "boundary-credential-0042 done."]]),
    )
    .await;
    assert_credential_redacted(&records);
}

#[tokio::test]
async fn control_a_credential_inside_one_fallback_text_block_is_redacted() {
    let records = durable_output_records(
        "claude-redaction-one-fallback-block",
        vec![assistant(&[format!("Key {CREDENTIAL} done.")])],
    )
    .await;
    assert_credential_redacted(&records);
}

#[tokio::test]
async fn a_credential_split_across_streamed_blocks_separated_by_content_block_stop_is_redacted() {
    let records = durable_output_records(
        "claude-redaction-streamed-blocks",
        streamed_message(&[&["Key ", HEAD], &[TAIL, " done."]]),
    )
    .await;
    assert_credential_redacted(&records);
}

#[tokio::test]
async fn a_credential_split_across_adjacent_fallback_text_blocks_is_redacted() {
    let records = durable_output_records(
        "claude-redaction-fallback-blocks",
        vec![assistant(&[format!("Key {HEAD}"), format!("{TAIL} done.")])],
    )
    .await;
    assert_credential_redacted(&records);
}
