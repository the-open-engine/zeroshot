use super::*;
use super::process::{run_invocation, run_turn, script, scripted_adapter};
use crate::native_v2_runner::LiveOutputStream;
use serde_json::json;
use crate::native_v2_runner::NodeDriver;

const COMPLETE: &str = r#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"{\"response\":null}"}]}}"#;

fn delta(text: &str) -> String {
    json!({"type":"message_update", "assistantMessageEvent":{"type":"text_delta","delta":text}})
        .to_string()
}

#[tokio::test]
async fn a_lost_output_consumer_stops_a_provider_that_would_keep_running() {
    let record = delta("visible output\n");
    let body = script(&[&record]).replace("exit 0\n", "sleep 30\n");
    assert_unpublishable_provider_stops(&body, serde_json::Value::Null).await;
}

#[tokio::test]
async fn output_failure_at_eof_interrupts_a_provider_with_unread_stdin() {
    // Short text stays buffered until stdout closes. The prompt is larger than the stdin pipe,
    // so waiting for input to finish before acting on that final output failure would deadlock.
    let record = delta("buffered output");
    let body = script(&[&record])
        .replace("cat > /dev/null\n", "")
        .replace("exit 0\n", "exec 1>&-\nsleep 30\n");
    assert_unpublishable_provider_stops(&body, json!("x".repeat(512 * 1024))).await;
}

async fn assert_unpublishable_provider_stops(body: &str, input: serde_json::Value) {
    let directory = TestDirectory::new("pi-stop-unpublishable-output");
    let adapter = scripted_adapter(&directory, body);
    let mut invocation = invocation(
        agent_binding("provider-model", None, SessionScope::Execution, &[]),
        NodeRole::Worker,
        resolved(&[], &[]),
    );
    invocation.node.input = input;
    let (_cancel, control, durable) = crate::native_v2_runner::test_support::control();
    drop(durable);
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        adapter.run(invocation, control),
    )
    .await
    .assert_value_with("failed output publication must stop the running provider");
    assert!(matches!(
        outcome,
        Err(crate::native_v2_runner::NodeRunnerError::DurableOutputClosed)
    ));
}

#[tokio::test]
async fn large_multibyte_and_nul_deltas_complete_with_bounded_public_output() {
    let directory = TestDirectory::new("pi-bounded-public-output");
    let text = format!("{}\0tail", "é🦀".repeat(5_000));
    let record = delta(&text);
    let adapter = scripted_adapter(&directory, &script(&[&record, COMPLETE]));
    let (outcome, output) = run_turn(&adapter, false).await;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(output.iter().all(|event| event.text.len() <= 16 * 1024));
    let visible = output
        .iter()
        .filter(|e| e.stream == LiveOutputStream::Output)
        .map(|e| e.text.as_str())
        .collect::<String>();
    assert_eq!(visible, text.replace('\0', "\u{fffd}"));
}

#[tokio::test]
async fn a_connection_key_split_across_records_and_messages_never_enters_public_output() {
    let directory = TestDirectory::new("pi-split-public-secret");
    let key = "review-private-provider-key";
    let first = delta("before review-private-");
    let second = delta("provider-key after");
    let boundary = r#"{"type":"message_end","message":{"role":"assistant","stopReason":"toolUse","content":[]}}"#;
    let adapter = scripted_adapter(&directory, &script(&[&first, boundary, &second, COMPLETE]));
    let binding = agent_binding(
        "provider-model",
        None,
        SessionScope::Execution,
        &["ANTHROPIC_API_KEY"],
    );
    let invocation = invocation(
        binding,
        NodeRole::Worker,
        resolved(&["ANTHROPIC_API_KEY"], &[("ANTHROPIC_API_KEY", key)]),
    );
    let (outcome, output) = run_invocation(&adapter, invocation, false).await;
    assert!(outcome.is_ok(), "{outcome:?}");
    let visible = output
        .iter()
        .filter(|e| e.stream == LiveOutputStream::Output)
        .map(|e| e.text.as_str())
        .collect::<String>();
    assert_eq!(visible, "before [REDACTED] after");
    assert!(output.iter().all(|event| !event.text.contains(key)));
}
