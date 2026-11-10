//! Host storage boundary shared by SQLite and authenticated hosting adapters.
use std::sync::Arc;
use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::Instant;
use super::{
    ActiveMemoryError, LessonMutation, LessonVersion, MemorySnapshot, ScopeQuery,
    SharedActiveMemoryStore,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPublication {
    pub scope: ScopeQuery,
    pub reviewed: Vec<LessonVersion>,
    pub mutations: Vec<LessonMutation>,
}

/// Direct access to the original hard-stop signal and absolute publication deadline.
/// Queued effects must check this again at their dispatch boundary; started writes drain.
#[derive(Clone, Debug)]
pub struct MemoryPublicationControl {
    cancellation: watch::Receiver<bool>,
    deadline: Instant,
}

impl MemoryPublicationControl {
    pub fn new(cancellation: watch::Receiver<bool>, deadline: Instant) -> Self {
        Self {
            cancellation,
            deadline,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        *self.cancellation.borrow()
            || self.cancellation.has_changed().is_err()
            || Instant::now() >= self.deadline
    }

    pub async fn cancelled(&self) {
        tokio::select! {
            biased;
            () = cancelled(self.cancellation.clone()) => {},
            () = tokio::time::sleep_until(self.deadline) => {},
        }
    }
}

/// Hosts must enforce run identity and scope independently of caller-supplied fields.
#[async_trait]
pub trait ActiveMemoryBackend: Send + Sync {
    async fn snapshot(
        &self,
        run: &RunId,
        scope: &ScopeQuery,
    ) -> Result<MemorySnapshot, ActiveMemoryError>;
    /// Stop queued work on cancellation; check again before starting a write.
    /// Drain already-started writes within the adapter's bounded storage timeout.
    async fn publish(
        &self,
        run: &RunId,
        publication: MemoryPublication,
        control: MemoryPublicationControl,
    ) -> Result<Vec<LessonVersion>, ActiveMemoryError>;
}

pub struct SqliteMemoryBackend(pub SharedActiveMemoryStore);

#[async_trait]
impl ActiveMemoryBackend for SqliteMemoryBackend {
    async fn snapshot(
        &self,
        _: &RunId,
        scope: &ScopeQuery,
    ) -> Result<MemorySnapshot, ActiveMemoryError> {
        let store = self.0.clone().lock_owned().await;
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || store.snapshot(&scope))
            .await
            .map_err(|_| ActiveMemoryError::Unavailable)?
    }
    async fn publish(
        &self,
        _: &RunId,
        publication: MemoryPublication,
        control: MemoryPublicationControl,
    ) -> Result<Vec<LessonVersion>, ActiveMemoryError> {
        let mut store = tokio::select! {
            biased;
            () = control.cancelled() => return Err(ActiveMemoryError::Cancelled),
            store = self.0.clone().lock_owned() => store,
        };
        tokio::task::spawn_blocking(move || {
            dispatch_write(&control, || {
                let expected = publication
                    .reviewed
                    .into_iter()
                    .map(|v| (v.id, Some(v.revision)))
                    .collect::<Vec<_>>();
                if expected.is_empty() {
                    store.apply_batch(&publication.mutations)
                } else {
                    store.apply_reviewed_batch(&publication.mutations, &expected)
                }
            })
        })
        .await
        .map_err(|_| ActiveMemoryError::Unavailable)?
    }
}

fn dispatch_write(
    control: &MemoryPublicationControl,
    write: impl FnOnce() -> Result<Vec<LessonVersion>, ActiveMemoryError>,
) -> Result<Vec<LessonVersion>, ActiveMemoryError> {
    if control.is_cancelled() {
        return Err(ActiveMemoryError::Cancelled);
    }
    write()
}

impl SqliteMemoryBackend {
    pub fn shared(store: SharedActiveMemoryStore) -> Arc<dyn ActiveMemoryBackend> {
        Arc::new(Self(store))
    }
}

/// A closed cancellation channel also revokes permission to start more work.
pub(super) async fn cancelled(mut cancellation: watch::Receiver<bool>) {
    loop {
        if *cancellation.borrow_and_update() || cancellation.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
