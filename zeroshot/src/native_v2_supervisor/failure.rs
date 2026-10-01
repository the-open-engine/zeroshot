use super::*;

impl FinishedDispatch {
    pub(super) fn completion_events(
        self,
        force: bool,
    ) -> Result<Vec<RunEvent>, NativeV2SupervisorError> {
        let mut events = if force {
            self.failure_log()?.into_iter().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let cause = settled_failure_cause(&self.result, force);
        let outcome = settled_outcome(&self.reference, self.result, force)?;
        events.extend(super::logging::completion_events(
            NodeCompletion {
                reference: self.reference,
                outcome,
            },
            Some(self.elapsed),
            cause.as_deref(),
        )?);
        Ok(events)
    }

    /// Preserve actual errors even when graph control voids or interrupts this execution.
    pub(super) fn failure_log(&self) -> Result<Option<RunEvent>, RunLedgerError> {
        let outcome = match &self.result {
            DispatchResult::Completed(Ok(completion))
                if completion.reference == self.reference
                    && completion.outcome.error_code().is_some() =>
            {
                completion.outcome.clone()
            }
            DispatchResult::Completed(Err(
                NodeRunnerError::Cancelled | NodeRunnerError::RunClosed,
            ))
            | DispatchResult::Interrupted => return Ok(None),
            DispatchResult::Completed(Err(error)) => runner_failure(error),
            DispatchResult::TimedOut => WorkerOutcome::declared_failure(WorkerErrorCode::Timeout),
            DispatchResult::Completed(Ok(_))
            | DispatchResult::DurableEventFailure(_)
            | DispatchResult::StartFailure(_) => return Ok(None),
        };
        super::logging::node_failure_log(
            &self.reference,
            &outcome,
            Some(self.elapsed),
            settled_failure_cause(&self.result, false).as_deref(),
        )
    }
}

pub(super) fn preserve_interrupted_failure(
    interrupted: Option<DispatchResult>,
    result: DispatchResult,
) -> DispatchResult {
    // A graph void or run stop still owns settlement. Retain a real error returned while
    // cancellation drains so that this authority cannot erase the user's failure evidence.
    // Deadlines keep their original timeout priority.
    if matches!(interrupted, Some(DispatchResult::Interrupted)) && completed_failure(&result) {
        return result;
    }
    interrupted.unwrap_or(result)
}

fn completed_failure(result: &DispatchResult) -> bool {
    match result {
        DispatchResult::Completed(Ok(completion)) => completion.outcome.error_code().is_some(),
        DispatchResult::Completed(Err(error)) => !matches!(
            error,
            NodeRunnerError::Cancelled | NodeRunnerError::RunClosed
        ),
        _ => false,
    }
}
