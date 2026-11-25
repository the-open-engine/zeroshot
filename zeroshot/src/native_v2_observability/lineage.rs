//! Public run identity follows the host's committed, reciprocal workspace handoffs.
//! A preparation row, copied continuation vector or proposed successor on its own
//! cannot move a waiting run to another part.
use std::collections::{HashSet, VecDeque};
use std::future::Future;

use openengine_cluster_protocol::{Cursor, RunId, RunStatus, TerminalResult, WorkspaceRecovery};
use thiserror::Error;
use crate::v2_run_ledger::RunLedgerError;

#[derive(Debug, Error)]
pub(crate) enum LineageError<E> {
    #[error("{0}")]
    Read(E),
    #[error("cycle in run recovery lineage")]
    Cycle,
}

/// An execution selector belongs to one ledger. Its scoped cursor can identify
/// that ledger after another part has started.
#[derive(Clone, Copy)]
pub(crate) enum StreamSelection<'a> {
    All,
    Execution,
    Part(&'a RunId),
}

impl<'a> StreamSelection<'a> {
    pub(crate) fn new(part: Option<&'a RunId>, execution: bool) -> Self {
        match (part, execution) {
            (Some(part), _) => Self::Part(part),
            (None, true) => Self::Execution,
            (None, false) => Self::All,
        }
    }
}

#[derive(Debug)]
pub(crate) struct RunLineage {
    parts: Vec<RunId>,
}

impl RunLineage {
    pub(crate) async fn resolve<E, F, Fut>(id: &RunId, read: F) -> Result<Self, LineageError<E>>
    where
        F: Fn(RunId) -> Fut,
        Fut: Future<Output = Result<Option<WorkspaceRecovery>, E>>,
    {
        let origin = read(id.clone())
            .await
            .map_err(LineageError::Read)?
            .unwrap_or_default();
        let mut parts = VecDeque::from([id.clone()]);
        let mut seen = HashSet::from([id.clone()]);
        for backwards in [true, false] {
            let mut from = id.clone();
            let mut next = if backwards {
                origin.resumed_from.clone()
            } else {
                origin.successor_run_id.clone()
            };
            while let Some(candidate) = next {
                let Some(record) = read(candidate.clone()).await.map_err(LineageError::Read)?
                else {
                    break;
                };
                let (back, onward) = if backwards {
                    (record.successor_run_id, record.resumed_from)
                } else {
                    (record.resumed_from, record.successor_run_id)
                };
                if back.as_ref() != Some(&from) {
                    break;
                }
                if !seen.insert(candidate.clone()) {
                    return Err(LineageError::Cycle);
                }
                if backwards {
                    parts.push_front(candidate.clone());
                } else {
                    parts.push_back(candidate.clone());
                }
                from = candidate;
                next = onward;
            }
        }
        Ok(Self {
            parts: parts.into(),
        })
    }

    /// Unqualified legacy cursors belong to the addressed ledger. A scoped cursor
    /// always resumes its original part, including execution-filtered log reads.
    pub(crate) fn stream_position(
        &self,
        addressed: &RunId,
        selected: StreamSelection<'_>,
        cursor: Option<Cursor>,
    ) -> Result<(RunId, Option<Cursor>), RunLedgerError> {
        if let Some((part, local)) = cursor.as_ref().and_then(split_cursor) {
            if !self.contains(&part) || matches!(selected, StreamSelection::Part(id) if id != &part)
            {
                return Err(RunLedgerError::InvalidCursor);
            }
            return Ok((part, Some(local)));
        }
        let part = match selected {
            StreamSelection::Part(part) => part,
            _ if cursor.is_some() => addressed,
            StreamSelection::Execution => self.current(),
            StreamSelection::All => self.root(),
        };
        if !self.contains(part) {
            return Err(RunLedgerError::InvalidCursor);
        }
        Ok((part.clone(), cursor))
    }

    pub(crate) fn parts(&self) -> &[RunId] {
        &self.parts
    }
    pub(crate) fn root(&self) -> &RunId {
        &self.parts[0]
    }
    pub(crate) fn current(&self) -> &RunId {
        &self.parts[self.parts.len() - 1]
    }
    pub(crate) fn successor(&self, part: &RunId) -> Option<&RunId> {
        self.parts
            .windows(2)
            .find(|pair| &pair[0] == part)
            .map(|pair| &pair[1])
    }

    /// Publish only the neighbors that selected this part. A later sidecar read can
    /// already include a newer claim, or a child whose ledger was never admitted.
    pub(crate) fn project_recovery(&self, part: &RunId, recovery: &mut WorkspaceRecovery) {
        recovery.successor_run_id = self.successor(part).cloned();
        recovery.resumed_from = self
            .parts
            .windows(2)
            .find(|pair| &pair[1] == part)
            .map(|pair| pair[0].clone());
    }

    pub(crate) fn contains(&self, id: &RunId) -> bool {
        self.parts.contains(id)
    }
}

/// Ledger cursors contain no '@'. Splitting from the right also admits older non-UUID run IDs.
pub fn part_cursor(part: &RunId, cursor: &Cursor) -> Cursor {
    Cursor::new(format!("{}@{}", part.as_str(), cursor.as_str()))
}

/// Extract the part and its opaque local cursor. Hosts validate both against the selected run.
pub fn split_cursor(cursor: &Cursor) -> Option<(RunId, Cursor)> {
    let (id, local) = cursor.as_str().rsplit_once('@')?;
    Some((RunId::new(id), Cursor::new(local)))
}

/// Match failure evidence to the same logical log cursor returned by replay.
pub fn scope_diagnostic_cursors(part: &RunId, status: &mut RunStatus) {
    let result = match status {
        RunStatus::Finished {
            terminal_result, ..
        } => Some(terminal_result),
        RunStatus::Running { graph_result, .. } | RunStatus::Stopping { graph_result, .. } => {
            graph_result.as_mut()
        }
        _ => None,
    };
    let Some(TerminalResult::Failed {
        context: Some(context),
        ..
    }) = result
    else {
        return;
    };
    for diagnostic in context
        .primary
        .iter_mut()
        .chain(&mut context.recorded)
        .chain(&mut context.verifiers)
    {
        if let Some(cursor) = &mut diagnostic.cursor
            && split_cursor(cursor).is_none()
        {
            *cursor = part_cursor(part, cursor);
        }
    }
}

#[cfg(test)]
#[path = "lineage_tests.rs"]
mod tests;
