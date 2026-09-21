use super::*;
use crate::native_v2_supervisor::durable_history;
use crate::v2_run_ledger::{
    MAX_EVENT_BYTES, MAX_PRIOR_EXECUTION_BYTES, MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, RunSnapshot,
    apply_event, cursor_sequence,
};
use crate::v2_run_ledger::state::validate_event;

fn with_sized_payload(event: RunEvent, pointer: &str, bytes: usize) -> RunEvent {
    let mut wire = serde_json::to_value(event).assert_value();
    *wire.pointer_mut(pointer).assert_value() = json!("");
    let overhead = serde_json::to_vec(&wire).assert_value().len();
    *wire.pointer_mut(pointer).assert_value() = json!("x".repeat(bytes - overhead));
    let event = serde_json::from_value(wire).assert_value();
    assert_eq!(serde_json::to_vec(&event).assert_value().len(), bytes);
    event
}

fn assert_event_boundaries(prior: &RunEvent, run_id: &RunId) {
    for (event, pointer, bound) in [
        (started(reference(run_id, 1)), "/input", MAX_EVENT_BYTES),
        (
            completed(reference(run_id, 1), Value::Null),
            "/completion/outcome/output",
            MAX_EVENT_BYTES,
        ),
        (prior.clone(), "/execution/input", MAX_PRIOR_EXECUTION_BYTES),
    ] {
        let exact = with_sized_payload(event.clone(), pointer, bound);
        validate_event(&exact).assert_value();
        let oversized = with_sized_payload(event, pointer, bound + 1);
        assert_eq!(
            validate_event(&oversized),
            Err(RunLedgerError::EventTooLarge)
        );
    }
}

async fn source_prerequisites(ledger: &dyn RunLedger) -> Vec<RunEvent> {
    let source = RunId::new("large-prerequisite-source");
    ledger
        .create_or_get(create(source.as_str(), source.as_str(), '8'))
        .await
        .assert_value();
    let mut events = vec![RunEvent::RunStarted];
    for execution in 1..=2 {
        events.push(with_sized_payload(
            started(reference(&source, execution)),
            "/input",
            MAX_EVENT_BYTES,
        ));
        events.push(with_sized_payload(
            completed(reference(&source, execution), Value::Null),
            "/completion/outcome/output",
            MAX_EVENT_BYTES,
        ));
    }
    let original = ledger.append(&source, events).await.assert_value().snapshot;
    let prerequisites = durable_history(&original).assert_value();
    assert_eq!(prerequisites.len(), 2);
    prerequisites
        .into_iter()
        .map(|execution| RunEvent::PriorExecution { execution })
        .collect()
}

async fn assert_large_prerequisite_replay(ledger: &dyn RunLedger) {
    let mut events = source_prerequisites(ledger).await;
    let successor = RunId::new("large-prerequisite-successor");
    assert_event_boundaries(events.assert_at(0), &successor);
    for event in &events {
        let bytes = serde_json::to_vec(event).assert_value().len();
        assert!(bytes > MAX_EVENT_BYTES && bytes <= MAX_PRIOR_EXECUTION_BYTES);
    }
    ledger
        .create_or_get(create(successor.as_str(), successor.as_str(), '9'))
        .await
        .assert_value();
    let oversized = with_sized_payload(
        events.assert_at(0).clone(),
        "/execution/input",
        MAX_PRIOR_EXECUTION_BYTES + 1,
    );
    assert_eq!(
        ledger.append(&successor, vec![oversized]).await,
        Err(RunLedgerError::EventTooLarge)
    );
    let untouched = ledger
        .get(&successor)
        .await
        .assert_value()
        .assert_value()
        .snapshot;
    assert!(untouched.execution_seed.is_empty());
    assert_eq!(untouched.cursor, cursor_for(0));
    events.push(RunEvent::RunStarted);
    events.push(RunEvent::Terminal {
        result: TerminalResult::Succeeded {
            output: Value::Null,
        },
    });
    let expected = ledger
        .append(&successor, events)
        .await
        .assert_value()
        .snapshot;
    assert_eq!(expected.execution_seed.len(), 2);
    assert!(expected.executions.is_empty());
    assert!(expected.token_usage.is_none());
    assert_bounded_replay(ledger, &expected).await;
}

async fn assert_bounded_replay(ledger: &dyn RunLedger, expected: &RunSnapshot) {
    let mut replay = expected.replay_seed();
    let mut pages = 0;
    let mut count = 0;
    while replay.cursor != expected.cursor {
        let tail = ledger
            .snapshot_and_tail(&expected.run_id, Some(&replay.cursor))
            .await
            .assert_value();
        assert!(
            !tail.events.is_empty(),
            "a legal event must fit so replay makes progress"
        );
        assert!(tail.events.len() <= MAX_REPLAY_EVENTS);
        let mut page_bytes = 0;
        assert_eq!(tail.snapshot, *expected);
        for stored in tail.events {
            page_bytes += serde_json::to_vec(&stored.event).assert_value().len();
            count += 1;
            assert_eq!(stored.cursor, cursor_for(count));
            apply_event(
                &mut replay,
                &stored.event,
                cursor_sequence(&stored.cursor).assert_value(),
            )
            .assert_value();
        }
        assert!(page_bytes <= MAX_REPLAY_BYTES);
        pages += 1;
    }
    assert_eq!(count, 4);
    assert!(
        pages >= 2,
        "large prerequisites must cross a replay page boundary"
    );
    assert_eq!(replay, *expected);
}

#[tokio::test]
async fn fake_imports_maximum_source_payloads_and_replays_large_prerequisites() {
    assert_large_prerequisite_replay(&FakeRunLedger::new()).await;
}

#[tokio::test]
async fn sqlite_imports_maximum_source_payloads_and_replays_large_prerequisites() {
    assert_large_prerequisite_replay(&SqliteRunLedger::open_in_memory().assert_value()).await;
}
