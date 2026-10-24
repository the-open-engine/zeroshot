//! Publication facts live outside rollbackable workspace snapshots.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::execution::platform::{self, FileAccess};

const MAX_RECORD_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublicationRecord {
    pub version: u32,
    pub url: String,
    pub marker: String,
    pub body_hash: String,
    pub author: String,
    pub status: PublicationStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum PublicationStatus {
    Ready,
    Attempted,
    Confirmed {
        #[serde(rename = "commentId")]
        comment_id: String,
        #[serde(rename = "commentUrl")]
        comment_url: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PublicationStoreError {
    #[error("comment publication storage is unavailable: {0}")]
    Unavailable(String),
    #[error("comment publication record changed concurrently")]
    Conflict,
    #[error("comment publication record is invalid")]
    InvalidRecord,
}

impl PublicationRecord {
    pub fn validate(&self) -> Result<(), PublicationStoreError> {
        let valid = self.version == 1
            && self.url.len() <= 2048
            && crate::source_code_provider::adapters::core::comment_destination(&self.url).is_ok()
            && self
                .marker
                .strip_prefix("<!-- zeroshot-comment:v1:")
                .and_then(|value| value.strip_suffix(" -->"))
                .is_some_and(super::valid_digest)
            && super::valid_digest(&self.body_hash)
            && super::valid_id(&self.author)
            && self.valid_status();
        if valid && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_RECORD_BYTES) {
            Ok(())
        } else {
            Err(PublicationStoreError::InvalidRecord)
        }
    }

    fn valid_status(&self) -> bool {
        match &self.status {
            PublicationStatus::Ready | PublicationStatus::Attempted => true,
            PublicationStatus::Confirmed {
                comment_id,
                comment_url,
            } => crate::source_code_provider::adapters::core::comment_destination(&self.url)
                .is_ok_and(|destination| {
                    super::valid_comment_permalink(&destination, comment_id, comment_url)
                }),
        }
    }
}

/// One root lineage's publisher slot. Hosts authorize that scope before supplying this handle.
#[async_trait]
pub trait CommentPublicationStore: Send + Sync {
    async fn load(&self) -> Result<Option<PublicationRecord>, PublicationStoreError>;
    async fn compare_and_set(
        &self,
        expected: Option<&PublicationRecord>,
        next: &PublicationRecord,
    ) -> Result<(), PublicationStoreError>;
}

#[derive(Clone, Debug)]
pub struct LocalCommentPublicationStore {
    path: PathBuf,
}

impl LocalCommentPublicationStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

fn io_error(error: std::io::Error) -> PublicationStoreError {
    PublicationStoreError::Unavailable(error.to_string())
}

fn read_record(path: &Path) -> Result<Option<PublicationRecord>, PublicationStoreError> {
    let file = match platform::private_file(path, FileAccess::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(PublicationStoreError::InvalidRecord);
    }
    let record: PublicationRecord =
        serde_json::from_slice(&bytes).map_err(|_| PublicationStoreError::InvalidRecord)?;
    record.validate()?;
    Ok(Some(record))
}

fn locked<T>(
    path: &Path,
    operation: impl FnOnce() -> Result<T, PublicationStoreError>,
) -> Result<T, PublicationStoreError> {
    let parent = path.parent().ok_or(PublicationStoreError::InvalidRecord)?;
    platform::private_directory(parent).map_err(io_error)?;
    let lock = platform::private_file(&path.with_extension("lock"), FileAccess::ReadWrite)
        .map_err(io_error)?;
    lock.lock_exclusive().map_err(io_error)?;
    let result = operation();
    FileExt::unlock(&lock).map_err(io_error)?;
    result
}

fn write_record(path: &Path, record: &PublicationRecord) -> Result<(), PublicationStoreError> {
    record.validate()?;
    let parent = path.parent().ok_or(PublicationStoreError::InvalidRecord)?;
    let temporary = parent.join(format!(".comment-{}.tmp", uuid::Uuid::now_v7()));
    let result = (|| {
        let mut file =
            platform::private_file(&temporary, FileAccess::CreateNew).map_err(io_error)?;
        let bytes = serde_json::to_vec(record).map_err(|_| PublicationStoreError::InvalidRecord)?;
        file.write_all(&bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        platform::commit_file(&temporary, path, parent).map_err(io_error)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[async_trait]
impl CommentPublicationStore for LocalCommentPublicationStore {
    async fn load(&self) -> Result<Option<PublicationRecord>, PublicationStoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || locked(&path, || read_record(&path)))
            .await
            .map_err(|_| PublicationStoreError::Unavailable("storage task failed".to_owned()))?
    }

    async fn compare_and_set(
        &self,
        expected: Option<&PublicationRecord>,
        next: &PublicationRecord,
    ) -> Result<(), PublicationStoreError> {
        next.validate()?;
        let path = self.path.clone();
        let expected = expected.cloned();
        let next = next.clone();
        tokio::task::spawn_blocking(move || {
            locked(&path, || {
                let current = read_record(&path)?;
                if current != expected {
                    return Err(PublicationStoreError::Conflict);
                }
                write_record(&path, &next)
            })
        })
        .await
        .map_err(|_| PublicationStoreError::Unavailable("storage task failed".to_owned()))?
    }
}
