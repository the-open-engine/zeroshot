//! Minimal single-run `ObservationStore`/`ClusterBackend` fixture for exercising the watch port
//! contract independent of `openengine-cluster-testkit`'s production-shaped
//! `InMemoryAdmissionStore`. Shared by this crate's own integration tests and by
//! `openengine-cluster-client`'s reconnect tests so both exercise identical
//! subscribe/replay/overflow semantics against one implementation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ClusterStatus, Cursor, GetParams, GetResult, InitializeParams, InitializeResult, RunId,
    ServerCapabilities, SubscriptionId, WatchEvent, WatchParams, WatchResult,
};
use tokio::sync::{mpsc, Mutex};

use super::{
    subscribe_and_stream, ObservationStore, PublicEventRecord, ReplayPageRequest,
    ResolvedSubscription, SubscribeRequest, WatchEventStream, WatchHandle,
};
use crate::admission::StoreError;
use crate::{BackendError, ClusterBackend, ConnectionContext};

#[derive(Default)]
struct FixtureState {
    history: Vec<PublicEventRecord>,
    live: Vec<(mpsc::Sender<PublicEventRecord>, Arc<AtomicBool>)>,
}

/// A single run's durable history plus live subscriber fan-out. Every subscription registered
/// against this store is bounded by `capacity` (fixed at construction), irrespective of the
/// `queue_capacity` a caller passes to [`ObservationStore::subscribe`] — this lets a test force a
/// deterministic overflow regardless of which layer (a production `Dispatcher`'s hardcoded
/// default, or an explicit direct backend call) established the subscription.
pub struct FixtureStore {
    run_id: RunId,
    capacity: usize,
    state: Mutex<FixtureState>,
}

impl FixtureStore {
    #[must_use]
    pub fn new(run_id: RunId, seed: Vec<WatchEvent>, capacity: usize) -> Self {
        let history = seed
            .into_iter()
            .enumerate()
            .map(|(index, event)| PublicEventRecord {
                run_id: run_id.clone(),
                cursor: Cursor::new(format!("cursor-{}", index + 1)),
                event,
            })
            .collect();
        Self {
            run_id,
            capacity: capacity.max(1),
            state: Mutex::new(FixtureState {
                history,
                live: Vec::new(),
            }),
        }
    }

    pub async fn publish(&self, event: WatchEvent) -> Cursor {
        let mut state = self.state.lock().await;
        let cursor = Cursor::new(format!("cursor-{}", state.history.len() + 1));
        let record = PublicEventRecord {
            run_id: self.run_id.clone(),
            cursor: cursor.clone(),
            event,
        };
        state.history.push(record.clone());
        fan_out(&mut state, &record);
        cursor
    }

    /// Redelivers the last recorded event to every live subscriber without allocating a new
    /// cursor, simulating a legal at-least-once physical retry.
    pub async fn republish_last(&self) {
        let mut state = self.state.lock().await;
        let Some(record) = state.history.last().cloned() else {
            return;
        };
        fan_out(&mut state, &record);
    }

    pub async fn history_cursors(&self) -> Vec<Cursor> {
        self.state
            .lock()
            .await
            .history
            .iter()
            .map(|record| record.cursor.clone())
            .collect()
    }
}

fn fan_out(state: &mut FixtureState, record: &PublicEventRecord) {
    state.live.retain(
        |(sender, overflowed)| match sender.try_send(record.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                overflowed.store(true, Ordering::Release);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        },
    );
}

#[async_trait]
impl ObservationStore for FixtureStore {
    async fn subscribe(
        &self,
        request: SubscribeRequest,
        _queue_capacity: usize,
    ) -> Result<ResolvedSubscription, StoreError> {
        if let Some(run_id) = &request.run_id {
            if run_id != &self.run_id {
                return Err(StoreError::UnknownRun);
            }
        }
        let mut state = self.state.lock().await;
        let replay_through = state.history.last().map(|record| record.cursor.clone());
        let (sender, receiver) = mpsc::channel(self.capacity);
        let overflowed = Arc::new(AtomicBool::new(false));
        state.live.push((sender, Arc::clone(&overflowed)));
        Ok(ResolvedSubscription {
            run_id: Some(self.run_id.clone()),
            at_cursor: replay_through.clone(),
            resume_after: request.from_cursor,
            replay_through,
            receiver,
            overflowed,
        })
    }

    async fn replay_page(
        &self,
        request: ReplayPageRequest<'_>,
    ) -> Result<Vec<PublicEventRecord>, StoreError> {
        if request.run_id != &self.run_id {
            return Err(StoreError::UnknownRun);
        }
        let state = self.state.lock().await;
        let start = match request.after {
            Some(after) => state
                .history
                .iter()
                .position(|record| &record.cursor == after)
                .map_or(0, |index| index + 1),
            None => 0,
        };
        let mut page = Vec::new();
        for record in state.history.iter().skip(start) {
            let reached_through = &record.cursor == request.through;
            page.push(record.clone());
            if page.len() >= request.limit || reached_through {
                break;
            }
        }
        Ok(page)
    }
}

/// Wraps a [`FixtureStore`] as a minimal [`ClusterBackend`]; `initialize`/`get` return an empty
/// status since these fixtures exist only to exercise `watch`.
pub struct FixtureBackend {
    pub store: Arc<FixtureStore>,
}

#[async_trait]
impl ClusterBackend for FixtureBackend {
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        Ok(InitializeResult::new(
            ServerCapabilities::default(),
            ClusterStatus::empty(),
        ))
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        _params: GetParams,
    ) -> Result<GetResult, BackendError> {
        Ok(GetResult {
            spec: None,
            status: ClusterStatus::empty(),
            at_cursor: None,
        })
    }

    async fn watch(
        &self,
        _context: &ConnectionContext,
        params: WatchParams,
        queue_capacity: usize,
    ) -> Result<(WatchResult, WatchEventStream, WatchHandle), BackendError> {
        let store: Arc<dyn ObservationStore> = Arc::clone(&self.store) as Arc<dyn ObservationStore>;
        subscribe_and_stream(
            &store,
            super::SubscribeAndStreamRequest {
                subscription_id: SubscriptionId::new("sub-fixture"),
                params,
                queue_capacity,
            },
            |_| BackendError::application("NOT_FOUND", "run not found", None),
        )
        .await
    }
}
