//! Native-v2 named-target connector.
//!
//! The CLI owns the local name/origin/access profile. A target control authority owns
//! discovery, explicit hosted/direct access, submission, and run-scoped OECP sessions. Runtime
//! values cross only in the ephemeral per-run request and are never stored locally.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroshot_engine::native_v2_cli::oecp::{BoxedSubscription, TargetConnector};
use zeroshot_engine::native_v2_cli::{
    CliRunForceResult, CliRunListResult, CliRunStatusResult, CliRunWatchEventNotification,
    NativeV2CliError, PreparedMergePlanRequest, PreparedRunRequest, TargetAdd,
};
use openengine_cluster_protocol::{
    ConnectionDeleteRequest, ConnectionDeleteResult, ConnectionListRequest, ConnectionListResult,
    ConnectionMutationResult, ConnectionSetRequest, MergePlan, MergePlanId, MergePlanSubmitRequest,
};
use openengine_cluster_protocol::{
    RunConnectionRequirements, RunForceParams, RunListParams, RunLogEventNotification,
    RunLogsParams, RunResumeParams, RunStatusParams, RunSubmission, RunSubmitResult,
    RunWatchParams,
};
use openengine_cluster_protocol::{
    RunProfile, RunProfileDefaultRequest, RunProfileDefaultResult, RunProfileDeleteResult,
    RunProfileListRequest, RunProfileListResult, RunProfileMutationResult, RunProfileRunRequest,
    RunProfileSelector, RunProfileSetRequest, TargetOecpSessionRequest, TargetRunRequest,
};

mod access;
mod authority;
mod authority_error;
mod contract;
mod controller_authority;
mod oecp;
mod registry;
mod serve;

use contract::{prepare_target, validate_bearer_token, validate_target_name};
pub use oecp::{TargetOecpDialer, TargetOecpWebSocketDialer};
pub use registry::{FileTargetRegistry, TargetRegistry, default_target_registry_path};
pub use controller_authority::TargetHttpControlAuthority;
#[cfg(feature = "ui")]
pub use controller_authority::TargetRunHistoryTransport;
pub use serve::{TargetServeError, serve_direct_target};
pub use access::{TargetAccess, TargetOecpAccess};
pub use authority::TargetControlAuthority;
pub use authority_error::TargetAuthorityError;

#[cfg(test)]
use contract::normalize_origin;
#[cfg(test)]
#[path = "native_v2_target/tests.rs"]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TargetRecord {
    pub id: String,
    pub name: String,
    pub origin: String,
    pub access: TargetAccess,
}

impl<'de> Deserialize<'de> for TargetRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        struct StoredTarget {
            id: String,
            name: String,
            origin: String,
            access: TargetAccess,
            #[serde(default)]
            repository: Option<String>,
            #[serde(default)]
            default_branch: Option<String>,
        }
        let stored = StoredTarget::deserialize(deserializer)?;
        let _ = (stored.repository, stored.default_branch);
        Ok(Self {
            id: stored.id,
            name: stored.name,
            origin: stored.origin,
            access: stored.access,
        })
    }
}

