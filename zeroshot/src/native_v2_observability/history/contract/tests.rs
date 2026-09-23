use std::sync::Arc;

use openengine_cluster_protocol::{IdempotencyKey, Sha256Digest};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::v2_run_ledger::fake::FakeRunLedger;
use crate::v2_run_ledger::{CreateRun, RunLedger};

async fn definition() -> RunDefinition {
    let run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    let ledger = Arc::new(FakeRunLedger::new());
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("history-contract-test").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted: crate::native_v2_runner::test_support::admitted(),
        })
        .await
        .assert_value();
    super::super::RunHistoryService::with_sources(
        ledger,
        super::super::ControlCache::default(),
        None,
    )
    .definition(&run_id)
    .await
    .assert_value()
}

fn page(cursors: &[&str], next: &str, head: &str, complete: bool) -> HistoryPage {
    serde_json::from_value(json!({
        "events": cursors.iter().map(|cursor| json!({"cursor":cursor})).collect::<Vec<_>>(),
        "nextCursor": next,
        "headCursor": head,
        "complete": complete,
        "finished": false,
        "observation": {"state":"active"}
    }))
    .assert_value()
}

fn control(cursor: &str) -> super::super::ControlRecord {
    serde_json::from_value(json!({
        "cursor":cursor,
        "node":"worker",
        "mapIndices":[],
        "visitId":"worker",
        "state":"entered"
    }))
    .assert_value()
}

#[tokio::test]
async fn definition_validation_covers_versions_identity_cursors_and_runtime_failure() {
    let valid = definition().await;
    assert!(validate_run_definition(&valid, &valid.run_id).is_ok());

    let mut legacy = serde_json::to_value(&valid).assert_value();
    assert!(legacy.get("snapshot").is_none());
    legacy["snapshot"] = json!({"executions":{"1":{"large":"legacy projection"}}});
    let legacy: RunDefinition = serde_json::from_value(legacy).assert_value();
    assert!(legacy.legacy_snapshot.is_some());
    assert!(
        serde_json::to_value(&legacy)
            .assert_value()
            .get("snapshot")
            .is_none()
    );

    let mut changed = valid.clone();
    changed.version = 2;
    assert!(validate_run_definition(&changed, &valid.run_id).is_err());
    changed = valid.clone();
    changed.projection_version = 2;
    assert!(validate_run_definition(&changed, &valid.run_id).is_err());
    assert!(
        validate_run_definition(&valid, &RunId::new(uuid::Uuid::now_v7().to_string())).is_err()
    );

    changed = valid.clone();
    changed.cursor = Cursor::new("v2:00");
    assert!(validate_run_definition(&changed, &valid.run_id).is_err());
    changed = valid.clone();
    changed.history.initial_cursor = Cursor::new("v2:00");
    assert!(validate_run_definition(&changed, &valid.run_id).is_err());
    changed = valid.clone();
    changed.history.cursor = Cursor::new("v2:1");
    assert!(validate_run_definition(&changed, &valid.run_id).is_err());

    let mut failed = valid.clone();
    failed.phase = RunPhase::Finished;
    failed.terminal = Some(TerminalResult::Failed {
        reason: "runtime_failed".parse().assert_value(),
    });
    failed.history.complete = false;
    failed.runtime_failure = Some(RuntimeFailure {
        at_cursor: Cursor::new("v2:0"),
        reason: "runtime_failed".into(),
    });
    assert!(validate_run_definition(&failed, &valid.run_id).is_ok());
    failed.terminal = Some(TerminalResult::Failed {
        reason: "runtime_lost".parse().assert_value(),
    });
    failed.runtime_failure.as_mut().assert_value().reason = "runtime_lost".into();
    assert!(validate_run_definition(&failed, &valid.run_id).is_ok());
    failed.runtime_failure.as_mut().assert_value().reason = "other".into();
    assert!(validate_run_definition(&failed, &valid.run_id).is_err());
    failed.runtime_failure = Some(RuntimeFailure {
        at_cursor: Cursor::new("v2:1"),
        reason: "runtime_failed".into(),
    });
    assert!(validate_run_definition(&failed, &valid.run_id).is_err());
    failed.runtime_failure.as_mut().assert_value().at_cursor = Cursor::new("v2:0");
    failed.history.complete = true;
    assert!(validate_run_definition(&failed, &valid.run_id).is_err());
}

#[test]
fn page_validation_rejects_noncanonical_gapped_oversized_and_stalled_pages() {
    let valid = page(&["v2:1", "v2:2"], "v2:2", "v2:3", false);
    assert!(validate_history_page(&valid, &Cursor::new("v2:0")).is_ok());
    assert!(validate_history_page(&valid, &Cursor::new("v2:00")).is_err());
    assert!(validate_history_page(&valid, &Cursor::new("v2:9223372036854775808")).is_err());
    let boundary = page(
        &[],
        "v2:9223372036854775807",
        "v2:9223372036854775807",
        true,
    );
    assert!(validate_history_page(&boundary, &Cursor::new("v2:9223372036854775807")).is_ok());

    let mut changed = valid.clone();
    changed.events[1]["cursor"] = json!("v2:3");
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());
    changed = valid.clone();
    changed.events[1]["cursor"] = json!("v2:02");
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());
    changed = valid.clone();
    changed.next_cursor = Cursor::new("v2:1");
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());
    changed = valid.clone();
    changed.complete = true;
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());

    let cursors = (1..=MAX_REPLAY_EVENTS + 1)
        .map(|sequence| format!("v2:{sequence}"))
        .collect::<Vec<_>>();
    let events = cursors
        .iter()
        .map(|cursor| json!({"cursor":cursor}))
        .collect();
    changed = page(&[], "v2:0", "v2:0", true);
    changed.events = events;
    changed.next_cursor = Cursor::new(format!("v2:{}", MAX_REPLAY_EVENTS + 1));
    changed.head_cursor = changed.next_cursor.clone();
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());

    let stalled = page(&[], "v2:0", "v2:1", false);
    assert!(validate_history_page(&stalled, &Cursor::new("v2:0")).is_err());
}

#[test]
fn page_validation_anchors_ordered_control_and_coherent_runtime_failure() {
    let mut valid = page(&["v2:1", "v2:2"], "v2:2", "v2:2", true);
    valid.control = vec![control("v2:1"), control("v2:2")];
    assert!(validate_history_page(&valid, &Cursor::new("v2:0")).is_ok());

    let mut changed = valid.clone();
    changed.control = vec![control("v2:2"), control("v2:1")];
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());
    changed = valid.clone();
    changed.control.push(control("v2:3"));
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());

    valid.finished = true;
    valid.runtime_failure = Some(RuntimeFailure {
        at_cursor: Cursor::new("v2:2"),
        reason: "runtime_failed".into(),
    });
    assert!(validate_history_page(&valid, &Cursor::new("v2:0")).is_ok());
    valid.runtime_failure.as_mut().assert_value().reason = "runtime_lost".into();
    assert!(validate_history_page(&valid, &Cursor::new("v2:0")).is_ok());
    changed = valid.clone();
    changed.finished = false;
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());
    changed = valid.clone();
    changed.runtime_failure.as_mut().assert_value().reason = "other".into();
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());
    changed = valid;
    changed.runtime_failure.as_mut().assert_value().at_cursor = Cursor::new("v2:3");
    assert!(validate_history_page(&changed, &Cursor::new("v2:0")).is_err());
}
