//! Minimal status fallback for a controller-owned runtime that cannot persist its own failure.
//! It never creates a durable event or advances a cursor. Only active runtime identities are seeded;
//! normal completion removes them, while a failed runtime remains observable for this controller.

use super::*;
use openengine_cluster_protocol::{EnumLabel, FailureContext, FailureDiagnostic, TerminalResult};

#[derive(Clone, Default)]
pub(super) struct RuntimeObservation {
    entries: Arc<Mutex<BTreeMap<RunId, RuntimeStatus>>>,
}

struct RuntimeStatus {
    last: RunStatusResult,
    metadata: RunMetadata,
    failed: bool,
    context: Option<Box<FailureContext>>,
}

impl RuntimeObservation {
    pub(super) fn track(&self, snapshot: &RunSnapshot) -> Result<(), NativeV2ObservationError> {
        let last = status_result(snapshot)?;
        self.entries().insert(
            last.run_id.clone(),
            RuntimeStatus {
                last,
                metadata: RunMetadata {
                    token_usage: snapshot.token_usage.clone(),
                },
                failed: false,
                context: snapshot.failure_context.clone(),
            },
        );
        Ok(())
    }

    pub(super) fn observe(
        &self,
        snapshot: &RunSnapshot,
    ) -> Result<RunStatusResult, NativeV2ObservationError> {
        let current = status_result(snapshot)?;
        let mut entries = self.entries();
        let Some(runtime) = entries.get_mut(&current.run_id) else {
            return Ok(current);
        };
        if cursor_sequence(&current.at_cursor)? >= cursor_sequence(&runtime.last.at_cursor)? {
            let failure = runtime.failed.then(|| runtime.last.status.clone());
            runtime.last = current;
            runtime.metadata.token_usage = snapshot.token_usage.clone();
            runtime.context = snapshot.failure_context.clone();
            if snapshot.terminal.is_none()
                && let Some(RunStatus::Finished {
                    mut terminal_result,
                    ..
                }) = failure
            {
                refresh_failure_context(&mut terminal_result, runtime.context.as_deref());
                runtime.last.status = RunStatus::Finished {
                    metadata: runtime.metadata.clone(),
                    terminal_result,
                };
            }
        }
        Ok(runtime.last.clone())
    }

    pub(super) fn fail(&self, run_id: &RunId, cause: Option<&str>) {
        if let Some(runtime) = self.entries().get_mut(run_id) {
            runtime.failed = true;
            if !matches!(runtime.last.status, RunStatus::Finished { .. }) {
                let context = unpersisted_context(runtime.context.clone(), cause);
                runtime.last.status = RunStatus::Finished {
                    metadata: runtime.metadata.clone(),
                    terminal_result: TerminalResult::Failed {
                        context,
                        reason: EnumLabel::new("runtime_failed")
                            .expect("static runtime failure reason"),
                    },
                };
            }
        }
    }

    pub(super) fn failure(&self, run_id: &RunId) -> Option<RunStatusResult> {
        self.entries()
            .get(run_id)
            .filter(|runtime| runtime.failed)
            .map(|runtime| runtime.last.clone())
    }

    pub(super) fn observe_finished(
        &self,
        snapshot: &RunSnapshot,
    ) -> Result<bool, NativeV2ObservationError> {
        self.observe(snapshot)?;
        Ok(snapshot.terminal.is_some() || self.failure(&snapshot.run_id).is_some())
    }

    pub(super) fn finish(&self, run_id: &RunId) {
        self.entries().remove(run_id);
    }

    fn entries(&self) -> MutexGuard<'_, BTreeMap<RunId, RuntimeStatus>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn refresh_failure_context(terminal: &mut TerminalResult, current: Option<&FailureContext>) {
    let TerminalResult::Failed {
        context: previous, ..
    } = terminal
    else {
        return;
    };
    let Some(current) = current else {
        return;
    };
    let mut refreshed = current.clone();
    if let Some(previous) = previous {
        refreshed.primary = previous.primary.clone().or(refreshed.primary);
        refreshed.incomplete |= previous.incomplete;
    }
    *previous = Some(Box::new(refreshed));
}

fn unpersisted_context(
    context: Option<Box<FailureContext>>,
    cause: Option<&str>,
) -> Option<Box<FailureContext>> {
    let Some(cause) = cause else {
        return context;
    };
    let mut context = context.unwrap_or_default();
    if context.primary.is_none() {
        let message = crate::native_v2_runner::bounded_log_text(
            format!("Runtime failed: {cause}"),
            crate::v2_run_ledger::MAX_SAFE_LOG_BYTES / 2,
        );
        context.primary = Some(FailureDiagnostic {
            message: BoundedLogMessage::new(crate::native_v2_runner::bounded_log_text(
                visible_text(&message),
                openengine_cluster_protocol::MAX_LOG_MESSAGE_BYTES,
            ))
            .unwrap_or_else(|_| BoundedLogMessage::redacted()),
            execution: None,
            cursor: None,
        });
    }
    context.incomplete = true;
    Some(context)
}
