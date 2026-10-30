//! Host storage boundary shared by SQLite and authenticated hosting adapters.
use std::sync::Arc;
use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use serde::{Deserialize, Serialize};
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

/// Hosts must enforce run identity and scope independently of caller-supplied fields.
#[async_trait]
pub trait ActiveMemoryBackend: Send + Sync {
    async fn snapshot(
        &self,
        run: &RunId,
        scope: &ScopeQuery,
    ) -> Result<MemorySnapshot, ActiveMemoryError>;
    async fn publish(
        &self,
        run: &RunId,
        publication: MemoryPublication,
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
    ) -> Result<Vec<LessonVersion>, ActiveMemoryError> {
        let mut store = self.0.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
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
        .await
        .map_err(|_| ActiveMemoryError::Unavailable)?
    }
}

impl SqliteMemoryBackend {
    pub fn shared(store: SharedActiveMemoryStore) -> Arc<dyn ActiveMemoryBackend> {
        Arc::new(Self(store))
    }
}
