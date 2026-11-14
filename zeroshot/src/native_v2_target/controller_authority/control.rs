use async_trait::async_trait;
use openengine_cluster_protocol::{
    ConnectionDeleteRequest, ConnectionDeleteResult, ConnectionListRequest, ConnectionListResult,
    ConnectionMutationResult, ConnectionSetRequest,
};
use openengine_cluster_protocol::{
    RunCheckpointsParams, RunCheckpointsResult, RunDiscardWorkspaceParams,
    RunDiscardWorkspaceResult, RunForceParams, RunListParams, RunLogEventNotification,
    RunLogsParams, RunResumeParams, RunResumeResult, RunStatusParams, RunSubmitResult,
    RunWatchParams,
};
use openengine_cluster_protocol::{RunProfile, RunProfileMutationResult, RunProfileRunRequest};
use openengine_cluster_protocol::{
    RunProfileDefaultRequest, RunProfileDefaultResult, RunProfileDeleteResult,
};
use openengine_cluster_protocol::{
    RunProfileListRequest, RunProfileListResult, RunProfileSelector, RunProfileSetRequest,
};
use reqwest::header::ACCEPT;
use zeroshot_engine::native_v2_cli::oecp::BoxedSubscription;
use zeroshot_engine::native_v2_cli::{
    CliRunForceResult, CliRunListResult, CliRunStatusResult, CliRunWatchEventNotification,
};
use zeroshot_engine::native_v2_target_authority::{
    TargetOecpSession, TargetRunReceipt, TargetRunRequest,
};

use super::contract::{
    ControllerDescriptor, authority_error, read_success_json, require_response_route,
};
use super::access::AccessToken;
use super::{HostedLogin, TargetHttpControlAuthority};
use crate::native_v2_target::{
    TargetAccess, TargetAuthorityError, TargetControlAuthority, TargetOecpAccess, TargetRecord,
};

impl TargetHttpControlAuthority {
    async fn issue_oecp_session(
        &self,
        target: &TargetRecord,
        request: &openengine_cluster_protocol::TargetOecpSessionRequest,
        controller_access: (ControllerDescriptor, Option<AccessToken>),
    ) -> Result<TargetOecpAccess, TargetAuthorityError> {
        let (controller, access) = controller_access;
        let response = self
            .with_access(
                self.client.post(controller.session_url.clone()),
                access.as_deref(),
            )?
            .header(ACCEPT, "application/json")
            .json(request)
            .send()
            .await
            .map_err(|error| {
                TargetAuthorityError::request_failed("target session request", &error)
            })?;
        require_response_route(&response, &controller.session_url)?;
        self.invalidate_rejected_access(&response, access.as_ref())
            .await;
        let session: TargetOecpSession =
            read_success_json(response, &controller.session_url, "target OECP session").await?;
        TargetOecpAccess::new(session.endpoint, session.bearer_token, &target.access)
            .map_err(|_| authority_error("target OECP session response is malformed"))
    }
}

#[async_trait]
impl TargetControlAuthority for TargetHttpControlAuthority {
    fn redact_error(&self, error: TargetAuthorityError) -> TargetAuthorityError {
        match &self.machine {
            Some(machine) => error.redact_secret(&machine.api_key),
            None => error,
        }
    }

    async fn default_resource_scope(
        &self,
        target: &TargetRecord,
    ) -> Result<openengine_cluster_protocol::ConnectionScope, TargetAuthorityError> {
        self.machine_default_scope(target).await
    }

    async fn review_cache_namespace(
        &self,
        target: &TargetRecord,
        github_token: Option<&str>,
    ) -> Result<Option<String>, TargetAuthorityError> {
        self.review_snapshot_namespace(target, github_token)
            .await
            .map(Some)
    }

    async fn resolve_review(
        &self,
        target: &TargetRecord,
        request: zeroshot_engine::native_v2_review::ReviewResolutionRequest,
        access: zeroshot_engine::native_v2_review::ReviewResolutionAccess<'_>,
    ) -> Result<zeroshot_engine::native_v2_review::PreparedReview, TargetAuthorityError> {
        self.resolve_review_request(target, request, access).await
    }

