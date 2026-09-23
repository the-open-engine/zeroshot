use async_trait::async_trait;
use openengine_cluster_protocol::is_canonical_uuid_v7;
use reqwest::header::{ACCEPT, CACHE_CONTROL};
use std::sync::Arc;
use zeroshot_engine::profile_ui::{
    RunHistoryRequest, RunHistoryResponse, RunHistoryTransport, RunHistoryTransportError,
};

use super::contract::{
    build_auth_descriptor, build_controller_descriptor, build_run_history_descriptor,
    require_response_route, validate_metadata_routes, HostedAuthDescriptor, OAuthMetadataWire,
    RunHistoryDescriptor,
};
use super::{AccessToken, TargetHttpControlAuthority};
use crate::native_v2_target::{TargetAccess, TargetAuthorityError, TargetRecord};

struct HistoryDescriptor {
    routes: RunHistoryDescriptor,
    hosted: Option<HostedHistoryAuthority>,
}

struct HostedHistoryAuthority {
    auth: HostedAuthDescriptor,
    audience: String,
}

/// Authenticated server-side executor for a target's discovered run-history capability.
#[derive(Clone)]
pub struct TargetRunHistoryTransport {
    authority: TargetHttpControlAuthority,
    target: TargetRecord,
    descriptor: Arc<tokio::sync::OnceCell<HistoryDescriptor>>,
}

impl TargetRunHistoryTransport {
    pub fn new(
        authority: TargetHttpControlAuthority,
        target: TargetRecord,
    ) -> Result<Self, TargetAuthorityError> {
        // Fail malformed stored origins before the UI binds; capability discovery stays lazy.
        super::contract::parse_origin(&target.origin)?;
        Ok(Self {
            authority,
            target,
            descriptor: Arc::new(tokio::sync::OnceCell::new()),
        })
    }

    async fn descriptor(&self) -> Result<&HistoryDescriptor, RunHistoryTransportError> {
        self.descriptor
            .get_or_try_init(|| async {
                let (origin, wire) = self
                    .authority
                    .discovery(&self.target)
                    .await
                    .map_err(unavailable)?;
                let routes = build_run_history_descriptor(&origin, &wire.extensions)
                    .map_err(incompatible)?;
                let hosted = match &self.target.access {
                    TargetAccess::Direct => {
                        build_controller_descriptor(
                            &origin,
                            wire,
                            self.target.access.authentication(),
                        )
                        .map_err(incompatible)?;
                        None
                    }
                    TargetAccess::Hosted { .. } => {
                        let auth = build_auth_descriptor(&origin, &wire).map_err(incompatible)?;
                        let controller = build_controller_descriptor(
                            &origin,
                            wire,
                            self.target.access.authentication(),
                        )
                        .map_err(incompatible)?;
                        let metadata: OAuthMetadataWire = self
                            .authority
                            .get_json(&auth.metadata_url, "OAuth metadata")
                            .await
                            .map_err(unavailable)?;
                        validate_metadata_routes(&origin, &auth, &metadata)
                            .map_err(incompatible)?;
                        Some(HostedHistoryAuthority {
                            auth,
                            audience: controller.audience,
                        })
                    }
                };
                Ok(HistoryDescriptor { routes, hosted })
            })
            .await
    }

    async fn access(
        &self,
        descriptor: &HistoryDescriptor,
    ) -> Result<Option<AccessToken>, RunHistoryTransportError> {
        let Some(hosted) = &descriptor.hosted else {
            return Ok(None);
        };
        self.authority
            .access_token(&self.target, &hosted.auth, &hosted.audience)
            .await
            .map(Some)
            .map_err(unavailable)
    }

    fn request_url(
        descriptor: &RunHistoryDescriptor,
        request: &RunHistoryRequest,
    ) -> Result<reqwest::Url, RunHistoryTransportError> {
        match request {
            RunHistoryRequest::List { after } => {
                if after.as_ref().is_some_and(|id| !is_canonical_uuid_v7(id)) {
                    return Err(RunHistoryTransportError::incompatible());
                }
                descriptor.list_url(after.as_ref())
            }
            RunHistoryRequest::Detail { id } => {
                if !is_canonical_uuid_v7(id) {
                    return Err(RunHistoryTransportError::incompatible());
                }
                descriptor.detail_url(id)
            }
            RunHistoryRequest::Page { id, after } => {
                if !is_canonical_uuid_v7(id) || !valid_cursor(after) {
                    return Err(RunHistoryTransportError::incompatible());
                }
                descriptor.page_url(id, after)
            }
        }
        .map_err(incompatible)
    }
}

fn valid_cursor(cursor: &openengine_cluster_protocol::Cursor) -> bool {
    cursor
        .as_str()
        .strip_prefix("v2:")
        .is_some_and(|sequence| sequence.parse::<u64>().is_ok())
}

#[async_trait]
impl RunHistoryTransport for TargetRunHistoryTransport {
    async fn get(
        &self,
        request: RunHistoryRequest,
    ) -> Result<RunHistoryResponse, RunHistoryTransportError> {
        let descriptor = self.descriptor().await?;
        let url = Self::request_url(&descriptor.routes, &request)?;
        let access = self.access(descriptor).await?;
        let response = self
            .authority
            .with_access(self.authority.client.get(url.clone()), access.as_deref())
            .map_err(unavailable)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .send()
            .await
            .map_err(|_| RunHistoryTransportError::unavailable())?;
        require_response_route(&response, &url).map_err(unavailable)?;
        self.authority
            .invalidate_rejected_access(&response, access.as_ref())
            .await;
        RunHistoryResponse::from_http(&request, response).await
    }
}

fn incompatible(_: TargetAuthorityError) -> RunHistoryTransportError {
    RunHistoryTransportError::incompatible()
}

fn unavailable(_: TargetAuthorityError) -> RunHistoryTransportError {
    RunHistoryTransportError::unavailable()
}

#[cfg(test)]
#[path = "history/tests.rs"]
mod tests;
