use futures_util::{FutureExt, StreamExt};
use tokio::sync::mpsc;

use super::{LiveTurn, TurnSignal, live, user_message};

struct Turn {
    steers: mpsc::UnboundedSender<String>,
    signals: mpsc::UnboundedSender<TurnSignal>,
    stdin: crate::native_v2_capsule::provider_process::LiveInput<'static>,
}

fn turn() -> Turn {
    let (steers, steer_messages) = mpsc::unbounded_channel();
    let (signals, turn_signals) = mpsc::unbounded_channel();
    Turn {
        steers,
        signals,
        stdin: live(LiveTurn {
            steers: steer_messages,
            signals: turn_signals,
            hooks: None,
            execution: 1,
        }),
    }
}

fn text(line: &[u8]) -> String {
    let record: serde_json::Value = serde_json::from_slice(line).unwrap();
    assert_eq!(record["type"], "user");
    assert_eq!(record["message"]["role"], "user");
    record["message"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_user_message_is_one_json_line_with_its_text_intact() {
    let prompt = "line one\nline \"two\" \\ é\r\n";
    let line = user_message(prompt);
    assert_eq!(line.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(line.last(), Some(&b'\n'));
    assert_eq!(text(&line), prompt);
}

#[tokio::test]
async fn stdin_closes_at_the_result_of_a_turn_without_steers() {
    let mut turn = turn();
    turn.signals
        .send(TurnSignal::Replayed("the prompt".to_owned()))
        .unwrap();
    turn.signals.send(TurnSignal::Result).unwrap();
    assert!(turn.stdin.next().await.is_none());
}

#[tokio::test]
async fn a_sent_steer_keeps_stdin_open_until_the_turn_that_showed_it_has_ended() {
    let mut turn = turn();
    turn.steers.send("<steer>go</steer>".to_owned()).unwrap();
    assert_eq!(text(&turn.stdin.next().await.unwrap()), "<steer>go</steer>");
    // The turn in progress ended before Claude Code took the queued message: it starts another.
    turn.signals.send(TurnSignal::Result).unwrap();
    turn.signals
        .send(TurnSignal::Replayed("<steer>go</steer>".to_owned()))
        .unwrap();
    assert!(turn.stdin.next().now_or_never().is_none());
    turn.signals.send(TurnSignal::Result).unwrap();
    assert!(turn.stdin.next().await.is_none());
}

#[tokio::test]
async fn a_steer_waiting_when_a_result_arrives_is_written_before_stdin_closes() {
    // Both are ready before stdin is asked for its next message, many times over: whichever
    // order they are looked at in must not decide whether the steer is written.
    for _ in 0..64 {
        let mut turn = turn();
        turn.signals.send(TurnSignal::Result).unwrap();
        turn.steers.send("<steer>go</steer>".to_owned()).unwrap();
        assert_eq!(text(&turn.stdin.next().await.unwrap()), "<steer>go</steer>");
        // The steer starts a turn of its own; stdin stays open until that turn has ended.
        assert!(turn.stdin.next().now_or_never().is_none());
        turn.signals
            .send(TurnSignal::Replayed("<steer>go</steer>".to_owned()))
            .unwrap();
        turn.signals.send(TurnSignal::Result).unwrap();
        assert!(turn.stdin.next().await.is_none());
    }
}

#[tokio::test]
async fn stdin_closes_when_stdout_has_ended() {
    let mut turn = turn();
    turn.steers.send("<steer>go</steer>".to_owned()).unwrap();
    assert!(turn.stdin.next().await.is_some());
    drop(turn.signals);
    assert!(turn.stdin.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn stdin_closes_when_a_sent_steer_stays_unreported_after_a_result() {
    let mut turn = turn();
    turn.steers.send("<steer>go</steer>".to_owned()).unwrap();
    assert!(turn.stdin.next().await.is_some());
    turn.signals.send(TurnSignal::Result).unwrap();
    let started = tokio::time::Instant::now();
    assert!(turn.stdin.next().await.is_none());
    assert_eq!(started.elapsed(), super::UNREPORTED_AFTER_RESULT);
}

#[tokio::test]
async fn steers_stop_when_their_source_is_retired_and_the_result_still_closes_stdin() {
    let mut turn = turn();
    drop(turn.steers);
    turn.signals.send(TurnSignal::Result).unwrap();
    assert!(turn.stdin.next().await.is_none());
}