    async fn discover(&self, target: &TargetRecord) -> Result<(), TargetAuthorityError> {
        match target.access {
            TargetAccess::Hosted { .. } => self.descriptors(target).await.map(|_| ()),
            TargetAccess::Direct => {
                self.require_no_machine_for_direct()?;
                self.controller_descriptor(target).await.map(|_| ())
            }
        }
    }

    async fn run_sizes(
        &self,
        target: &TargetRecord,
    ) -> Result<openengine_cluster_protocol::TargetRunSizesDiscovery, TargetAuthorityError> {
        let (controller, access) = self.controller_access(target).await?;
        let controller = match target.access {
            TargetAccess::Direct => controller,
            TargetAccess::Hosted { .. } => {
                let origin = super::contract::parse_origin(&target.origin)?;
                let url = origin
                    .join(super::DISCOVERY_PATH)
                    .map_err(|_| authority_error("target discovery URL is invalid"))?;
                let response = self
                    .with_access(self.client.get(url.clone()), access.as_deref())?
                    .header(ACCEPT, "application/json")
                    .send()
                    .await
                    .map_err(|error| {
                        TargetAuthorityError::request_failed("target run sizes", &error)
                    })?;
                require_response_route(&response, &url)?;
                self.invalidate_rejected_access(&response, access.as_ref())
                    .await;
                let wire = super::contract::read_success_json_with_limit(
                    response,
                    &url,
                    "target run sizes",
                    super::contract::MAX_DISCOVERY_RESPONSE_BYTES,
                )
                .await?;
                super::contract::build_controller_descriptor(
                    &origin,
                    wire,
                    target.access.authentication(),
                )?
            }
        };
        controller
            .run_sizes
            .ok_or_else(|| authority_error("target does not advertise run sizes"))
    }

    async fn login(&self, target: &TargetRecord) -> Result<(), TargetAuthorityError> {
        self.require_human_credential()?;
        let device_token = target
            .access
            .device_token()
            .ok_or_else(|| authority_error("direct target does not use login"))?;
        let mut cached = self.hosted_access.lock().await;
        *cached = None;
        let (auth, controller) = self.descriptors(target).await?;
        let audience = if auth.delegations.is_some() {
            super::ADMIN_AUDIENCE
        } else {
            &controller.audience
        };
        let _refresh_guard = self.lock_refresh_family(&target.id).await?;
        self.review_login_generation(&target.id, true)?;
        self.login_inner(HostedLogin {
            target_id: &target.id,
            device_token,
            auth: &auth,
            audience,
        })
        .await
    }

    async fn delegation(
        &self,
        target: &TargetRecord,
        action: zeroshot_engine::native_v2_cli::DelegationAction,
    ) -> Result<zeroshot_engine::native_v2_cli::DelegationResult, TargetAuthorityError> {
        self.manage_delegation(target, action).await
    }

    async fn submit(
        &self,
        target: &TargetRecord,
        request: &TargetRunRequest,
    ) -> Result<RunSubmitResult, TargetAuthorityError> {
        let (controller, access) = self
            .controller_access_for_submission(target, request)
            .await?;
        let response = self
            .with_access(
                self.client.post(controller.run_url.clone()),
                access.as_deref(),
            )?
            .header(ACCEPT, "application/json")
            .json(request)
            .send()
            .await
            .map_err(|error| {
                TargetAuthorityError::request_failed("target run request", &error).submission()
            })?;
        require_response_route(&response, &controller.run_url)?;
        self.invalidate_rejected_access(&response, access.as_ref())
            .await;
        let receipt: TargetRunReceipt =
            read_success_json(response, &controller.run_url, "target run")
                .await
                .map_err(TargetAuthorityError::submission)?;
        Ok(RunSubmitResult {
            run_id: receipt.run_id,
        })
    }

    async fn oecp_session(
        &self,
        target: &TargetRecord,
        request: &openengine_cluster_protocol::TargetOecpSessionRequest,
    ) -> Result<TargetOecpAccess, TargetAuthorityError> {
        let controller_access = self.controller_access(target).await?;
        self.issue_oecp_session(target, request, controller_access)
            .await
    }

