use super::*;

pub struct RunWatchSubscription {
    pub(super) follow: Option<RunFollow>,
    pub(super) ledger: Arc<dyn RunLedger>,
    pub(super) runtime: RuntimeObservation,
    pub(super) subscription_id: SubscriptionId,
    pub(super) run_id: RunId,
    pub(super) scanned_through: Cursor,
    pub(super) projection: RunSnapshot,
    /// Earlier parts are merged into published metadata, independently of this part's replay.
    pub(super) previous_usage: Option<TokenUsage>,
    pub(super) after: u64,
    pub(super) pending: VecDeque<RunWatchEventNotification>,
}

impl RunWatchSubscription {
    async fn refresh(&mut self) -> Result<ReplayProgress, NativeV2ObservationError> {
        let tail = self
            .ledger
            .snapshot_and_tail(&self.run_id, Some(&self.scanned_through))
            .await?;
        self.enqueue(&tail.events)?;
        replay_progress(&self.runtime, &tail, &mut self.scanned_through)
    }
}

pub struct RunLogsSubscription {
    pub(super) follow: Option<RunFollow>,
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
    suspended: bool,
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
        suspended: tail.snapshot.phase == RunPhase::Suspended,
    })
}

#[async_trait]
trait DurableSubscription {
    type Notification: Send;

    fn pending(&mut self) -> &mut VecDeque<Self::Notification>;
    async fn refresh_subscription(&mut self) -> Result<ReplayProgress, NativeV2ObservationError>;
}

macro_rules! durable_subscription {
    ($subscription:ty, $notification:ty $(, $status:ident)?) => {
        impl $subscription {
            /// Replays only the head captured by a read-only local inspector, without waiting
            /// for a controller or inventing a terminal event.
            pub(crate) async fn recv_recorded(
                &mut self,
                through: &Cursor,
            ) -> Result<Option<$notification>, NativeV2ObservationError> {
                let head = cursor_sequence(through)?;
                loop {
                    if let Some(event) = self.pending.pop_front() {
                        return Ok((cursor_sequence(&event.cursor)? <= head).then_some(event));
                    }
                    if cursor_sequence(&self.scanned_through)? >= head {
                        return Ok(None);
                    }
                    self.refresh().await?;
                }
            }

            /// Returns one bounded batch of currently durable notifications.
            pub async fn read_available(
                &mut self,
            ) -> Result<Vec<$notification>, NativeV2ObservationError> {
                if self.pending.is_empty() {
                    self.refresh_subscription().await?;
                }
                let mut events: Vec<_> = self.pending.drain(..).collect();
                for event in &mut events {
                    self.project_identity(event);
                }
                Ok(events)
            }

            /// Waits for a durable notification. Dropping an observer cannot stop execution.
            pub async fn recv(
                &mut self,
            ) -> Result<Option<$notification>, NativeV2ObservationError> {
                let mut event = recv_durable(self).await?;
                if let Some(event) = &mut event {
                    self.project_identity(event);
                }
                Ok(event)
            }

            fn project_identity(&self, event: &mut $notification) {
                if let Some(scope) = &self.follow {
                    event.run_id = scope.root.clone();
                    if scope.qualify {
                        event.cursor = lineage::part_cursor(&event.part_id, &event.cursor);
                        $(lineage::scope_diagnostic_cursors(&event.part_id, &mut event.$status);)?
                    }
                }
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
                let mut progress = self.refresh().await?;
                if progress.finished
                    && self.pending.is_empty()
                    && let Some(scope) = &self.follow
                {
                    if let Some(next) = scope.advance(&self.run_id).await? {
                        self.enter_part(next).await?;
                        progress.finished = false;
                        progress.caught_up = false;
                    } else if progress.suspended && scope.follows_suspensions() {
                        progress.finished = false;
                    }
                }
                Ok(progress)
            }
        }
    };
}

durable_subscription!(RunWatchSubscription, RunWatchEventNotification, status);
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
            part_id: self.run_id.clone(),
            run_id: self.run_id.clone(),
            execution: self.execution.clone(),
            event,
        })
    }
}
