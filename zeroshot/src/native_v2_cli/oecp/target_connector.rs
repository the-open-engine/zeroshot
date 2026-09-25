use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_client::SubscriptionTransport;
use openengine_cluster_protocol::{
    EnvironmentId, RuntimeEnvironmentResource, RunProfileScope, ConnectionDeleteRequest,
    ConnectionDeleteResult, ConnectionListRequest, ConnectionListResult, ConnectionMutationResult,
    ConnectionSetRequest, MergePlan, MergePlanId, RunForceParams, RunConnectionRequirements,
    RunListParams, RunLogEventNotification, RunLogsParams, RunProfile, RunProfileDefaultRequest,
    RunProfileDefaultResult, RunProfileDeleteResult, RunProfileListRequest, RunProfileListResult,
    RunProfileMutationResult, RunProfileSelector, RunProfileSetRequest, RunResumeParams,
    RunStatusParams, RunSubmitResult, RunWatchParams,
};

use super::BoxedSubscription;
use crate::native_v2_cli::{
    CliRunForceResult, CliRunListResult, CliRunStatusResult, CliRunWatchEventNotification,
    NativeV2CliError, PreparedMergePlanRequest, PreparedRunRequest, TargetAdd,
};

/// Named-target authority. The CLI does not interpret login credentials or runtime configuration.
#[async_trait]
pub trait TargetConnector: Send + Sync {
    type Transport: SubscriptionTransport + Send + Sync + 'static;

    async fn add(&self, request: TargetAdd) -> Result<(), NativeV2CliError>;
    async fn login(&self, name: &str) -> Result<(), NativeV2CliError>;
    async fn connection_list(
        &self,
        name: &str,
        request: ConnectionListRequest,
    ) -> Result<ConnectionListResult, NativeV2CliError>;
    async fn connection_set(
        &self,
        name: &str,
        request: ConnectionSetRequest,
    ) -> Result<ConnectionMutationResult, NativeV2CliError>;
    async fn connection_delete(
        &self,
        name: &str,
        request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResult, NativeV2CliError>;
    async fn environment_show(
        &self,
        _name: &str,
        _scope: RunProfileScope,
        _id: EnvironmentId,
    ) -> Result<RuntimeEnvironmentResource, NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "target does not advertise environment resources".into(),
        ))
    }
    async fn profile_list(
        &self,
        name: &str,
        request: RunProfileListRequest,
    ) -> Result<RunProfileListResult, NativeV2CliError>;
    async fn profile_show(
        &self,
        name: &str,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, NativeV2CliError>;
    async fn profile_set(
        &self,
        name: &str,
        request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, NativeV2CliError>;
    async fn profile_delete(
        &self,
        name: &str,
        selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, NativeV2CliError>;
    async fn profile_default(
        &self,
        name: &str,
        request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, NativeV2CliError>;
    async fn merge_plan_submit(
        &self,
        _name: &str,
        _request: PreparedMergePlanRequest,
    ) -> Result<MergePlan, NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "target does not advertise merge plans".to_owned(),
        ))
    }
    async fn merge_plan_status(
        &self,
        _name: &str,
        _plan_id: MergePlanId,
    ) -> Result<MergePlan, NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "target does not advertise merge plans".to_owned(),
        ))
    }
    async fn merge_plan_force(
        &self,
        _name: &str,
        _plan_id: MergePlanId,
    ) -> Result<MergePlan, NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "target does not advertise merge plans".to_owned(),
        ))
    }
    async fn submit(
        &self,
        name: &str,
        request: PreparedRunRequest,
    ) -> Result<RunSubmitResult, NativeV2CliError>;
    async fn connect(
        &self,
        name: &str,
        run_id: Option<openengine_cluster_protocol::RunId>,
    ) -> Result<Arc<Self::Transport>, NativeV2CliError>;
    async fn connect_workspace_checkpoints(
        &self,
        _name: &str,
        _run_id: openengine_cluster_protocol::RunId,
    ) -> Result<Arc<Self::Transport>, NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "target does not advertise workspace checkpoints".to_owned(),
        ))
    }
    async fn connect_workspace_recovery(
        &self,
        _name: &str,
        _run_id: openengine_cluster_protocol::RunId,
    ) -> Result<Arc<Self::Transport>, NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "target does not advertise workspace recovery".to_owned(),
        ))
    }
    fn authorize_workspace_recovery_requirements(
        &self,
        _name: &str,
        _run_id: &openengine_cluster_protocol::RunId,
        _requirements: RunConnectionRequirements,
    ) -> Result<RunConnectionRequirements, NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "local authorization for workspace recovery is unavailable".to_owned(),
        ))
    }
    fn prepare_workspace_recovery_resume(
        &self,
        _name: &str,
        _params: &RunResumeParams,
    ) -> Result<(), NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "local authorization for workspace recovery is unavailable".to_owned(),
        ))
    }
    fn revoke_workspace_recovery(
        &self,
        _name: &str,
        _run_id: &openengine_cluster_protocol::RunId,
    ) -> Result<(), NativeV2CliError> {
        Err(NativeV2CliError::Target(
            "local authorization for workspace recovery is unavailable".to_owned(),
        ))
    }
    async fn hosted_run_resume(
        &self,
        _name: &str,
        _params: openengine_cluster_protocol::RunResumeParams,
    ) -> Result<Option<openengine_cluster_protocol::RunResumeResult>, NativeV2CliError> {
        Ok(None)
    }
    async fn hosted_run_checkpoints(
        &self,
        _name: &str,
        _params: openengine_cluster_protocol::RunCheckpointsParams,
    ) -> Result<Option<openengine_cluster_protocol::RunCheckpointsResult>, NativeV2CliError> {
        Ok(None)
    }
    async fn hosted_run_discard_workspace(
        &self,
        _name: &str,
        _params: openengine_cluster_protocol::RunDiscardWorkspaceParams,
    ) -> Result<Option<openengine_cluster_protocol::RunDiscardWorkspaceResult>, NativeV2CliError>
    {
        Ok(None)
    }
    async fn hosted_run_list(
        &self,
        name: &str,
        params: RunListParams,
    ) -> Result<Option<CliRunListResult>, NativeV2CliError>;
    async fn hosted_run_status(
        &self,
        name: &str,
        params: RunStatusParams,
    ) -> Result<Option<CliRunStatusResult>, NativeV2CliError>;
    async fn hosted_run_watch(
        &self,
        name: &str,
        params: RunWatchParams,
    ) -> Result<Option<BoxedSubscription<CliRunWatchEventNotification>>, NativeV2CliError>;
    async fn hosted_run_logs(
        &self,
        name: &str,
        params: RunLogsParams,
    ) -> Result<Option<BoxedSubscription<RunLogEventNotification>>, NativeV2CliError>;
    async fn hosted_run_force(
        &self,
        name: &str,
        params: RunForceParams,
    ) -> Result<Option<CliRunForceResult>, NativeV2CliError>;
}
