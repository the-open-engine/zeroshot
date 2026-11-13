use super::*;
use openengine_cluster_protocol::{RunSuspension, RunVectorContent, RunWait};

impl NativeV2Supervisor {
    pub(super) async fn suspend_at(
        &self,
        suspension: RunSuspension,
    ) -> Result<(), NativeV2SupervisorError> {
        if self.force_requested.load(Ordering::Acquire)
            || self.snapshot().await?.force_stop_requested
        {
            return Ok(());
        }
        self.runner.close_run(&self.run_id).await;
        let events = vec![
            crate::v2_run_ledger::vector_event(
                &self.run_id,
                RunVectorContent::Suspension {
                    reason: if suspension.due_at().is_some() {
                        openengine_cluster_protocol::VectorSuspensionReason::Wait
                    } else {
                        openengine_cluster_protocol::VectorSuspensionReason::Command
                    },
                    node: suspension.node().clone(),
                },
            )?,
            RunEvent::SafeLog {
                execution: None,
                timestamp: (self.wall_clock)(),
                stream: SafeLogStream::System,
                line: SafeLogLine::new(format!("run suspended at {}", suspension.node().as_str()))?,
                diagnostic: None,
            },
            RunEvent::Suspended { suspension },
        ];
        // The host's store acknowledges both the complete graph state and this suspension's
        // vector before cleanup. The final ledger event remains the durable end of the attempt.
        let mut vectors = Vec::new();
        let mut after = None;
        loop {
            let page = crate::v2_run_ledger::vectors::list(
                self.ledger.as_ref(),
                openengine_cluster_protocol::RunVectorsParams {
                    run_id: self.run_id.clone(),
                    after,
                    limit: Some(100),
                },
            )
            .await?;
            vectors.extend(page.vectors);
            after = page.next_after;
            if after.is_none() {
                break;
            }
        }
        if let RunEvent::Vector { vector } = &events[0] {
            vectors.push(vector.clone());
        }
        self.checkpoints
            .as_ref()
            .ok_or(NativeV2SupervisorError::InvalidState)?
            .suspend(&continuation_history(&self.snapshot().await?)?, &vectors)
            .await?;
        self.cleanup_runtime(RunRuntimeExit::Suspended).await?;
        match self.ledger.append(&self.run_id, events).await {
            Ok(_) => Err(NativeV2SupervisorError::Suspended),
            Err(_) if self.force_requested.load(Ordering::Acquire) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) async fn execute_wait(
        &self,
        dispatch: &Dispatch,
    ) -> Result<(), NativeV2SupervisorError> {
        if !self
            .record_dispatches(std::slice::from_ref(dispatch))
            .await?
        {
            return Ok(());
        }
        let now = (self.wall_clock)();
        let due_at = match crate::native_v2_wait::deadline(&dispatch.input, now) {
            Ok(due) => due,
            Err(reason) => {
                self.ledger
                    .append(
                        &self.run_id,
                        vec![
                            RunEvent::SafeLog {
                                execution: Some(dispatch.reference.execution),
                                timestamp: now,
                                stream: SafeLogStream::Error,
                                line: SafeLogLine::new(format!(
                                    "wait {} refused: {reason}",
                                    dispatch.reference.node.as_str()
                                ))?,
                                diagnostic: None,
                            },
                            RunEvent::NodeCompleted {
                                completion: NodeCompletion {
                                    reference: dispatch.reference.clone(),
                                    outcome: WorkerOutcome::declared_failure(
                                        WorkerErrorCode::Refusal,
                                    ),
                                },
                            },
                        ],
                    )
                    .await?;
                return Ok(());
            }
        };
        self.ledger
            .append(
                &self.run_id,
                vec![
                    crate::v2_run_ledger::vector_event(
                        &self.run_id,
                        RunVectorContent::Wait {
                            node: dispatch.reference.node.clone(),
                            due_at,
                        },
                    )?,
                    RunEvent::SafeLog {
                        execution: Some(dispatch.reference.execution),
                        timestamp: now,
                        stream: SafeLogStream::System,
                        line: SafeLogLine::new(format!(
                            "wait {} due at {}",
                            dispatch.reference.node.as_str(),
                            due_at.get()
                        ))?,
                        diagnostic: None,
                    },
                ],
            )
            .await?;
        let mut suspend = self.suspend_signal.subscribe();
        let mut stop = self.resolution_stop.subscribe();
        loop {
            if *stop.borrow() {
                return Ok(());
            }
            let remaining = due_at.get().saturating_sub((self.wall_clock)().get());
            if remaining == 0 {
                return self.complete_wait(dispatch).await;
            }
            if self
                .wait_threshold
                .is_some_and(|threshold| Duration::from_millis(remaining) >= threshold)
                || *suspend.borrow()
                || self.snapshot().await?.suspend_requested
            {
                self.ledger
                    .append(
                        &self.run_id,
                        vec![RunEvent::WaitDeferred {
                            reference: dispatch.reference.clone(),
                        }],
                    )
                    .await?;
                self.suspend_at(RunSuspension::Wait {
                    node: dispatch.reference.node.clone(),
                    created_at: (self.wall_clock)(),
                    wait: RunWait { due_at },
                })
                .await?;
                return Ok(());
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(remaining.min(60_000))) => {},
                _ = suspend.changed() => {},
                _ = stop.changed() => {},
            }
        }
    }

    async fn complete_wait(&self, dispatch: &Dispatch) -> Result<(), NativeV2SupervisorError> {
        self.ledger
            .append(
                &self.run_id,
                vec![RunEvent::NodeCompleted {
                    completion: NodeCompletion {
                        reference: dispatch.reference.clone(),
                        outcome: crate::native_v2_wait::elapsed()
                            .map_err(|_| NativeV2SupervisorError::InvalidState)?,
                    },
                }],
            )
            .await?;
        Ok(())
    }
}
