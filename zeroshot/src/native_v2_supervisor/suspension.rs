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
        let mut events = vec![
            crate::v2_run_ledger::vector_event(
                &self.run_id,
                RunVectorContent::Suspension {
                    reason: if matches!(suspension, RunSuspension::Request { .. }) {
                        openengine_cluster_protocol::VectorSuspensionReason::Request
                    } else if suspension.due_at().is_some() {
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
            RunEvent::Suspended {
                suspension: suspension.clone(),
            },
        ];
        if let RunSuspension::Request {
            node,
            created_at,
            request,
        } = &suspension
        {
            events.insert(
                0,
                RunEvent::Vector {
                    vector: openengine_cluster_protocol::RunVector {
                        id: request.id.clone(),
                        run_id: self.run_id.clone(),
                        created_at: *created_at,
                        content: RunVectorContent::Request {
                            node: node.clone(),
                            text: request.text.clone(),
                            signals: request.signals.clone(),
                            output: request.output.clone(),
                            expires_at: request.expires_at,
                        },
                    },
                },
            );
        }
        // Keep admission live during host I/O. The first suspend request is the only concurrent
        // vector append here; republish if it arrived after this snapshot was taken.
        loop {
            let snapshot = self.snapshot().await?;
            let mut vectors = Vec::new();
            let mut after = None;
            loop {
                let page = crate::v2_run_ledger::vectors::list_full(
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
            vectors.extend(events.iter().filter_map(|event| match event {
                RunEvent::Vector { vector } => Some(vector.clone()),
                _ => None,
            }));
            self.checkpoints
                .as_ref()
                .ok_or(NativeV2SupervisorError::InvalidState)?
                .suspend(&continuation_history(&snapshot)?, &vectors)
                .await?;
            // Fence new requests through disposal and the durable end of the attempt, so the
            // acknowledged host record remains complete. Force-stop stays independent.
            let _turn = self.suspend_turn.lock().await;
            if self.snapshot().await?.suspend_requested != snapshot.suspend_requested {
                continue;
            }
            self.cleanup_runtime(RunRuntimeExit::Suspended).await?;
            return match self.ledger.append(&self.run_id, events).await {
                Ok(_) => Err(NativeV2SupervisorError::Suspended),
                Err(_) if self.force_requested.load(Ordering::Acquire) => Ok(()),
                Err(error) => Err(error.into()),
            };
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

impl NativeV2Supervisor {
    pub(super) async fn execute_request(
        &self,
        program: &RunProgram,
        dispatch: &Dispatch,
    ) -> Result<(), NativeV2SupervisorError> {
        if !self
            .record_dispatches(std::slice::from_ref(dispatch))
            .await?
        {
            return Ok(());
        }
        let node =
            crate::native_v2_request::find(&program.admitted.graph.root, &dispatch.reference.node)
                .ok_or(NativeV2SupervisorError::InvalidState)?;
        let created_at = (self.wall_clock)();
        let expires_at = crate::native_v2_request::deadline(
            created_at,
            node.timeout_ms.map(|timeout| timeout.get()),
        )
        .map_err(|_| NativeV2SupervisorError::InvalidState)?;
        let text = dispatch
            .input
            .get("text")
            .and_then(Value::as_str)
            .ok_or(NativeV2SupervisorError::InvalidState)?
            .to_owned();
        let request = openengine_cluster_protocol::RunRequest {
            id: openengine_cluster_protocol::VectorId::new(uuid::Uuid::now_v7().to_string())
                .map_err(|_| NativeV2SupervisorError::InvalidState)?,
            text,
            signals: node.signals.clone(),
            output: node.output.clone(),
            expires_at,
        };
        self.ledger
            .append(
                &self.run_id,
                vec![RunEvent::RequestDeferred {
                    reference: dispatch.reference.clone(),
                }],
            )
            .await?;
        self.suspend_at(RunSuspension::Request {
            node: node.name.clone(),
            created_at,
            request: Box::new(request),
        })
        .await
    }
}
