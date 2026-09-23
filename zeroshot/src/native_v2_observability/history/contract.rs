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
#[path = "contract/tests.rs"]
mod tests;
