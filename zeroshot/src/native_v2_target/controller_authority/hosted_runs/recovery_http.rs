use openengine_cluster_protocol::{
    RunCheckpointsParams, RunCheckpointsResult, RunDiscardWorkspaceParams,
    RunDiscardWorkspaceResult, RunResumeParams, RunResumeResult,
};
use reqwest::Url;
use reqwest::header::{ACCEPT, CACHE_CONTROL};
use serde::{Serialize, de::DeserializeOwned};

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
        let request = self
            .authorized(self.client.post(url), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(&params);
        self.hosted_json(
            (request, &access),
            "hosted run vector",
            Some(openengine_cluster_protocol::MAX_VECTOR_RESPONSE_BYTES),
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
        let request = self
            .authorized(self.client.post(url), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(&params);
        self.hosted_json(
            (request, &access),
            "hosted inbox show",
            Some(openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES),
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
        let request = self
            .authorized(self.client.post(url), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(&params);
        self.hosted_json((request, &access), "hosted run vectors", None)
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
        let request = self
            .authorized(self.client.post(url), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(&params);
        self.hosted_json((request, &access), "hosted inbox list", None)
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
        let request = self
            .authorized(self.client.post(url), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(&params);
        self.hosted_json((request, &access), "hosted run suspend", None)
            .await
            .map(Some)
    }

    pub(in super::super) async fn hosted_run_resume(
        &self,
        target: &TargetRecord,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, TargetAuthorityError> {
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
        self.hosted_recovery_json((url, &access), params, "hosted workspace resume")
            .await
    }

    pub(in super::super) async fn hosted_run_checkpoints(
        &self,
        target: &TargetRecord,
        params: RunCheckpointsParams,
    ) -> Result<RunCheckpointsResult, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        let url = routes.checkpoints_url(&params.run_id)?;
        self.hosted_recovery_json((url, &access), params, "hosted workspace checkpoints")
            .await
    }

    pub(in super::super) async fn hosted_run_discard_workspace(
        &self,
        target: &TargetRecord,
        params: RunDiscardWorkspaceParams,
    ) -> Result<RunDiscardWorkspaceResult, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        let url = routes.discard_workspace_url(&params.run_id)?;
        self.hosted_recovery_json((url, &access), params, "hosted workspace discard_workspace")
            .await
    }

    async fn hosted_recovery_json<P: Serialize, T: DeserializeOwned>(
        &self,
        context: (Url, &AccessToken),
        params: P,
        operation: &'static str,
    ) -> Result<T, TargetAuthorityError> {
        let (url, access) = context;
        let request = self
            .authorized(self.client.post(url), access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(&params);
        self.hosted_json((request, access), operation, None).await
    }
}
