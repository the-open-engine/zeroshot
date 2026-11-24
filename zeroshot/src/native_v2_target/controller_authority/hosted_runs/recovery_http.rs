use openengine_cluster_protocol::{
    RunCheckpointsParams, RunCheckpointsResult, RunDiscardWorkspaceParams,
    RunDiscardWorkspaceResult, RunResumeParams, RunResumeResult,
};
use openengine_cluster_protocol as protocol;
use reqwest::Url;
use reqwest::header::{ACCEPT, CACHE_CONTROL};
use serde::{Serialize, de::DeserializeOwned};

use super::{HostedRunResponse, part_identity};
use super::{AccessToken, TargetAuthorityError, TargetHttpControlAuthority, TargetRecord};

impl TargetHttpControlAuthority {
    pub(in super::super) async fn hosted_run_vector(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::RunVectorParams,
    ) -> Result<Option<openengine_cluster_protocol::RunVectorResult>, TargetAuthorityError> {
        let (auth, controller) = self
            .descriptors_inner(target, super::super::TargetSessionPurpose::RunVectors)
            .await?;
        let Some(url) = auth.hosted_runs.vector_url(&params.run_id)? else {
            return Ok(None);
        };
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        self.hosted_run_post(
            (url, &access, controller.run_parts),
            params,
            HostedRunResponse::new("hosted run vector", protocol::RUN_VECTOR_METHOD)
                .with_limit(openengine_cluster_protocol::MAX_VECTOR_RESPONSE_BYTES),
        )
        .await
        .map(Some)
    }

    pub(in super::super) async fn hosted_inbox_show(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::InboxShowParams,
    ) -> Result<Option<openengine_cluster_protocol::InboxShowResult>, TargetAuthorityError> {
        let (auth, controller) = self
            .descriptors_inner(target, super::super::TargetSessionPurpose::RequestAsk)
            .await?;
        let url = auth.hosted_runs.inbox_show_url(&params.run_id)?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        self.hosted_run_post(
            (url, &access, controller.run_parts),
            params,
            HostedRunResponse::new("hosted inbox show", protocol::INBOX_SHOW_METHOD)
                .with_limit(openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES),
        )
        .await
        .map(Some)
    }

    pub(in super::super) async fn hosted_run_vectors(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::RunVectorsParams,
    ) -> Result<Option<openengine_cluster_protocol::RunVectorsResult>, TargetAuthorityError> {
        let (auth, controller) = self
            .descriptors_inner(target, super::super::TargetSessionPurpose::RunVectors)
            .await?;
        let Some(url) = auth.hosted_runs.vectors_url(&params.run_id)? else {
            return Ok(None);
        };
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        self.hosted_run_post(
            (url, &access, controller.run_parts),
            params,
            HostedRunResponse::new("hosted run vectors", protocol::RUN_VECTORS_METHOD),
        )
        .await
        .map(Some)
    }

    pub(in super::super) async fn hosted_inbox_list(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::InboxListParams,
    ) -> Result<Option<openengine_cluster_protocol::InboxListResult>, TargetAuthorityError> {
        let (auth, controller) = self
            .descriptors_inner(target, super::super::TargetSessionPurpose::RequestAsk)
            .await?;
        let url = auth.hosted_runs.inbox_list_url()?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        self.hosted_run_post(
            (url, &access, controller.run_parts),
            params,
            HostedRunResponse::new("hosted inbox list", protocol::INBOX_LIST_METHOD),
        )
        .await
        .map(Some)
    }

    pub(in super::super) async fn hosted_run_suspend(
        &self,
        target: &TargetRecord,
        params: openengine_cluster_protocol::RunSuspendParams,
    ) -> Result<Option<zeroshot_engine::native_v2_cli::CliRunStatusResult>, TargetAuthorityError>
    {
        let (auth, controller) = self
            .descriptors_inner(target, super::super::TargetSessionPurpose::RunSuspension)
            .await?;
        let Some(url) = auth.hosted_runs.suspend_url(&params.run_id)? else {
            return Ok(None);
        };
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        self.hosted_run_post(
            (url, &access, controller.run_parts),
            params,
            HostedRunResponse::new("hosted run suspend", protocol::RUN_SUSPEND_METHOD),
        )
        .await
        .map(Some)
    }

    pub(in super::super) async fn hosted_run_resume(
        &self,
        target: &TargetRecord,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, TargetAuthorityError> {
        if let Some(runtime) = &params.runtime {
            crate::native_v2_target::TargetControlAuthority::validate_resume_runtime(
                self, target, runtime,
            )
            .await?;
        }
        if matches!(
            params.from,
            Some(
                openengine_cluster_protocol::RunResumeFrom::Answer { .. }
                    | openengine_cluster_protocol::RunResumeFrom::Expiry {}
            )
        ) {
            crate::native_v2_target::TargetControlAuthority::validate_request_ask(self, target)
                .await?;
        }
        let (routes, access) = self
            .require_hosted_run_access_for(target, params.git_credentials.is_some())
            .await?;
        let url = routes.resume_url(&params.run_id)?;
        self.hosted_run_post(
            (url, &access, routes.run_parts),
            params,
            HostedRunResponse::new("hosted workspace resume", protocol::RUN_RESUME_METHOD),
        )
        .await
    }

    pub(in super::super) async fn hosted_run_checkpoints(
        &self,
        target: &TargetRecord,
        params: RunCheckpointsParams,
    ) -> Result<RunCheckpointsResult, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        let url = routes.checkpoints_url(&params.run_id)?;
        self.hosted_run_post(
            (url, &access, routes.run_parts),
            params,
            HostedRunResponse::new(
                "hosted workspace checkpoints",
                protocol::RUN_CHECKPOINTS_METHOD,
            ),
        )
        .await
    }

    pub(in super::super) async fn hosted_run_discard_workspace(
        &self,
        target: &TargetRecord,
        params: RunDiscardWorkspaceParams,
    ) -> Result<RunDiscardWorkspaceResult, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        let url = routes.discard_workspace_url(&params.run_id)?;
        self.hosted_run_post(
            (url, &access, routes.run_parts),
            params,
            HostedRunResponse::new(
                "hosted workspace discard_workspace",
                protocol::RUN_DISCARD_WORKSPACE_METHOD,
            ),
        )
        .await
    }

    async fn hosted_run_post<P: Serialize, T: DeserializeOwned>(
        &self,
        target: (Url, &AccessToken, bool),
        params: P,
        response: HostedRunResponse,
    ) -> Result<T, TargetAuthorityError> {
        let (url, access, run_parts) = target;
        let request = self
            .authorized(self.client.post(url), access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store");
        let request = if run_parts {
            request.json(&params)
        } else {
            request.json(&part_identity::request(false, &params)?)
        };
        self.hosted_run_json((request, access), run_parts, response)
            .await
    }
}