    async fn workspace_recovery_session(
        &self,
        target: &TargetRecord,
        request: &openengine_cluster_protocol::TargetOecpSessionRequest,
    ) -> Result<TargetOecpAccess, TargetAuthorityError> {
        let controller_access = self.workspace_recovery_controller_access(target).await?;
        self.issue_oecp_session(target, request, controller_access)
            .await
    }

    async fn validate_run_suspension(
        &self,
        target: &TargetRecord,
    ) -> Result<(), TargetAuthorityError> {
        let (origin, wire) = self.discovery(target).await?;
        let descriptor = super::contract::build_controller_descriptor(
            &origin,
            wire,
            target.access.authentication(),
        )?;
        super::require_session_capability(&descriptor, super::TargetSessionPurpose::RunSuspension)
    }

    async fn validate_request_ask(
        &self,
        target: &TargetRecord,
    ) -> Result<(), TargetAuthorityError> {
        let (origin, wire) = self.discovery(target).await?;
        let descriptor = super::contract::build_controller_descriptor(
            &origin,
            wire,
            target.access.authentication(),
        )?;
        super::require_session_capability(&descriptor, super::TargetSessionPurpose::RequestAsk)
    }

    async fn run_suspension_session(
        &self,
        target: &TargetRecord,
        request: &openengine_cluster_protocol::TargetOecpSessionRequest,
    ) -> Result<TargetOecpAccess, TargetAuthorityError> {
        let access = self
            .controller_access_inner(
                target,
                super::TargetSessionPurpose::RunSuspension,
                super::RequestCapabilities::default(),
            )
            .await?;
        self.issue_oecp_session(target, request, access).await
    }

    async fn request_ask_session(
        &self,
        target: &TargetRecord,
        request: &openengine_cluster_protocol::TargetOecpSessionRequest,
    ) -> Result<TargetOecpAccess, TargetAuthorityError> {
        let access = self
            .controller_access_inner(
                target,
                super::TargetSessionPurpose::RequestAsk,
                super::RequestCapabilities::default(),
            )
            .await?;
        self.issue_oecp_session(target, request, access).await
    }

    async fn run_vectors_session(
        &self,
        target: &TargetRecord,
        request: &openengine_cluster_protocol::TargetOecpSessionRequest,
    ) -> Result<TargetOecpAccess, TargetAuthorityError> {
        let access = self
            .controller_access_inner(
                target,
                super::TargetSessionPurpose::RunVectors,
                super::RequestCapabilities::default(),
            )
            .await?;
        self.issue_oecp_session(target, request, access).await
    }

    async fn workspace_checkpoints_session(
        &self,
        target: &TargetRecord,
        request: &openengine_cluster_protocol::TargetOecpSessionRequest,
    ) -> Result<TargetOecpAccess, TargetAuthorityError> {
        let controller_access = self.workspace_checkpoints_controller_access(target).await?;
        self.issue_oecp_session(target, request, controller_access)
            .await
    }

    async fn connection_list(
        &self,
        target: &TargetRecord,
        request: ConnectionListRequest,
    ) -> Result<ConnectionListResult, TargetAuthorityError> {
        TargetHttpControlAuthority::connection_list(self, target, request).await
    }

    async fn connection_set(
        &self,
        target: &TargetRecord,
        request: ConnectionSetRequest,
    ) -> Result<ConnectionMutationResult, TargetAuthorityError> {
        TargetHttpControlAuthority::connection_set(self, target, request).await
    }

