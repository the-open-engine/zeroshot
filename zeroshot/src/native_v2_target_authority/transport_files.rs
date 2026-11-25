//! Dedicated, bounded-buffer HTTP file transport. Authentication precedes body reads and every
//! lookup goes through the mounted host's authorization. This route never enters OECP JSON.

use std::io::{self, Seek};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use openengine_cluster_protocol::{
    FileRequest, FileResponse, MAX_FILE_HEADER_BYTES, MAX_FILE_TRANSFER_BYTES,
    TARGET_FILES_MEDIA_TYPE,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::native_v2_files::{
    FileAccess, FileReplaceRequest, FileTransfer, TargetFileHost, new_transfer, reclaim_transfers,
};
use super::{NativeV2TargetServer, TargetAuthorityError};
use super::http::{
    RequestHead, HttpResponse, authority_error_response, invalid_request_response,
    not_found_response, write_http_response,
};

/// Optional host installation, independent of controller, artifact and recovery support.
#[derive(Clone)]
pub struct TargetFilesMount {
    pub target: String,
    pub host: Arc<dyn TargetFileHost>,
    pub transfers: PathBuf,
}

impl NativeV2TargetServer {
    pub fn with_files(mut self, mount: TargetFilesMount) -> io::Result<Self> {
        if mount.target.is_empty() {
            return Err(io::Error::other("file target identity is required"));
        }
        reclaim_transfers(&mount.transfers)?;
        self.files = Some(mount);
        Ok(self)
    }

    pub(super) async fn serve_files(
        &self,
        mut stream: TcpStream,
        head: RequestHead,
    ) -> io::Result<()> {
        // Routing only peeked these bounded headers. Consume them before early replies so closing
        // with unread headers cannot reset the peer. Authentication still precedes all body reads.
        let mut headers = vec![0_u8; head.encoded_len];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut headers)).await??;
        let Some(_mount) = &self.files else {
            return write_http_response(&mut stream, not_found_response()).await;
        };
        if head.method != "POST" {
            return write_http_response(&mut stream, not_found_response()).await;
        }
        let identity = match self.authenticate_control(&head).await {
            Ok(identity) => identity,
            Err(error) => {
                return write_http_response(&mut stream, authority_error_response(error)).await;
            }
        };
        let result = self.files_body(&mut stream, &head, identity).await;
        match result {
            Ok(FileReply::Json(response)) => {
                write_http_response(&mut stream, HttpResponse::private_json(200, &response)).await
            }
            Ok(FileReply::Tree(staged)) => {
                let length = staged.as_file().metadata()?.len();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
                );
                stream.write_all(response.as_bytes()).await?;
                let mut file = tokio::fs::File::from_std(staged.as_file().try_clone()?);
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    let count = file.read(&mut buffer).await?;
                    if count == 0 {
                        break;
                    }
                    tokio::time::timeout(
                        Duration::from_secs(30),
                        stream.write_all(&buffer[..count]),
                    )
                    .await??;
                }
                stream.shutdown().await
            }
            Err(error) => {
                let response = if error.kind() == io::ErrorKind::PermissionDenied {
                    authority_error_response(TargetAuthorityError::unauthorized())
                } else if error.kind() == io::ErrorKind::NotFound {
                    not_found_response()
                } else {
                    invalid_request_response(
                        "file operation failed: invalid, incomplete or conflicting transfer",
                    )
                };
                write_http_response(&mut stream, response).await
            }
        }
    }

    async fn files_body(
        &self,
        stream: &mut TcpStream,
        head: &RequestHead,
        identity: openengine_cluster_server::identity::ConnectionIdentity,
    ) -> io::Result<FileReply> {
        let mount = self
            .files
            .clone()
            .ok_or_else(|| io::Error::other("file host is unavailable"))?;
        if head.has_header("transfer-encoding")
            || head.header_exact("content-type") != Some(TARGET_FILES_MEDIA_TYPE)
        {
            return Err(io::Error::other("unsupported file framing"));
        }
        let length = head
            .header_exact("content-length")
            .and_then(|length| length.parse::<u64>().ok())
            .filter(|length| {
                *length >= 4
                    && *length <= MAX_FILE_TRANSFER_BYTES + MAX_FILE_HEADER_BYTES as u64 + 4
            })
            .ok_or_else(|| io::Error::other("invalid file transfer length"))?;
        let metadata_len =
            tokio::time::timeout(Duration::from_secs(30), stream.read_u32()).await?? as usize;
        if metadata_len > MAX_FILE_HEADER_BYTES || metadata_len as u64 + 4 > length {
            return Err(io::Error::other("file request exceeds limit"));
        }
        let mut metadata = vec![0_u8; metadata_len];
        tokio::time::timeout(Duration::from_secs(30), stream.read_exact(&mut metadata)).await??;
        let request: FileRequest = serde_json::from_slice(&metadata)
            .map_err(|_| io::Error::other("invalid file request"))?;
        let payload_len = length - 4 - metadata_len as u64;
        if !matches!(request, FileRequest::Replace { .. }) && payload_len != 0 {
            return Err(io::Error::other("unexpected file payload"));
        }
        if let FileRequest::Cleanup { all } = request {
            return self.cleanup_files(mount, identity, all).await;
        }
        // The guard stays in this future while async I/O drains. Cancellation drops it; no partial
        // payload can reach the catalog, and restart cleanup reclaims abandoned transfer files.
        let mut staged = new_transfer(&mount.transfers)?;
        let mut file = tokio::fs::File::from_std(staged.as_file().try_clone()?);
        let mut left = payload_len;
        let mut buffer = [0_u8; 64 * 1024];
        while left != 0 {
            let count = left.min(buffer.len() as u64) as usize;
            tokio::time::timeout(
                Duration::from_secs(30),
                stream.read_exact(&mut buffer[..count]),
            )
            .await??;
            file.write_all(&buffer[..count]).await?;
            left -= count as u64;
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        staged.rewind()?;
        // Once a complete mutation begins, let it finish durably even if its HTTP caller leaves.
        // Stable operation keys reconcile a lost response without publishing different bytes.
        tokio::task::spawn_blocking(move || {
            let access = FileAccess {
                target: &mount.target,
                identity: &identity,
            };
            match request {
                FileRequest::Cleanup { .. } => Err(io::Error::other("cleanup was already handled")),
                FileRequest::List { scope, path } => Ok(FileReply::Json(FileResponse::List {
                    paths: mount.host.list(&access, scope, &path)?,
                })),
                FileRequest::ListMembers { selection } => {
                    Ok(FileReply::Json(FileResponse::Members {
                        entries: mount.host.list_members(&access, &selection)?,
                        selection,
                    }))
                }
                FileRequest::Replace { address, operation } => {
                    mount.host.replace(
                        &access,
                        &FileReplaceRequest {
                            address: address.clone(),
                            operation: operation.clone(),
                        },
                        &mut staged,
                    )?;
                    Ok(FileReply::Json(FileResponse::Replaced {
                        address,
                        operation,
                    }))
                }
                FileRequest::Remove { address } => Ok(FileReply::Json(FileResponse::Removed {
                    removed: mount.host.remove(&access, &address)?,
                })),
                FileRequest::Download { selection } => {
                    mount.host.download(&access, &selection, &mut staged)?;
                    staged.rewind()?;
                    Ok(FileReply::Tree(staged))
                }
            }
        })
        .await
        .map_err(io::Error::other)?
    }

    async fn cleanup_files(
        &self,
        mount: TargetFilesMount,
        identity: openengine_cluster_server::identity::ConnectionIdentity,
        all: bool,
    ) -> io::Result<FileReply> {
        // Retention is a direct single-account host operation, never a hosted file-client grant.
        if !matches!(self.access, super::TargetServerAccess::Direct(_)) {
            return Err(crate::native_v2_files::denied());
        }
        let now = crate::native_v2_runner::current_timestamp();
        let controller = self.target.controller().await.map_err(io::Error::other)?;
        let selection_mount = mount.clone();
        let selection_identity = identity.clone();
        let candidates = tokio::task::spawn_blocking(move || {
            selection_mount.host.select_publications(
                &FileAccess {
                    target: &selection_mount.target,
                    identity: &selection_identity,
                },
                now,
                all,
            )
        })
        .await
        .map_err(io::Error::other)??;
        let protected = controller
            .protected_file_runs()
            .await
            .map_err(io::Error::other)?;
        let checkpoint_lineages = controller
            .cleanup_checkpoints(all, now)
            .await
            .map_err(io::Error::other)?;
        let publications = tokio::task::spawn_blocking(move || {
            mount.host.cleanup_publications(
                &FileAccess {
                    target: &mount.target,
                    identity: &identity,
                },
                &crate::native_v2_files::FileRetention {
                    candidates: &candidates,
                    now,
                    all,
                    protected: &protected,
                },
            )
        })
        .await
        .map_err(io::Error::other)??;
        Ok(FileReply::Json(FileResponse::Cleaned {
            publications: publications as u64,
            checkpoint_lineages: checkpoint_lineages as u64,
        }))
    }
}

enum FileReply {
    Json(FileResponse),
    Tree(FileTransfer),
}
