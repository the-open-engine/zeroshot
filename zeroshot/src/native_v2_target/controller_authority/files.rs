use std::io::Seek;
use std::path::Path;
use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use openengine_cluster_protocol::{
    FileRequest, FileResponse, MAX_FILE_HEADER_BYTES, MAX_FILE_RESPONSE_BYTES,
    MAX_FILE_TRANSFER_BYTES, TARGET_FILES_KIND, TARGET_FILES_MEDIA_TYPE, TARGET_FILES_PATH,
};
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{TargetHttpControlAuthority, TargetRecord};
use super::contract::{
    authority_error, http_error, require_response_route, read_success_json_with_limit,
    build_controller_descriptor,
};
use crate::native_v2_target::{TargetAccess, TargetAuthorityError};
use zeroshot_engine::native_v2_files::{FileTransfer, new_transfer};

pub(in super::super) enum FileReply {
    Json(FileResponse),
    Tree(FileTransfer),
}

impl TargetHttpControlAuthority {
    pub(super) async fn release_uploads(
        &self,
        target: &TargetRecord,
        uploads: &[zeroshot_engine::native_v2_files::FilePin],
    ) -> Result<(), TargetAuthorityError> {
        let scratch = self.refresh_lock_directory.join("file-transfers");
        let staged = new_transfer(&scratch).map_err(file_io)?;
        for upload in uploads {
            match self
                .file_request(
                    target,
                    &FileRequest::ReleaseStaged {
                        run_id: upload.run_id.clone(),
                        operation: upload.operation.clone(),
                    },
                    (&staged, &scratch),
                )
                .await?
            {
                FileReply::Json(FileResponse::Released {}) => {}
                _ => return Err(authority_error("unexpected upload release response")),
            }
        }
        Ok(())
    }

    pub(in super::super) async fn file_request(
        &self,
        target: &TargetRecord,
        request: &FileRequest,
        transfer: (&FileTransfer, &Path),
    ) -> Result<FileReply, TargetAuthorityError> {
        let (origin, wire) = self.discovery(target).await?;
        // Independent marker checked before tokens or transfer bytes. Old targets retain their
        // existing operations and receive no file request when support is absent or unknown.
        if !wire
            .extensions
            .files
            .as_ref()
            .is_some_and(|marker| marker.kind == TARGET_FILES_KIND)
        {
            return Err(authority_error(
                "target does not advertise openengine.target-files/v1",
            ));
        }
        let controller =
            build_controller_descriptor(&origin, wire.clone(), target.access.authentication())?;
        let access = match target.access {
            TargetAccess::Direct => {
                self.require_no_machine_for_direct()?;
                None
            }
            TargetAccess::Hosted { .. } => {
                let auth = self.hosted_auth_descriptor(&origin, &wire)?;
                self.validate_hosted_metadata(&origin, &auth).await?;
                Some(
                    self.access_token(target, &auth, &controller.audience)
                        .await?,
                )
            }
        };
        let url = origin
            .join(TARGET_FILES_PATH)
            .map_err(|_| authority_error("invalid file route"))?;
        let metadata =
            serde_json::to_vec(request).map_err(|_| authority_error("invalid file request"))?;
        if metadata.len() > MAX_FILE_HEADER_BYTES {
            return Err(authority_error("file request exceeds limit"));
        }
        let mut prefix = (metadata.len() as u32).to_be_bytes().to_vec();
        prefix.extend(metadata);
        let mut file = transfer.0.as_file().try_clone().map_err(file_io)?;
        file.rewind().map_err(file_io)?;
        let length = prefix.len() as u64 + file.metadata().map_err(file_io)?.len();
        let file = tokio::fs::File::from_std(file);
        let stream =
            futures_util::stream::once(async move { Ok::<_, std::io::Error>(prefix) }).chain(
                futures_util::stream::try_unfold(file, |mut file| async move {
                    let mut buffer = vec![0_u8; 64 * 1024];
                    let count = file.read(&mut buffer).await?;
                    buffer.truncate(count);
                    Ok::<_, std::io::Error>((count != 0).then_some((buffer, file)))
                }),
            );
        let response = self
            .with_access(self.stream_client.post(url.clone()), access.as_deref())?
            .header(CONTENT_TYPE, TARGET_FILES_MEDIA_TYPE)
            .header(ACCEPT, TARGET_FILES_MEDIA_TYPE)
            .header(CONTENT_LENGTH, length)
            .body(reqwest::Body::wrap_stream(
                stream.map_err(std::io::Error::other),
            ))
            .send()
            .await
            .map_err(|error| TargetAuthorityError::request_failed("file request", &error))?;
        require_response_route(&response, &url)?;
        self.invalidate_rejected_access(&response, access.as_ref())
            .await;
        if !response.status().is_success() {
            return Err(http_error(response, "file request").await);
        }
        if !matches!(request, FileRequest::Download { .. }) {
            return read_success_json_with_limit(
                response,
                &url,
                "file request",
                MAX_FILE_RESPONSE_BYTES,
            )
            .await
            .map(FileReply::Json);
        }
        receive_tree(response, transfer.1)
            .await
            .map(FileReply::Tree)
    }
}

async fn receive_tree(
    response: reqwest::Response,
    scratch: &Path,
) -> Result<FileTransfer, TargetAuthorityError> {
    if response.status() != reqwest::StatusCode::OK
        || response.headers().contains_key(TRANSFER_ENCODING)
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(TARGET_FILES_MEDIA_TYPE)
    {
        return Err(authority_error(
            "file download requires a complete framed file tree",
        ));
    }
    let expected = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|length| *length <= MAX_FILE_TRANSFER_BYTES)
        .ok_or_else(|| authority_error("invalid file download length"))?;
    let mut staged = new_transfer(scratch).map_err(file_io)?;
    let mut output = tokio::fs::File::from_std(staged.as_file().try_clone().map_err(file_io)?);
    let mut received = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(30), stream.next())
        .await
        .map_err(|_| authority_error("file download idle timeout"))?
    {
        let chunk =
            chunk.map_err(|error| TargetAuthorityError::request_failed("file download", &error))?;
        received = received
            .checked_add(chunk.len() as u64)
            .filter(|length| *length <= expected)
            .ok_or_else(|| authority_error("file download exceeds length"))?;
        output.write_all(&chunk).await.map_err(file_io)?;
    }
    if received != expected {
        return Err(authority_error("incomplete file download"));
    }
    output.flush().await.map_err(file_io)?;
    output.sync_all().await.map_err(file_io)?;
    drop(output);
    staged.rewind().map_err(file_io)?;
    Ok(staged)
}

fn file_io(_error: std::io::Error) -> TargetAuthorityError {
    authority_error("private file transfer I/O failed")
}
