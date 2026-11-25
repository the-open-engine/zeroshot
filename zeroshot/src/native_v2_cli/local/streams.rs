//! Bridge separate local controller transports using the cursors issued by logical reads.
use super::*;
use super::super::{CliSubscription, CliSubscriptionItem};
use crate::native_v2_observability::lineage::{part_cursor, StreamSelection};
use openengine_cluster_protocol::{Cursor, RunStatus, SubscriptionCloseReason, TokenUsage};

#[async_trait]
pub(super) trait PartStream: Send + Sync + Clone + 'static {
    type Event: Send + 'static;
    const WAIT_AT_SUSPENSION: bool;
    async fn open(
        &self,
        backend: &LocalCliBackend,
    ) -> Result<ChannelSubscription<Self::Event>, NativeV2CliError>;
    fn part(&self) -> Option<&RunId>;
    fn run(&self) -> &RunId;
    fn cursor(&self) -> Option<&Cursor>;
    fn scoped(&self, part: RunId, cursor: Option<Cursor>, exact: bool) -> Self;
    fn publish(event: &mut Self::Event, root: &RunId, qualify: bool);
    fn execution_filter(&self) -> bool;
    fn add_usage(
        event: &mut Self::Event,
        previous: Option<&TokenUsage>,
    ) -> Result<(), NativeV2CliError>;
}

macro_rules! part_stream {
    ($params:ty, $event:ty, $inspect:ident, $wait:literal, $filter:expr, $usage:expr $(, $status:ident)?) => {
        #[async_trait]
        impl PartStream for $params {
            type Event = $event;
            const WAIT_AT_SUSPENSION: bool = $wait;
            async fn open(
                &self,
                backend: &LocalCliBackend,
            ) -> Result<ChannelSubscription<Self::Event>, NativeV2CliError> {
                backend.$inspect(self.clone()).await
            }
            fn part(&self) -> Option<&RunId> {
                self.part_id.as_ref()
            }
            fn run(&self) -> &RunId {
                &self.run_id
            }
            fn cursor(&self) -> Option<&Cursor> {
                self.from_cursor.as_ref()
            }
            fn scoped(&self, part: RunId, cursor: Option<Cursor>, exact: bool) -> Self {
                let mut params = self.clone();
                params.part_id = exact.then(|| part.clone());
                params.run_id = part;
                params.from_cursor = cursor;
                params
            }
            fn publish(event: &mut Self::Event, root: &RunId, qualify: bool) {
                event.run_id = root.clone();
                if qualify {
                    event.cursor = part_cursor(&event.part_id, &event.cursor);
                    $(if let super::super::CliRunStatus::Target(status) = &mut event.$status {
                        crate::native_v2_observability::lineage::scope_diagnostic_cursors(&event.part_id, status);
                    })?
                }
            }
            fn execution_filter(&self) -> bool {
                ($filter)(self)
            }
            fn add_usage(
                event: &mut Self::Event,
                previous: Option<&TokenUsage>,
            ) -> Result<(), NativeV2CliError> {
                ($usage)(event, previous)
            }
        }
    };
}

part_stream!(
    RunWatchParams,
    CliRunWatchEventNotification,
    inspect_watch,
    true,
    |_: &RunWatchParams| false,
    |event: &mut CliRunWatchEventNotification, previous| {
        if let super::super::CliRunStatus::Target(
            RunStatus::Finished { metadata, .. } | RunStatus::Suspended { metadata, .. },
        ) = &mut event.status
        {
            crate::v2_run_ledger::merge_usage(&mut metadata.token_usage, previous)?;
        }
        Ok(())
    },
    status
);
part_stream!(
    RunLogsParams,
    RunLogEventNotification,
    inspect_logs,
    false,
    |p: &RunLogsParams| p.execution.is_some(),
    |_, _| Ok(())
);

struct StreamProjection {
    root: RunId,
    qualify: bool,
    previous: Option<TokenUsage>,
}

