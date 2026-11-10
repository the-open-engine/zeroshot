use futures_util::StreamExt;
use openengine_cluster_protocol::RunExportWorkspaceParams;
use reqwest::header::{ACCEPT, CACHE_CONTROL, CONTENT_TYPE, CONTENT_LENGTH, TRANSFER_ENCODING};

use super::{TargetAuthorityError, TargetHttpControlAuthority, TargetRecord};
use super::super::contract::{authority_error, http_error, require_response_route};
use zeroshot_engine::native_v2_cli::WorkspaceExportStream;

impl TargetHttpControlAuthority {
    pub(in super::super) async fn hosted_run_export_workspace(
        &self,
        target: &TargetRecord,
        params: RunExportWorkspaceParams,
    ) -> Result<WorkspaceExportStream, TargetAuthorityError> {
        if matches!(target.access, crate::native_v2_target::TargetAccess::Direct) {
            return Err(authority_error(
                "workspace export requires a named hosted target",
            ));
        }
        let (auth, controller) = self.descriptors(target).await?;
        // Check capability and compile the exact advertised route before reading credentials.
        let url = auth.hosted_runs.export_workspace_url(&params.run_id)?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        let response = self
            .authorized(self.stream_client.post(url.clone()), &access)?
            .header(ACCEPT, "application/x-tar")
            .header(CACHE_CONTROL, "no-store")
            .json(&params)
            .send()
            .await
            .map_err(|error| TargetAuthorityError::request_failed("workspace export", &error))?;
        require_response_route(&response, &url)?;
        self.invalidate_rejected_access(&response, Some(&access))
            .await;
        if !response.status().is_success() {
            return Err(http_error(response, "workspace export")
                .await
                .redact_secret(&access));
        }
        if response.status() != reqwest::StatusCode::OK
            || !response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value
                        .split(';')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .eq_ignore_ascii_case("application/x-tar")
                })
        {
            return Err(authority_error(
                "workspace export response must be a complete application/x-tar transfer",
            ));
        }
        // A close-delimited HTTP/1 body cannot distinguish exporter failure from clean completion.
        if matches!(
            response.version(),
            reqwest::Version::HTTP_10 | reqwest::Version::HTTP_11
        ) {
            let encodings = response.headers().get_all(TRANSFER_ENCODING);
            let mut encodings = encodings.iter();
            // Transfer-Encoding overrides Content-Length; accept only the chunked coding
            // that the transport decodes and verifies, never an EOF-delimited alternative.
            let framed = match encodings.next() {
                Some(encoding) => {
                    encodings.next().is_none()
                        && encoding
                            .to_str()
                            .is_ok_and(|value| value.trim().eq_ignore_ascii_case("chunked"))
                }
                None => response.headers().contains_key(CONTENT_LENGTH),
            };
            if !framed {
                return Err(authority_error(
                    "workspace export response requires explicit HTTP framing",
                ));
            }
        }
        let target = target.clone();
        Ok(Box::pin(response.bytes_stream().map(move |chunk| {
            chunk.map(|bytes| bytes.to_vec()).map_err(|error| {
                TargetAuthorityError::request_failed("workspace export transfer", &error)
                    .into_cli(&target)
            })
        })))
    }
}