#[derive(Debug, Error)]
pub enum TargetConnectorError {
    #[error("target name must be 1-64 ASCII alphanumeric/hyphen characters")]
    InvalidName,
    #[error("target URL must be an HTTPS origin (literal loopback HTTP is allowed)")]
    InvalidOrigin,
    #[error("target {0:?} already exists")]
    AlreadyExists(String),
    #[error("target {0:?} not found")]
    NotFound(String),
    #[error("target registry path cannot be resolved: {0}")]
    RegistryPath(&'static str),
    #[error("target registry I/O failed: {0}")]
    RegistryIo(#[source] std::io::Error),
    #[error("target registry is malformed: {0}")]
    RegistryJson(#[source] serde_json::Error),
    #[error("target registry exceeds 1 MiB")]
    RegistryTooLarge,
    #[error("local workspace-recovery authorization is unavailable")]
    RecoveryAuthorizationUnavailable,
    #[error("workspace-recovery connection requirements do not match the original run")]
    RecoveryAuthorizationMismatch,
    #[error("local workspace-recovery authorization is malformed")]
    RecoveryAuthorizationInvalid,
    #[error("secure randomness is unavailable")]
    Randomness,
    #[error("target control authority failed: {0}")]
    Authority(#[from] TargetAuthorityError),
    #[error("target OECP endpoint is invalid")]
    InvalidOecpEndpoint,
    #[error("target OECP bearer token is invalid")]
    InvalidBearerToken,
    #[error("target OECP connection failed: {0}")]
    OecpConnection(String),
}

pub struct NativeV2TargetConnector<R, A, D> {
    registry: R,
    authority: A,
    dialer: D,
}

enum TargetSessionPurpose {
    General,
    WorkspaceRecovery,
}

impl<R, A, D> NativeV2TargetConnector<R, A, D> {
    #[must_use]
    pub const fn new(registry: R, authority: A, dialer: D) -> Self {
        Self {
            registry,
            authority,
            dialer,
        }
    }
}

#[async_trait]
impl<R, A, D> TargetConnector for NativeV2TargetConnector<R, A, D>
where
    R: TargetRegistry,
    A: TargetControlAuthority,
    D: TargetOecpDialer,
{
    type Transport = D::Transport;

    async fn add(&self, request: TargetAdd) -> Result<(), NativeV2CliError> {
        let target = prepare_target(request).map_err(cli_target_error)?;
        self.authority
            .discover(&target)
            .await
            .map_err(|error| error.into_cli(&target))?;
        self.registry.insert(target).map_err(cli_target_error)
    }

    async fn login(&self, name: &str) -> Result<(), NativeV2CliError> {
        validate_target_name(name).map_err(cli_target_error)?;
        let target = self.registry.get(name).map_err(cli_target_error)?;
        self.authority
            .login(&target)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn connection_list(
        &self,
        name: &str,
        request: ConnectionListRequest,
    ) -> Result<ConnectionListResult, NativeV2CliError> {
        validate_target_name(name).map_err(cli_target_error)?;
        let target = self.registry.get(name).map_err(cli_target_error)?;
        self.authority
            .connection_list(&target, request)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn connection_set(
        &self,
        name: &str,
        request: ConnectionSetRequest,
    ) -> Result<ConnectionMutationResult, NativeV2CliError> {
        validate_target_name(name).map_err(cli_target_error)?;
        let target = self.registry.get(name).map_err(cli_target_error)?;
        self.authority
            .connection_set(&target, request)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn connection_delete(
        &self,
        name: &str,
        request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResult, NativeV2CliError> {
        validate_target_name(name).map_err(cli_target_error)?;
        let target = self.registry.get(name).map_err(cli_target_error)?;
        self.authority
            .connection_delete(&target, request)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn profile_list(
        &self,
        name: &str,
        request: RunProfileListRequest,
    ) -> Result<RunProfileListResult, NativeV2CliError> {
        let target = self.target(name)?;
        self.authority
            .profile_list(&target, request)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn profile_show(
        &self,
        name: &str,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, NativeV2CliError> {
        let target = self.target(name)?;
        self.authority
            .profile_show(&target, selector)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn profile_set(
        &self,
        name: &str,
        request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, NativeV2CliError> {
        let target = self.target(name)?;
        self.authority
            .profile_set(&target, request)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn profile_delete(
        &self,
        name: &str,
        selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, NativeV2CliError> {
        let target = self.target(name)?;
        self.authority
            .profile_delete(&target, selector)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn profile_default(
        &self,
        name: &str,
        request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, NativeV2CliError> {
        let target = self.target(name)?;
        self.authority
            .profile_default(&target, request)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn merge_plan_submit(
        &self,
        name: &str,
        request: PreparedMergePlanRequest,
    ) -> Result<MergePlan, NativeV2CliError> {
        let target = self.target(name)?;
        if matches!(target.access, TargetAccess::Direct) {
            return Err(NativeV2CliError::Target(
                "direct target does not support hosted merge plans".to_owned(),
            ));
        }
        self.authority
            .merge_plan_submit(
                &target,
                &MergePlanSubmitRequest {
                    submission_key: request.submission_key,
                    title: request.title,
                    expires_at: request.expires_at,
                    source: request.source,
                    profile: request.profile,
                    runs: request.runs,
                    connections: request.connections,
                    github_token: request.github_token,
                },
            )
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn merge_plan_status(
        &self,
        name: &str,
        plan_id: MergePlanId,
    ) -> Result<MergePlan, NativeV2CliError> {
        let target = self.target(name)?;
        self.authority
            .merge_plan_status(&target, &plan_id)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn merge_plan_force(
        &self,
        name: &str,
        plan_id: MergePlanId,
    ) -> Result<MergePlan, NativeV2CliError> {
        let target = self.target(name)?;
        self.authority
            .merge_plan_force(&target, &plan_id)
            .await
            .map_err(|error| error.into_cli(&target))
    }

    async fn submit(
        &self,
        name: &str,
        request: PreparedRunRequest,
    ) -> Result<RunSubmitResult, NativeV2CliError> {
        validate_target_name(name).map_err(cli_target_error)?;
        let target = self.registry.get(name).map_err(cli_target_error)?;
        let source = request
            .source
            .as_ref()
            .ok_or_else(|| {
                NativeV2CliError::Usage(
                    "named target submission requires a resolved worktree source".to_owned(),
                )
            })?
            .resolved
            .clone();
        let recovery_requirements =
            self.record_initial_recovery_authorization(&target, &request)?;
        let receipt = if let Some(profile) = request.profile {
            self.authority
                .profile_run(
                    &target,
                    &RunProfileRunRequest {
                        run_id: request.run_id,
                        profile,
                        title: request.intent.title,
                        initial_input: request.intent.initial_input,
                        source,
                        submission_key: request.intent.submission_key,
                        connections: request.connections,
                        github_token: request.github_token,
                    },
                )
                .await
                .map_err(|error| error.into_cli(&target))?
        } else {
            self.authority
                .submit(
                    &target,
                    &TargetRunRequest {
                        run_id: request.run_id,
                        submission: RunSubmission {
                            title: request.intent.title,
                            graph: request.intent.graph,
                            initial_input: request.intent.initial_input,
                            runtime: request.intent.runtime,
                            source,
                            submission_key: request.intent.submission_key,
                        },
                        connections: request.connections,
                        connection_resolver: None,
                        github_token: request.github_token,
                    },
                )
                .await
                .map_err(|error| error.into_cli(&target))?
        };
        self.record_receipt_recovery_authorization(
            &target,
            &receipt.run_id,
            recovery_requirements.as_ref(),
        )?;
        Ok(receipt)
    }

    async fn connect(
        &self,
        name: &str,
        run_id: Option<openengine_cluster_protocol::RunId>,
    ) -> Result<Arc<Self::Transport>, NativeV2CliError> {
        self.connect_session(name, run_id, TargetSessionPurpose::General)
            .await
    }

    async fn connect_workspace_recovery(
        &self,
        name: &str,
        run_id: openengine_cluster_protocol::RunId,
    ) -> Result<Arc<Self::Transport>, NativeV2CliError> {
        self.connect_session(name, Some(run_id), TargetSessionPurpose::WorkspaceRecovery)
            .await
    }

    fn authorize_workspace_recovery_requirements(
        &self,
        name: &str,
        run_id: &openengine_cluster_protocol::RunId,
        requirements: RunConnectionRequirements,
    ) -> Result<RunConnectionRequirements, NativeV2CliError> {
        let target = self.direct_recovery_target(name)?;
        let trusted = self
            .registry
            .recovery_authorization(&target.id, run_id)
            .map_err(cli_target_error)?;
        if trusted != requirements {
            return Err(cli_target_error(
                TargetConnectorError::RecoveryAuthorizationMismatch,
            ));
        }
        Ok(trusted)
    }

    fn prepare_workspace_recovery_resume(
        &self,
        name: &str,
        params: &RunResumeParams,
    ) -> Result<(), NativeV2CliError> {
        let target = self.direct_recovery_target(name)?;
        let trusted = self
            .registry
            .recovery_authorization(&target.id, &params.run_id)
            .map_err(cli_target_error)?;
        if params.connection_resolver.is_some()
            || !connection_values_match_requirements(&params.connections, &trusted)
        {
            return Err(cli_target_error(
                TargetConnectorError::RecoveryAuthorizationMismatch,
            ));
        }
        self.registry
            .record_recovery_authorization(&target.id, &params.successor_run_id, &trusted)
            .map_err(cli_target_error)
    }

    fn revoke_workspace_recovery(
        &self,
        name: &str,
        run_id: &openengine_cluster_protocol::RunId,
    ) -> Result<(), NativeV2CliError> {
        let target = self.direct_recovery_target(name)?;
        self.registry
            .remove_recovery_authorization(&target.id, run_id)
            .map_err(cli_target_error)
    }

    async fn hosted_run_list(
        &self,
        name: &str,
        params: RunListParams,
    ) -> Result<Option<CliRunListResult>, NativeV2CliError> {
        let target = self.target(name)?;
        if matches!(target.access, TargetAccess::Direct) {
            return Ok(None);
        }
        self.authority
            .hosted_run_list(&target, params)
            .await
            .map_err(|error| error.into_cli(&target))
            .map(Some)
    }

    async fn hosted_run_status(
        &self,
        name: &str,
        params: RunStatusParams,
    ) -> Result<Option<CliRunStatusResult>, NativeV2CliError> {
        let target = self.target(name)?;
        if matches!(target.access, TargetAccess::Direct) {
            return Ok(None);
        }
        self.authority
            .hosted_run_status(&target, params)
            .await
            .map_err(|error| error.into_cli(&target))
            .map(Some)
    }

    async fn hosted_run_watch(
        &self,
        name: &str,
        params: RunWatchParams,
    ) -> Result<Option<BoxedSubscription<CliRunWatchEventNotification>>, NativeV2CliError> {
        let target = self.target(name)?;
        if matches!(target.access, TargetAccess::Direct) {
            return Ok(None);
        }
        self.authority
            .hosted_run_watch(&target, params)
            .await
            .map_err(|error| error.into_cli(&target))
            .map(Some)
    }

    async fn hosted_run_logs(
        &self,
        name: &str,
        params: RunLogsParams,
    ) -> Result<Option<BoxedSubscription<RunLogEventNotification>>, NativeV2CliError> {
        let target = self.target(name)?;
        if matches!(target.access, TargetAccess::Direct) {
            return Ok(None);
        }
        self.authority
            .hosted_run_logs(&target, params)
            .await
            .map_err(|error| error.into_cli(&target))
            .map(Some)
    }

    async fn hosted_run_force(
        &self,
        name: &str,
        params: RunForceParams,
    ) -> Result<Option<CliRunForceResult>, NativeV2CliError> {
        let target = self.target(name)?;
        if matches!(target.access, TargetAccess::Direct) {
            return Ok(None);
        }
        self.authority
            .hosted_run_force(&target, params)
            .await
            .map_err(|error| error.into_cli(&target))
            .map(Some)
    }
}

impl<R, A, D> NativeV2TargetConnector<R, A, D>
where
    R: TargetRegistry,
    A: TargetControlAuthority,
    D: TargetOecpDialer,
{
    async fn connect_session(
        &self,
        name: &str,
        run_id: Option<openengine_cluster_protocol::RunId>,
        purpose: TargetSessionPurpose,
    ) -> Result<Arc<D::Transport>, NativeV2CliError> {
        validate_target_name(name).map_err(cli_target_error)?;
        let target = self.registry.get(name).map_err(cli_target_error)?;
        let request = TargetOecpSessionRequest { run_id };
        let session = match purpose {
            TargetSessionPurpose::General => self.authority.oecp_session(&target, &request).await,
            TargetSessionPurpose::WorkspaceRecovery => {
                self.authority
                    .workspace_recovery_session(&target, &request)
                    .await
            }
        }
        .map_err(|error| error.into_cli(&target))?;
        self.dialer
            .dial(&target, session)
            .await
            .map_err(|error| cli_connector_error(&target, error))
    }
}

impl<R, A, D> NativeV2TargetConnector<R, A, D>
where
    R: TargetRegistry,
{
    fn record_initial_recovery_authorization(
        &self,
        target: &TargetRecord,
        request: &PreparedRunRequest,
    ) -> Result<Option<RunConnectionRequirements>, NativeV2CliError> {
        if !matches!(target.access, TargetAccess::Direct) {
            return Ok(None);
        }
        let requirements = runtime_connection_requirements(&request.intent.runtime);
        self.registry
            .record_recovery_authorization(&target.id, &request.run_id, &requirements)
            .map_err(cli_target_error)?;
        Ok(Some(requirements))
    }

    fn record_receipt_recovery_authorization(
        &self,
        target: &TargetRecord,
        run_id: &openengine_cluster_protocol::RunId,
        requirements: Option<&RunConnectionRequirements>,
    ) -> Result<(), NativeV2CliError> {
        let Some(requirements) = requirements else {
            return Ok(());
        };
        self.registry
            .record_recovery_authorization(&target.id, run_id, requirements)
            .map_err(cli_target_error)
    }

    fn target(&self, name: &str) -> Result<TargetRecord, NativeV2CliError> {
        validate_target_name(name).map_err(cli_target_error)?;
        self.registry.get(name).map_err(cli_target_error)
    }

    fn direct_recovery_target(&self, name: &str) -> Result<TargetRecord, NativeV2CliError> {
        let target = self.target(name)?;
        if !matches!(target.access, TargetAccess::Direct) {
            return Err(NativeV2CliError::Target(
                "target does not advertise workspace recovery".to_owned(),
            ));
        }
        Ok(target)
    }
}

fn runtime_connection_requirements(
    runtime: &openengine_cluster_protocol::RuntimePlan,
) -> RunConnectionRequirements {
    runtime
        .connection_requirements()
        .into_iter()
        .map(|(key, fields)| (key, fields.into_iter().collect()))
        .collect()
}

fn connection_values_match_requirements(
    values: &openengine_cluster_protocol::RunConnectionValues,
    requirements: &RunConnectionRequirements,
) -> bool {
    values.iter().all(|(key, values)| {
        requirements
            .get(key)
            .is_some_and(|fields| values.as_map().keys().all(|name| fields.contains(name)))
    })
}

fn cli_target_error(error: impl fmt::Display) -> NativeV2CliError {
    NativeV2CliError::Target(error.to_string())
}

fn cli_connector_error(target: &TargetRecord, error: TargetConnectorError) -> NativeV2CliError {
    match error {
        TargetConnectorError::OecpConnection(_) => {
            TargetAuthorityError::disconnected("WebSocket connection failed").into_cli(target)
        }
        error => cli_target_error(error),
    }
}