/// Forward events until a durable end-of-part marker. A disconnect goes back to
/// the caller's reconnect loop with its last delivered cursor.
async fn forward_part<P: PartStream>(
    stream: &mut ChannelSubscription<P::Event>,
    sender: &tokio::sync::mpsc::Sender<Result<CliSubscriptionItem<P::Event>, NativeV2CliError>>,
    projection: &StreamProjection,
) -> Result<bool, NativeV2CliError> {
    while let Some(item) = stream.next().await? {
        match item {
            CliSubscriptionItem::Event(mut event) => {
                P::add_usage(&mut event, projection.previous.as_ref())?;
                P::publish(&mut event, &projection.root, projection.qualify);
                if sender
                    .send(Ok(CliSubscriptionItem::Event(event)))
                    .await
                    .is_err()
                {
                    return Ok(false);
                }
            }
            CliSubscriptionItem::Closed {
                reason: SubscriptionCloseReason::Done,
            } => return Ok(true),
            other => {
                let _ = sender.send(Ok(other)).await;
                return Ok(false);
            }
        }
    }
    Ok(false)
}

impl LocalCliBackend {
    pub(super) async fn run_stream<P: PartStream>(
        &self,
        params: P,
    ) -> Result<ChannelSubscription<P::Event>, NativeV2CliError> {
        let chain = self.lineage(params.run()).await?;
        let (mut part, after) = chain.stream_position(
            params.run(),
            StreamSelection::new(params.part(), params.execution_filter()),
            params.cursor().cloned(),
        )?;
        let aggregate = P::WAIT_AT_SUSPENSION && params.part().is_none();
        let previous = if aggregate {
            self.previous_usage(&chain, &part).await?
        } else {
            None
        };
        let follow = params.part().is_none() && !params.execution_filter();
        let mut stream = params
            .scoped(part.clone(), after, !follow)
            .open(self)
            .await?;
        let mut projection = StreamProjection {
            root: chain.root().clone(),
            qualify: params.part().is_none(),
            previous,
        };
        let backend = self.clone();
        Ok(ChannelSubscription::recorded(move |sender| async move {
            let outcome = async {
                loop {
                    if !forward_part::<P>(&mut stream, &sender, &projection).await? {
                        return Ok(());
                    }
                    let next = if follow {
                        backend
                            .next_stream_part(&part, P::WAIT_AT_SUSPENSION)
                            .await?
                    } else {
                        None
                    };
                    let Some(next) = next else {
                        let _ = sender
                            .send(Ok(CliSubscriptionItem::Closed {
                                reason: SubscriptionCloseReason::Done,
                            }))
                            .await;
                        return Ok(());
                    };
                    if aggregate {
                        backend
                            .accumulate_part_usage(&part, &mut projection.previous)
                            .await?;
                    }
                    part = next;
                    stream = params
                        .scoped(part.clone(), None, false)
                        .open(&backend)
                        .await?;
                }
            }
            .await;
            if let Err(error) = outcome {
                let _ = sender.send(Err(error)).await;
            }
        }))
    }

    pub(super) async fn next_stream_part(
        &self,
        part: &RunId,
        wait: bool,
    ) -> Result<Option<RunId>, NativeV2CliError> {
        loop {
            if let Some(next) = self
                .part_recovery(part.clone())
                .await?
                .and_then(|record| record.successor_run_id)
                && self
                    .part_recovery(next.clone())
                    .await?
                    .and_then(|record| record.resumed_from)
                    .as_ref()
                    == Some(part)
            {
                return Ok(Some(next));
            }
            if !wait {
                return Ok(None);
            }
            let status = self
                .read_recorded_status(RunStatusParams::new(part.clone()))
                .await?;
            if !matches!(status.status, RunStatus::Suspended { .. }) {
                return Ok(None);
            }
            sleep(Duration::from_millis(300)).await;
        }
    }
}
