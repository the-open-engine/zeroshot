use openengine_cluster_protocol::{
    RunCheckpointsParams, RunCheckpointsResult, RunDiscardWorkspaceParams,
    RunDiscardWorkspaceResult, RunResumeParams, RunResumeResult,
};
use reqwest::Url;
use reqwest::header::{ACCEPT, CACHE_CONTROL};
use serde::{Serialize, de::DeserializeOwned};

use super::{AccessToken, TargetAuthorityError, TargetHttpControlAuthority, TargetRecord};

impl TargetHttpControlAuthority {
    pub(in super::super) async fn hosted_run_resume(
        &self,
        target: &TargetRecord,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
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
