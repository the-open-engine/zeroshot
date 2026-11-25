//! Pure browser projections shared by hosts that own retained per-execution history.
use std::collections::HashSet;

use openengine_cluster_protocol::{Cursor, RunId};
use serde_json::{json, Value};

use super::{
    created_at, HistoryPage, ObservationState, RunDefinition, RunHistorySummary, RunHistoryPhase,
};
pub use crate::native_v2_observability::lineage::{part_cursor, scope_diagnostic_cursors, split_cursor};
use crate::v2_run_ledger::{initial_cursor, RunPhase};

/// Project the current part's menu entry under the original submission.
pub fn grouped_summary(root: &RunId, count: usize, entry: RunHistorySummary) -> Value {
    let current = &entry.run_id;
    let mut row = json!(entry);
    row["runId"] = json!(root);
    row["createdAt"] = json!(created_at(root));
    row["currentPartId"] = json!(current);
    row["partId"] = json!(current);
    row["partCount"] = json!(count);
    if let Some(cursor) = entry.cursor {
        row["cursor"] = json!(part_cursor(current, &cursor));
    }
    qualify_state(&mut row, current);
    row
}

/// Qualify the current definition for the supplied ordered, authorized run parts.
/// The host supplies aggregate usage and owns the lifecycle of the current part.
pub fn grouped_definition(parts: &[RunId], mut detail: RunDefinition) -> Option<Value> {
    let root = parts.first()?;
    let head = parts.last()?;
    if &detail.run_id != head {
        return None;
    }
    detail.cursor = part_cursor(head, &detail.cursor);
    detail.history.cursor = detail.cursor.clone();
    detail.history.initial_cursor = part_cursor(root, &initial_cursor());
    detail.history.complete = detail.phase == RunPhase::Finished && detail.history.complete;
    detail.created_at = created_at(root);
    detail.run_id = root.clone();
    let mut body = json!(detail);
    body["currentPartId"] = json!(head);
    body["partId"] = json!(head);
    body["parts"] = json!(parts);
    qualify_state(&mut body, head);
    Some(body)
}

// Preserve the exact per-part cursor for graph projection and expose a reconnectable
// runCursor wherever a browser can link to durable evidence.
fn qualify_cursor(record: &mut Value, part: &RunId, field: &str) {
    if let Some(cursor) = record.get(field).and_then(Value::as_str) {
        record["runCursor"] = json!(part_cursor(part, &Cursor::new(cursor)));
        record["partId"] = json!(part);
    }
}

fn qualify_diagnostics(context: &mut Value, part: &RunId) {
    if let Some(primary) = context.get_mut("primary") {
        qualify_cursor(primary, part, "cursor");
    }
    for key in ["recorded", "verifiers"] {
        if let Some(records) = context.get_mut(key).and_then(Value::as_array_mut) {
            for record in records {
                qualify_cursor(record, part, "cursor");
            }
        }
    }
}

fn qualify_state(body: &mut Value, part: &RunId) {
    if let Some(failure) = body.get_mut("runtimeFailure") {
        qualify_cursor(failure, part, "atCursor");
        if let Some(context) = failure.get_mut("context") {
            qualify_diagnostics(context, part);
        }
    }
    if let Some(context) = body.pointer_mut("/terminal/context") {
        qualify_diagnostics(context, part);
    }
}

/// A page from one execution, with positions suitable for replay across the whole run.
pub struct RunPartsPage {
    run_id: RunId,
    current_part_id: RunId,
    part_id: RunId,
    part_after_cursor: Cursor,
    part_head_cursor: Cursor,
    part_next_cursor: Cursor,
    page: HistoryPage,
}

fn copied_vector(event: &Value, part: &RunId) -> bool {
    event["kind"] == "vector"
        && event["vector"]["runId"]
            .as_str()
            .is_some_and(|id| id != part.as_str())
}

