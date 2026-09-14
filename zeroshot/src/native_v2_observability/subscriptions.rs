use super::*;

pub struct RunWatchSubscription {
    pub(super) ledger: Arc<dyn RunLedger>,
    pub(super) runtime: RuntimeObservation,
    pub(super) subscription_id: SubscriptionId,
    pub(super) run_id: RunId,
    pub(super) scanned_through: Cursor,
    pub(super) projection: RunSnapshot,
    pub(super) after: u64,
    pub(super) pending: VecDeque<RunWatchEventNotification>,
}

impl RunWatchSubscription {
    async fn refresh(&mut self) -> Result<ReplayProgress, NativeV2ObservationError> {
        let tail = self
            .ledger
            .snapshot_and_tail(&self.run_id, Some(&self.scanned_through))
            .await?;
        WatchFold {
            subscription_id: &self.subscription_id,
            after: self.after,
            projection: &mut self.projection,
            pending: &mut self.pending,
        }
        .apply(&tail.events)?;
        replay_progress(&self.runtime, &tail, &mut self.scanned_through)
    }
}

pub struct RunLogsSubscription {
    pub(super) ledger: Arc<dyn RunLedger>,
    pub(super) runtime: RuntimeObservation,
    pub(super) subscription_id: SubscriptionId,
    pub(super) run_id: RunId,
    pub(super) execution: Option<ExecutionId>,
    pub(super) scanned_through: Cursor,
    pub(super) pending: VecDeque<RunLogEventNotification>,
}

impl RunLogsSubscription {
    async fn refresh(&mut self) -> Result<ReplayProgress, NativeV2ObservationError> {
        let tail = self
            .ledger
            .snapshot_and_tail(&self.run_id, Some(&self.scanned_through))
            .await?;
        for stored in &tail.events {
            if let Some(notification) = log_notification(
                &self.subscription_id,
                &tail.snapshot,
                self.execution,
                stored,
            )? {
                self.pending.push_back(notification);
            }
        }
        replay_progress(&self.runtime, &tail, &mut self.scanned_through)
    }
}

struct ReplayProgress {
    caught_up: bool,
    finished: bool,
}

fn replay_progress(
    runtime: &RuntimeObservation,
    tail: &crate::v2_run_ledger::SnapshotAndTail,
    scanned_through: &mut Cursor,
) -> Result<ReplayProgress, NativeV2ObservationError> {
    if let Some(last) = tail.events.last() {
        *scanned_through = last.cursor.clone();
    }
    let caught_up = *scanned_through == tail.snapshot.cursor;
    let finished = runtime.observe_finished(&tail.snapshot)? && caught_up;
    Ok(ReplayProgress {
        caught_up,
        finished,
    })
}

#[async_trait]
trait DurableSubscription {
    type Notification: Send;

    fn pending(&mut self) -> &mut VecDeque<Self::Notification>;
    async fn refresh_subscription(&mut self) -> Result<ReplayProgress, NativeV2ObservationError>;
}

macro_rules! durable_subscription {
    ($subscription:ty, $notification:ty) => {
        impl $subscription {
            /// Returns one bounded batch of currently durable notifications.
            pub async fn read_available(
                &mut self,
            ) -> Result<Vec<$notification>, NativeV2ObservationError> {
                if self.pending.is_empty() {
                    self.refresh().await?;
                }
                Ok(self.pending.drain(..).collect())
            }

            /// Waits for a durable notification. Dropping an observer cannot stop execution.
            pub async fn recv(
                &mut self,
            ) -> Result<Option<$notification>, NativeV2ObservationError> {
                recv_durable(self).await
            }
        }

        #[async_trait]
        impl DurableSubscription for $subscription {
            type Notification = $notification;

            fn pending(&mut self) -> &mut VecDeque<Self::Notification> {
                &mut self.pending
            }

            async fn refresh_subscription(
                &mut self,
            ) -> Result<ReplayProgress, NativeV2ObservationError> {
                self.refresh().await
            }
        }
    };
}

durable_subscription!(RunWatchSubscription, RunWatchEventNotification);
durable_subscription!(RunLogsSubscription, RunLogEventNotification);

async fn recv_durable<S>(
    subscription: &mut S,
) -> Result<Option<S::Notification>, NativeV2ObservationError>
where
    S: DurableSubscription + Send,
{
    loop {
        if let Some(event) = subscription.pending().pop_front() {
            return Ok(Some(event));
        }
        let progress = subscription.refresh_subscription().await?;
        if let Some(event) = subscription.pending().pop_front() {
            return Ok(Some(event));
        }
        if progress.finished {
            return Ok(None);
        }
        if progress.caught_up {
            tokio::time::sleep(POLL_INTERVAL).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
}

pub struct RunAttachSubscription {
    pub(super) subscription_id: SubscriptionId,
    pub(super) run_id: RunId,
    pub(super) execution: PublicExecutionRef,
    pub(super) initial_working: bool,
    pub(super) settled: bool,
    pub(super) receiver: ReadOnlyAttach,
}

impl RunAttachSubscription {
    pub async fn recv(&mut self) -> Result<RunAttachEventNotification, NativeV2ObservationError> {
        let event = if self.initial_working {
            self.initial_working = false;
            AgentAttachEvent::Working {}
        } else {
            match self.receiver.recv().await {
                Ok(output) => AgentAttachEvent::Output {
                    text: bounded_attach_output(&output.text),
                },
                Err(AttachReceiveError::Closed) if !self.settled => {
                    self.settled = true;
                    AgentAttachEvent::Settled {}
                }
                Err(AttachReceiveError::Closed) => {
                    return Err(NativeV2ObservationError::AttachClosed);
                }
                Err(AttachReceiveError::Lagged) => {
                    return Err(NativeV2ObservationError::AttachLagged);
                }
            }
        };
        Ok(RunAttachEventNotification {
            subscription_id: self.subscription_id.clone(),
            run_id: self.run_id.clone(),
            execution: self.execution.clone(),
            event,
        })
    }
}
