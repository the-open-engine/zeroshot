use async_trait::async_trait;
use openengine_cluster_protocol::is_canonical_uuid_v7;
use reqwest::header::{ACCEPT, CACHE_CONTROL};
use std::sync::Arc;
use zeroshot_engine::profile_ui::{
    RunHistoryRequest, RunHistoryResponse, RunHistoryTransport, RunHistoryTransportError,
};

use super::contract::{
    build_controller_descriptor, build_run_history_descriptor, require_response_route,
    HostedAuthDescriptor, RunHistoryDescriptor,
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
                        self.authority
                            .require_no_machine_for_direct()
                            .map_err(incompatible)?;
                        build_controller_descriptor(
                            &origin,
                            wire,
                            self.target.access.authentication(),
                        )
                        .map_err(incompatible)?;
                        None
                    }
                    TargetAccess::Hosted { .. } => {
                        let auth = self
                            .authority
                            .hosted_auth_descriptor(&origin, &wire)
                            .map_err(incompatible)?;
                        let controller = build_controller_descriptor(
                            &origin,
                            wire,
                            self.target.access.authentication(),
                        )
                        .map_err(incompatible)?;
                        self.authority
                            .validate_hosted_metadata(&origin, &auth)
                            .await
                            .map_err(unavailable)?;
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

    async fn read_response(
        &self,
        request: &RunHistoryRequest,
        response: reqwest::Response,
    ) -> Result<RunHistoryResponse, RunHistoryTransportError> {
        if !response.status().is_success()
            && let Some(machine) = &self.authority.machine
        {
            let status = response.status().as_u16();
            let mut problem: serde_json::Value = super::contract::read_json_with_limit(
                response,
                "run history",
                request.maximum_problem_bytes(),
            )
            .await
            .map_err(unavailable)?;
            super::super::authority_error::redact_json(&mut problem, &machine.api_key);
            let body = serde_json::to_vec(&problem)
                .map_err(|_| RunHistoryTransportError::unavailable())?;
            return RunHistoryResponse::new(status, body);
        }
        RunHistoryResponse::from_http(request, response).await
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
        self.read_response(&request, response).await
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
