//! Read-only local inspection. Recovery ownership belongs exclusively to control operations.
use super::*;
use crate::native_v2_observability::NativeV2Observability;
use crate::native_v2_portable_controller::availability::{ControllerAvailability, check_controller};
use crate::native_v2_cli::CliSubscriptionItem;
use openengine_cluster_protocol::{RunStatus, SubscriptionCloseReason};

struct Inspection {
    observation: NativeV2Observability,
    status: RunStatusResult,
    transport: Option<Arc<PortableControllerTransport>>,
}

impl LocalCliBackend {
    async fn read_recorded_run(&self, id: &RunId) -> Result<Inspection, NativeV2CliError> {
        let paths = self.existing_run_paths(id)?;
        // Match the retained-history reader's directory boundary, including native reparse checks.
        let _directory =
            crate::execution::platform::open_directory(paths.storage()).map_err(local_io)?;
        if !require_existing_ledger(&paths.ledger())? {
            return Err(NativeV2CliError::RunNotFound {
                run_id: id.as_str().to_owned(),
            });
        }
        let ledger =
            Arc::new(SqliteRunLedger::open_read_only(paths.ledger()).map_err(local_error)?);
        let observation = NativeV2Observability::new(ledger);
        let status = observation
            .status(RunStatusParams { run_id: id.clone() })
            .await
            .map_err(local_error)?;
        Ok(Inspection {
            observation,
            status,
            transport: None,
        })
    }

