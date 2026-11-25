//! Host-side capture before request visibility or primary graph completion.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use async_trait::async_trait;
use openengine_cluster_protocol::{FieldName, PublishedFile, PublishedFiles, UnixTimestampMillis};
use openengine_cluster_server::identity::ConnectionIdentity;
use super::{DiskFileStore, FileAccess, FilePublicationOwner, Result};

/// Validate the complete receipt against the caller's durable and public record envelopes.
pub type PublicationPreflight = Arc<dyn Fn(&PublishedFiles) -> Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct FilePublication {
    pub owner: FilePublicationOwner,
    pub paths: BTreeMap<FieldName, String>,
    pub now: UnixTimestampMillis,
    pub cancellation: crate::execution::driver::DriverCancellation,
    pub preflight: PublicationPreflight,
}

#[async_trait]
pub trait RunFilePublisher: Send + Sync {
    /// Call the supplied preflight before committing a publication, including on receipt replay.
    async fn publish(&self, request: FilePublication) -> Result<PublishedFiles>;
}

/// The local placement records references into the user's original working root.
pub struct LocalFilePublisher {
    pub workspace: PathBuf,
}

#[async_trait]
impl RunFilePublisher for LocalFilePublisher {
    async fn publish(&self, request: FilePublication) -> Result<PublishedFiles> {
        let FilePublication {
            paths,
            cancellation,
            preflight,
            ..
        } = request;
        let root = self.workspace.clone();
        tokio::task::spawn_blocking(move || {
            if cancellation.is_cancelled() {
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
            }
            let files = paths
                .into_iter()
                .map(|(field, path)| {
                    let resolved =
                        super::workspace::local_workspace_path(&root, &path).map_err(|error| {
                            std::io::Error::new(
                                error.kind(),
                                format!("artifacts.{} path {path:?}: {error}", field.as_str()),
                            )
                        })?;
                    let requested = root.join(&path);
                    let path = requested.strip_prefix(&root).unwrap_or_else(|_| {
                        resolved
                            .strip_prefix(&root)
                            .expect("validated workspace path")
                    });
                    let path = path
                        .to_str()
                        .ok_or_else(|| super::invalid("filepath is not UTF-8"))?;
                    Ok((
                        field,
                        PublishedFile::Local {
                            root: root
                                .to_str()
                                .ok_or_else(|| super::invalid("working root is not UTF-8"))?
                                .to_owned(),
                            path: if path.is_empty() { "." } else { path }.to_owned(),
                        },
                    ))
                })
                .collect::<Result<PublishedFiles>>()?;
            preflight(&files)?;
            Ok(files)
        })
        .await
        .map_err(|_| super::invalid("filepath validation task failed"))?
    }
}

pub struct ManagedFilePublisher {
    pub workspace: PathBuf,
    pub store: Arc<DiskFileStore>,
    pub identity: ConnectionIdentity,
}

#[async_trait]
impl RunFilePublisher for ManagedFilePublisher {
    async fn publish(&self, request: FilePublication) -> Result<PublishedFiles> {
        let workspace = self.workspace.clone();
        let store = self.store.clone();
        let identity = self.identity.clone();
        // A detached waiter never cancels a started atomic catalog mutation.
        tokio::task::spawn_blocking(move || {
            store.capture_publication(
                &FileAccess {
                    target: store.target(),
                    identity: &identity,
                },
                &workspace,
                &request,
            )
        })
        .await
        .map_err(|_| super::invalid("artifact capture task failed"))?
    }
}

/// Recheck confinement each time a local reference is opened; these bytes are never retained.
pub fn open_local_publication(
    reference: &PublishedFile,
    output: &mut dyn std::io::Write,
) -> Result<()> {
    let PublishedFile::Local { root, path } = reference else {
        return Err(super::invalid("live local reference required"));
    };
    let root = std::path::Path::new(root);
    let path = super::workspace::local_workspace_path(root, path)?;
    super::pack(&path, output)
}
