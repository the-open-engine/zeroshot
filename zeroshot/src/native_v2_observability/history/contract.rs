//! Feature-neutral semantic checks for history records received across a host boundary.
//! Wire decoding rejects unknown fields; these checks reject internally inconsistent values.

use std::collections::BTreeMap;

use openengine_cluster_protocol::{Cursor, RunId, TerminalResult};

use super::{HistoryPage, RunDefinition, RuntimeFailure};
use crate::v2_run_ledger::{cursor_for, cursor_sequence, RunPhase, MAX_REPLAY_EVENTS};

const DEFINITION_VERSION: u8 = 1;
const PROJECTION_VERSION: u8 = 1;
const RUNTIME_FAILURE_REASONS: [&str; 2] = ["runtime_failed", "runtime_lost"];

/// An intentionally opaque semantic contract failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("run history violates the shared wire contract")]
pub struct RunHistoryContractError(());

/// Validates a decoded definition against the requested run and the shared v1 projection.
pub fn validate_run_definition(
    definition: &RunDefinition,
    expected_run_id: &RunId,
) -> Result<(), RunHistoryContractError> {
    validate_definition_header(definition, expected_run_id)?;
    let cursor = validate_definition_cursors(definition)?;
    validate_definition_runtime_failure(definition, cursor)
}

fn validate_definition_header(
    definition: &RunDefinition,
    expected_run_id: &RunId,
) -> Result<(), RunHistoryContractError> {
    if definition.version != DEFINITION_VERSION
        || definition.projection_version != PROJECTION_VERSION
        || &definition.run_id != expected_run_id
        || !definition.history_available
    {
        return invalid();
    }
    Ok(())
}

fn validate_definition_cursors(definition: &RunDefinition) -> Result<u64, RunHistoryContractError> {
    let cursor = canonical_sequence(&definition.cursor)?;
    let history_cursor = canonical_sequence(&definition.history.cursor)?;
    if canonical_sequence(&definition.history.initial_cursor)? != 0 || cursor != history_cursor {
        return invalid();
    }
    Ok(cursor)
}

fn validate_definition_runtime_failure(
    definition: &RunDefinition,
    cursor: u64,
) -> Result<(), RunHistoryContractError> {
    let Some(failure) = &definition.runtime_failure else {
        return Ok(());
    };
    validate_runtime_failure(failure, cursor)?;
    let coherent_terminal = matches!(
        &definition.terminal,
        Some(TerminalResult::Failed { reason }) if reason.as_str() == failure.reason
    );
    if definition.phase != RunPhase::Finished || !coherent_terminal || definition.history.complete {
        return invalid();
    }
    Ok(())
}

/// Validates a decoded page against the exact cursor used to request it.
pub fn validate_history_page(
    page: &HistoryPage,
    after: &Cursor,
) -> Result<(), RunHistoryContractError> {
    let (requested, next, head) = validate_page_cursors(page, after)?;
    let event_positions = validate_events(&page.events, requested, next)?;
    if event_positions.is_empty() && next < head {
        return invalid();
    }
    validate_control(page, &event_positions)?;
    validate_page_runtime_failure(page, head)
}

fn validate_page_cursors(
    page: &HistoryPage,
    after: &Cursor,
) -> Result<(u64, u64, u64), RunHistoryContractError> {
    let requested = canonical_sequence(after)?;
    let next = canonical_sequence(&page.next_cursor)?;
    let head = canonical_sequence(&page.head_cursor)?;
    if page.events.len() > MAX_REPLAY_EVENTS
        || next < requested
        || next > head
        || page.complete != (next == head)
    {
        return invalid();
    }
    Ok((requested, next, head))
}

fn validate_page_runtime_failure(
    page: &HistoryPage,
    head: u64,
) -> Result<(), RunHistoryContractError> {
    let Some(failure) = &page.runtime_failure else {
        return Ok(());
    };
    validate_runtime_failure(failure, head)?;
    if !page.finished {
        return invalid();
    }
    Ok(())
}

fn validate_events(
    events: &[serde_json::Value],
    requested: u64,
    next: u64,
) -> Result<BTreeMap<u64, usize>, RunHistoryContractError> {
    let mut sequence = requested;
    let mut positions = BTreeMap::new();
    for (position, event) in events.iter().enumerate() {
        sequence = sequence.checked_add(1).ok_or_else(contract_error)?;
        let cursor = event
            .get("cursor")
            .and_then(serde_json::Value::as_str)
            .map(Cursor::new)
            .ok_or_else(contract_error)?;
        if canonical_sequence(&cursor)? != sequence {
            return invalid();
        }
        positions.insert(sequence, position);
    }
    if sequence != next {
        return invalid();
    }
    Ok(positions)
}

fn validate_control(
    page: &HistoryPage,
    event_positions: &BTreeMap<u64, usize>,
) -> Result<(), RunHistoryContractError> {
    let mut previous = None;
    for record in &page.control {
        let sequence = canonical_sequence(&record.cursor)?;
        let position = event_positions
            .get(&sequence)
            .copied()
            .ok_or_else(contract_error)?;
        if previous.is_some_and(|previous| position < previous) {
            return invalid();
        }
        previous = Some(position);
    }
    Ok(())
}

fn validate_runtime_failure(
    failure: &RuntimeFailure,
    head: u64,
) -> Result<(), RunHistoryContractError> {
    if !RUNTIME_FAILURE_REASONS.contains(&failure.reason.as_str())
        || canonical_sequence(&failure.at_cursor)? > head
    {
        return invalid();
    }
    Ok(())
}

fn canonical_sequence(cursor: &Cursor) -> Result<u64, RunHistoryContractError> {
    let sequence = cursor_sequence(cursor).map_err(|_| contract_error())?;
    if sequence > i64::MAX as u64 || cursor_for(sequence) != *cursor {
        return invalid();
    }
    Ok(sequence)
}

const fn contract_error() -> RunHistoryContractError {
    RunHistoryContractError(())
}

fn invalid<T>() -> Result<T, RunHistoryContractError> {
    Err(contract_error())
}

#[cfg(test)]
mod tests {
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
}