    async fn connection_delete(
        &self,
        target: &TargetRecord,
        request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResult, TargetAuthorityError> {
        TargetHttpControlAuthority::connection_delete(self, target, request).await
    }

    async fn profile_list(
        &self,
        target: &TargetRecord,
        request: RunProfileListRequest,
    ) -> Result<RunProfileListResult, TargetAuthorityError> {
        TargetHttpControlAuthority::profile_list(self, target, request).await
    }

    async fn profile_show(
        &self,
        target: &TargetRecord,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, TargetAuthorityError> {
        TargetHttpControlAuthority::profile_show(self, target, selector).await
    }

    async fn profile_set(
        &self,
        target: &TargetRecord,
        request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, TargetAuthorityError> {
        TargetHttpControlAuthority::profile_set(self, target, request).await
    }

    async fn profile_delete(
        &self,
        target: &TargetRecord,
        selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, TargetAuthorityError> {
        TargetHttpControlAuthority::profile_delete(self, target, selector).await
    }

    async fn profile_default(
        &self,
        target: &TargetRecord,
        request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, TargetAuthorityError> {
        TargetHttpControlAuthority::profile_default(self, target, request).await
    }

    async fn profile_run(
        &self,
        target: &TargetRecord,
        request: &RunProfileRunRequest,
    ) -> Result<RunSubmitResult, TargetAuthorityError> {
        TargetHttpControlAuthority::profile_run(self, target, request).await
    }

    async fn stack_submit(
        &self,
        target: &TargetRecord,
        request: &openengine_cluster_protocol::StackSubmitRequest,
    ) -> Result<openengine_cluster_protocol::Stack, TargetAuthorityError> {
        TargetHttpControlAuthority::stack_submit(self, target, request).await
    }

    async fn stack_status(
        &self,
        target: &TargetRecord,
        stack_id: &openengine_cluster_protocol::StackId,
    ) -> Result<openengine_cluster_protocol::Stack, TargetAuthorityError> {
        TargetHttpControlAuthority::stack_status(self, target, stack_id).await
    }

    async fn stack_force(
        &self,
        target: &TargetRecord,
        stack_id: &openengine_cluster_protocol::StackId,
    ) -> Result<openengine_cluster_protocol::Stack, TargetAuthorityError> {
        TargetHttpControlAuthority::stack_force(self, target, stack_id).await
    }

    async fn hosted_run_resume(
        &self,
        target: &TargetRecord,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_resume(self, target, params).await
    }

    async fn hosted_run_export_workspace(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::RunExportWorkspaceParams,
    ) -> Result<zeroshot_engine::native_v2_cli::WorkspaceExportStream, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_export_workspace(self, target, params).await
    }

    async fn hosted_run_suspend(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::RunSuspendParams,
    ) -> Result<Option<CliRunStatusResult>, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_suspend(self, target, params).await
    }

    async fn hosted_run_vectors(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::RunVectorsParams,
    ) -> Result<Option<openengine_cluster_protocol::RunVectorsResult>, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_vectors(self, target, params).await
    }

    async fn hosted_inbox_show(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::InboxShowParams,
    ) -> Result<Option<openengine_cluster_protocol::InboxShowResult>, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_inbox_show(self, target, params).await
    }

    async fn hosted_inbox_list(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::InboxListParams,
    ) -> Result<Option<openengine_cluster_protocol::InboxListResult>, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_inbox_list(self, target, params).await
    }

    async fn hosted_run_vector(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::RunVectorParams,
    ) -> Result<Option<openengine_cluster_protocol::RunVectorResult>, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_vector(self, target, params).await
    }

    async fn hosted_run_checkpoints(
        &self,
        target: &TargetRecord,
        params: RunCheckpointsParams,
    ) -> Result<RunCheckpointsResult, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_checkpoints(self, target, params).await
    }

    async fn hosted_run_discard_workspace(
        &self,
        target: &TargetRecord,
        params: RunDiscardWorkspaceParams,
    ) -> Result<RunDiscardWorkspaceResult, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_discard_workspace(self, target, params).await
    }

    async fn hosted_run_list(
        &self,
        target: &TargetRecord,
        params: RunListParams,
    ) -> Result<CliRunListResult, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_list(self, target, params).await
    }

    async fn hosted_run_status(
        &self,
        target: &TargetRecord,
        params: RunStatusParams,
    ) -> Result<CliRunStatusResult, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_status(self, target, params).await
    }

    async fn hosted_run_watch(
        &self,
        target: &TargetRecord,
        params: RunWatchParams,
    ) -> Result<BoxedSubscription<CliRunWatchEventNotification>, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_watch(self, target, params).await
    }

    async fn hosted_run_logs(
        &self,
        target: &TargetRecord,
        params: RunLogsParams,
    ) -> Result<BoxedSubscription<RunLogEventNotification>, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_logs(self, target, params).await
    }

    async fn hosted_run_force(
        &self,
        target: &TargetRecord,
        params: RunForceParams,
    ) -> Result<CliRunForceResult, TargetAuthorityError> {
        TargetHttpControlAuthority::hosted_run_force(self, target, params).await
    }
}