    // Control fast paths require a durable terminal, not a live runtime-failure projection.
    pub(super) async fn read_recorded_status(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2CliError> {
        Ok(self.read_recorded_run(&params.run_id).await?.status)
    }

    async fn inspect_run(&self, id: &RunId) -> Result<Inspection, NativeV2CliError> {
        let Inspection {
            observation,
            mut status,
            ..
        } = self.read_recorded_run(id).await?;
        let paths = self.existing_run_paths(id)?;
        let mut transport = None;
        if !matches!(status.status, RunStatus::Finished { .. }) {
            match check_controller(&paths, id).await {
                ControllerAvailability::Responding(connection, current) => {
                    status = *current;
                    transport = Some(connection);
                }
                ControllerAvailability::Embedded(controller)
                | ControllerAvailability::Unavailable(controller) => {
                    // The controller may have committed its terminal while the probe was in
                    // flight. Durable completion wins over an availability observation.
                    if let Ok(current) = observation
                        .status(RunStatusParams { run_id: id.clone() })
                        .await
                    {
                        status = current;
                    }
                    if !matches!(status.status, RunStatus::Finished { .. }) {
                        status.controller = Some(controller);
                    }
                }
            }
        }
        Ok(Inspection {
            observation,
            status,
            transport,
        })
    }

    pub(super) async fn inspect_status(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2CliError> {
        Ok(self.inspect_run(&params.run_id).await?.status)
    }

    pub(super) async fn inspect_watch(
        &self,
        params: RunWatchParams,
    ) -> Result<ChannelSubscription<CliRunWatchEventNotification>, NativeV2CliError> {
        let inspected = self.inspect_run(&params.run_id).await?;
        if let Some(transport) = inspected.transport {
            return Ok(spawn_watch(transport, params));
        }
        let (_, mut replay) = inspected
            .observation
            .watch(params)
            .await
            .map_err(local_error)?;
        Ok(ChannelSubscription::recorded(move |sender| async move {
            let result = async {
                while let Some(event) = replay
                    .recv_recorded(&inspected.status.at_cursor)
                    .await
                    .map_err(local_error)?
                {
                    if sender
                        .send(Ok(CliSubscriptionItem::Event(event.into())))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                finish_recorded(&sender, &inspected.status).await
            }
            .await;
            if let Err(error) = result {
                let _ = sender.send(Err(error)).await;
            }
        }))
    }

    pub(super) async fn inspect_logs(
        &self,
        params: RunLogsParams,
    ) -> Result<ChannelSubscription<RunLogEventNotification>, NativeV2CliError> {
        let inspected = self.inspect_run(&params.run_id).await?;
        if let Some(transport) = inspected.transport {
            return Ok(spawn_logs(transport, params));
        }
        let (_, mut replay) = inspected
            .observation
            .logs(params)
            .await
            .map_err(local_error)?;
        Ok(ChannelSubscription::recorded(move |sender| async move {
            let result = async {
                while let Some(event) = replay
                    .recv_recorded(&inspected.status.at_cursor)
                    .await
                    .map_err(local_error)?
                {
                    if sender
                        .send(Ok(CliSubscriptionItem::Event(event)))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                finish_recorded(&sender, &inspected.status).await
            }
            .await;
            if let Err(error) = result {
                let _ = sender.send(Err(error)).await;
            }
        }))
    }

    pub(super) async fn inspect_attach(
        &self,
        params: RunAttachParams,
    ) -> Result<ChannelSubscription<RunAttachEventNotification>, NativeV2CliError> {
        let inspected = self.inspect_run(&params.run_id).await?;
        if let Some(transport) = inspected.transport {
            return Ok(spawn_attach(transport, params));
        }
        if matches!(inspected.status.status, RunStatus::Finished { .. }) {
            return Err(local_message("execution is not active"));
        }
        let (_, mut replay) = inspected
            .observation
            .logs(RunLogsParams {
                run_id: params.run_id.clone(),
                from_cursor: None,
                execution: Some(params.execution.clone()),
            })
            .await
            .map_err(local_error)?;
        Ok(ChannelSubscription::recorded(move |sender| async move {
            let result = async {
                while let Some(log) = replay
                    .recv_recorded(&inspected.status.at_cursor)
                    .await
                    .map_err(local_error)?
                {
                    let event = RunAttachEventNotification {
                        subscription_id: log.subscription_id,
                        run_id: params.run_id.clone(),
                        execution: params.execution.clone(),
                        event: openengine_cluster_protocol::AgentAttachEvent::Output {
                            text: openengine_cluster_protocol::BoundedAssistantOutput::new(
                                log.record.message.as_str(),
                            )
                            .map_err(local_error)?,
                        },
                    };
                    if sender
                        .send(Ok(CliSubscriptionItem::Event(event)))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                finish_recorded(&sender, &inspected.status).await
            }
            .await;
            if let Err(error) = result {
                let _ = sender.send(Err(error)).await;
            }
        }))
    }

    /// Explicit controls may recover a confirmed dead controller. Inspection never calls this.
    pub(super) async fn confirm_loss_for_resume(&self, id: &RunId) -> Result<(), NativeV2CliError> {
        let status = self
            .read_recorded_status(RunStatusParams { run_id: id.clone() })
            .await;
        if status.is_ok_and(|status| matches!(status.status, RunStatus::Finished { .. })) {
            return Ok(());
        }
        let paths = self.existing_run_paths(id)?;
        if matches!(check_controller(&paths, id).await,
            ControllerAvailability::Unavailable(observed)
                if observed.state == openengine_cluster_protocol::ControllerState::Lost)
        {
            // Recovery acquires and verifies the existing lease before any ledger write.
            let controller = PortableRunController::confirm_loss(paths, id.clone())
                .await
                .map_err(local_error)?;
            drop(controller);
            return Ok(());
        }
        Err(local_message(format!(
            "controller availability is unknown; run {} is still owned or unreachable",
            id.as_str()
        )))
    }
}

async fn finish_recorded<E>(
    sender: &tokio::sync::mpsc::Sender<Result<CliSubscriptionItem<E>, NativeV2CliError>>,
    status: &RunStatusResult,
) -> Result<(), NativeV2CliError> {
    if !matches!(status.status, RunStatus::Finished { .. }) {
        return Err(controller_unavailable(status));
    }
    let _ = sender
        .send(Ok(CliSubscriptionItem::Closed {
            reason: SubscriptionCloseReason::Done,
        }))
        .await;
    Ok(())
}

pub(super) fn controller_unavailable(status: &RunStatusResult) -> NativeV2CliError {
    let state = match status.controller.as_ref().map(|value| value.state) {
        Some(openengine_cluster_protocol::ControllerState::Lost) => "lost",
        _ => "unknown or unreachable",
    };
    local_message(format!(
        "controller is {state}; recorded state is unchanged. Use `zeroshot resume {0}` or `zeroshot force-stop {0}`",
        status.run_id.as_str()
    ))
}

#[cfg(test)]
#[path = "inspection/tests.rs"]
mod tests;
