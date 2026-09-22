//! GitHub Copilot CLI harness over its pinned headless JSON-RPC protocol.
//!
//! Each execution owns one contained runtime. Corrections share its session; node-instance
//! revisits resume the persisted session in the existing private provider home. Credentials
//! cross private RPC only and are never placed in the process or tool environment.

mod auth;
mod command;
mod events;
mod framing;
mod provider;
mod rpc;
mod session;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use openengine_cluster_protocol::WorkerOutcome;
use crate::execution::process::HostedProcessPool;
use crate::native_v2_capsule::provider_process::{
    ClosedSessionFailure, ProviderExecution, ProviderFilesystemConfig, ProviderProcessRunners,
    ProviderExecutionFiles, ProviderProcess, open_provider_process, require_process_cleanup,
};
use crate::native_v2_runner::{DriverControl, DriverInvocation, NodeRunnerError, ResolvedEnvironment};
use session::CopilotSession;

/// Host-owned launch capabilities; credentials belong to each invocation's resolved connection.
#[derive(Clone, Eq, PartialEq)]
pub struct CopilotConfig {
    pub executable: PathBuf,
    pub workspace: PathBuf,
    pub runtime_home: PathBuf,
    /// Current-user paths are available only to the built-in local target.
    pub local_user: Option<CopilotLocalUser>,
    pub base_environment: BTreeMap<String, String>,
    /// Private invoking-shell context used only by a local BYOK key command.
    pub local_command_environment: BTreeMap<String, String>,
    pub search_path: String,
    pub process_pool: HostedProcessPool,
}

impl fmt::Debug for CopilotConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CopilotConfig")
            .field("executable", &self.executable)
            .field("workspace", &self.workspace)
            .field("runtime_home", &self.runtime_home)
            .field("local_user", &self.local_user)
            .field("base_environment_fields", &self.base_environment.len())
            .field(
                "local_command_environment_fields",
                &self.local_command_environment.len(),
            )
            .field("search_path", &self.search_path)
            .field("process_pool", &self.process_pool)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopilotLocalUser {
    pub home: PathBuf,
    pub copilot_home: PathBuf,
}

/// Copilot's GitHub-backed harness. Model names remain caller-owned opaque values.
pub struct CopilotAdapter {
    config: CopilotConfig,
    runners: ProviderProcessRunners,
    local_token: Option<String>,
    local_provider: Option<provider::LocalProviderContext>,
}

impl CopilotAdapter {
    #[must_use]
    pub fn new(mut config: CopilotConfig) -> Self {
        config.local_user = None;
        auth::remove_local_credentials(&mut config.base_environment);
        provider::remove_local_configuration(&mut config.base_environment);
        config.local_command_environment.clear();
        let runners = ProviderProcessRunners::hosted(config.process_pool);
        Self {
            config,
            runners,
            local_token: None,
            local_provider: None,
        }
    }

    #[must_use]
    pub fn new_local(mut config: CopilotConfig) -> Self {
        let local_token = config
            .local_user
            .is_some()
            .then(|| auth::take_local_token(&mut config.base_environment))
            .flatten();
        let local_provider = config.local_user.as_ref().map(|user| {
            provider::take_local_configuration(&mut config.base_environment, &user.copilot_home)
        });
        Self {
            config,
            runners: ProviderProcessRunners::local(),
            local_token,
            local_provider,
        }
    }

    async fn run_turn(
        &self,
        invocation: &DriverInvocation,
        session: &CopilotSession,
        control: &DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let _turn = session.core.turn.lock().await;
        session.core.ensure_live(ClosedSessionFailure::Driver)?;
        let authentication = auth::CopilotAuthentication::new(
            &invocation.environment,
            self.local_token.as_deref(),
            self.config.local_user.is_some(),
            &self.config.base_environment,
        )?;
        let local_provider = self.provider_configuration(
            &invocation.environment,
            &authentication,
            command::agent_model(invocation)?,
        )?;
        let execution = ProviderExecution::new(
            ProviderFilesystemConfig {
                runners: self.runners,
                root: &self.config.runtime_home,
                workspace: &self.config.workspace,
            },
            invocation,
            &session.core,
        );
        let files = execution
            .prepare(control)
            .await
            .map_err(rpc::process_error)?;
        let command = command::command(&self.config, invocation, &files, local_provider.as_ref())?;
        let mut process = open_provider_process(files.clone(), command, control)
            .await?
            .map_err(rpc::process_error)?;
        let mut connection = rpc::CopilotRpc::new(
            process.detach_stdout(),
            invocation,
            control,
            rpc::CopilotRpcNative {
                verifier_workspace: self.runners.verifier_workspace(),
                authentication,
                provider: local_provider.as_ref(),
                redactions: self
                    .provider_redactions(&invocation.environment, local_provider.as_ref()),
            },
        );
        let outcome = exchange_rpc(&mut connection, &process, session, &files).await;
        finish_rpc(&mut connection, &mut process, control, outcome).await
    }

    fn provider_configuration(
        &self,
        environment: &ResolvedEnvironment,
        authentication: &auth::CopilotAuthentication<'_>,
        model: &str,
    ) -> Result<Option<provider::LocalProvider>, NodeRunnerError> {
        if self.config.local_user.is_none() || authentication.has_declared_token() {
            return Ok(None);
        }
        self.local_provider.as_ref().map_or(Ok(None), |ambient| {
            provider::resolve_local_configuration(ambient, environment, model)
        })
    }

    fn provider_redactions(
        &self,
        environment: &ResolvedEnvironment,
        rpc: Option<&provider::LocalProvider>,
    ) -> Vec<String> {
        crate::native_v2_capsule::provider_process::provider_redactions(
            environment,
            &self.config.base_environment,
        )
        .into_iter()
        .chain(
            rpc.into_iter()
                .flat_map(provider::LocalProvider::redactions),
        )
        .collect()
    }
}

async fn finish_rpc(
    connection: &mut rpc::CopilotRpc<'_>,
    process: &mut ProviderProcess,
    control: &DriverControl,
    outcome: Result<WorkerOutcome, NodeRunnerError>,
) -> Result<WorkerOutcome, NodeRunnerError> {
    // Stop the owned tree while draining its remaining events, including token usage.
    // Releasing an intentionally completed headless server is not a provider cancellation.
    let (completion, drained) = tokio::join!(process.release(), connection.drain());
    let completion = require_process_cleanup(&completion)?;
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    drained?;
    outcome.map_err(|error| connection.failure_diagnostic(error, completion))
}

async fn exchange_rpc(
    connection: &mut rpc::CopilotRpc<'_>,
    process: &ProviderProcess,
    session: &CopilotSession,
    files: &ProviderExecutionFiles,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    let mut cancellation = connection.control.cancellation();
    let exchange = connection.run(sender, session, files);
    let writer = rpc::write_messages(process, receiver);
    tokio::pin!(exchange, writer);
    tokio::select! {
        result = &mut exchange => match result {
            Ok(outcome) => writer.await.map(|()| outcome),
            Err(error) => Err(error),
        },
        result = &mut writer => match result {
            Ok(()) => exchange.await,
            Err(error) => Err(error),
        },
        () = cancellation.cancelled() => Err(NodeRunnerError::Cancelled),
    }
}

#[cfg(test)]
mod tests;