impl RunPartsPage {
    /// Project a physical page from an ordered series of authorized, committed parts.
    /// A queued current part without a native head starts at its initial cursor.
    pub fn from_part(
        parts: &[RunId],
        offset: (usize, Cursor),
        mut page: HistoryPage,
        head_summary: Option<&RunHistorySummary>,
    ) -> Option<Self> {
        let (position, cursor) = offset;
        let part = parts.get(position)?;
        let current = parts.last()?;
        let at_head = part == current;
        let exhausted = page.complete;
        let next_part = parts.get(position + 1).filter(|_| exhausted);
        let next = match next_part {
            Some(next) => part_cursor(next, &initial_cursor()),
            None => part_cursor(part, &page.next_cursor),
        };
        let head = if at_head {
            part_cursor(part, &page.head_cursor)
        } else {
            let cursor = head_summary.and_then(|summary| summary.cursor.as_ref());
            part_cursor(current, cursor.unwrap_or(&initial_cursor()))
        };

        let current_suspended =
            head_summary.is_some_and(|summary| summary.phase == RunHistoryPhase::Suspended);
        page.finished = at_head && page.finished && !current_suspended;
        page.complete = at_head && exhausted;
        if !at_head || current_suspended {
            page.observation.state = ObservationState::Collecting;
        }
        Some(Self {
            run_id: parts.first()?.clone(),
            current_part_id: current.clone(),
            part_id: part.clone(),
            part_after_cursor: cursor,
            part_next_cursor: std::mem::replace(&mut page.next_cursor, next),
            part_head_cursor: std::mem::replace(&mut page.head_cursor, head),
            page,
        })
    }

    /// Lifecycle and qualified positions for a host following this page over SSE.
    pub fn history(&self) -> &HistoryPage {
        &self.page
    }

    /// Render the viewer wire contract, preserving execution seeds and exact evidence positions.
    pub fn value(&self) -> Value {
        let mut body = json!(self.page);
        body["runId"] = json!(self.run_id);
        body["currentPartId"] = json!(self.current_part_id);
        body["partId"] = json!(self.part_id);
        body["partAfterCursor"] = json!(self.part_after_cursor);
        body["partHeadCursor"] = json!(self.part_head_cursor);
        body["partNextCursor"] = json!(self.part_next_cursor);
        let part = &self.part_id;
        qualify_state(&mut body, part);
        let mut seeds = Vec::new();
        let mut omitted = Vec::new();
        let mut imported = HashSet::new();
        if let Some(events) = body.get_mut("events").and_then(Value::as_array_mut) {
            events.retain_mut(|record| {
                qualify_cursor(record, part, "cursor");
                if let Some(context) = record.pointer_mut("/event/result/context") {
                    qualify_diagnostics(context, part);
                }
                if part == &self.run_id {
                    return true;
                }
                let cursor = record["cursor"].as_str().unwrap_or_default().to_owned();
                if record["event"]["kind"] == "prior_execution" {
                    imported.insert(cursor);
                    seeds.push(record.take());
                } else if copied_vector(&record["event"], part) {
                    imported.insert(cursor.clone());
                    omitted.push(cursor);
                } else {
                    return true;
                }
                false
            });
        }
        if !omitted.is_empty() {
            body["omittedCursors"] = json!(omitted);
        }
        if !seeds.is_empty() {
            body["imports"] = json!(seeds);
        }
        let mut seed_controls = Vec::new();
        if let Some(controls) = body.get_mut("control").and_then(Value::as_array_mut) {
            controls.retain_mut(|record| {
                qualify_cursor(record, part, "cursor");
                if record["cursor"]
                    .as_str()
                    .is_some_and(|cursor| imported.contains(cursor))
                {
                    seed_controls.push(record.take());
                    false
                } else {
                    true
                }
            });
        }
        if !seed_controls.is_empty() {
            body["importedControl"] = json!(seed_controls);
        }
        body
    }
}
