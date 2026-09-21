//! GitHub Copilot CLI harness over its pinned headless JSON-RPC protocol.
//!
//! Each execution owns one contained runtime. Corrections share its session; node-instance
//! revisits resume the persisted session in the existing private provider home. Credentials
//! cross private RPC only and are never placed in the process or tool environment.

mod auth;
mod command;
mod events;
mod framing;
mod rpc;
mod session;

use std::path::PathBuf;

use openengine_cluster_protocol::WorkerOutcome;
use crate::execution::process::HostedProcessPool;
use crate::native_v2_capsule::provider_process::{
    ClosedSessionFailure, ProviderExecution, ProviderFilesystemConfig, ProviderProcessRunners,
    open_provider_process, require_process_cleanup,
};
use crate::native_v2_runner::{DriverControl, DriverInvocation, NodeRunnerError};
use session::CopilotSession;

/// Host-owned launch capabilities; credentials belong to each invocation's resolved connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopilotConfig {
    pub executable: PathBuf,
    pub workspace: PathBuf,
    pub runtime_home: PathBuf,
    pub search_path: String,
    pub process_pool: HostedProcessPool,
}

/// Copilot's GitHub-backed harness. Model names remain caller-owned opaque values.
pub struct CopilotAdapter {
    config: CopilotConfig,
    runners: ProviderProcessRunners,
}

impl CopilotAdapter {
    #[must_use]
    pub fn new(config: CopilotConfig) -> Self {
        let runners = ProviderProcessRunners::hosted(config.process_pool);
        Self { config, runners }
    }

    #[must_use]
    pub fn new_local(config: CopilotConfig) -> Self {
        Self {
            config,
            runners: ProviderProcessRunners::local(),
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
        auth::validate(&invocation.environment)?;
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
        let command = command::command(&self.config, invocation, &files)?;
        let mut process = open_provider_process(files.clone(), command, control)
            .await?
            .map_err(rpc::process_error)?;
        let mut connection = rpc::CopilotRpc::new(
            process.detach_stdout(),
            invocation,
            control,
            self.runners.verifier_workspace(),
        );
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let outcome = {
            let exchange = connection.run(sender, session, &files);
            let writer = rpc::write_messages(&process, receiver);
            let mut cancellation = control.cancellation();
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
        };
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
}

#[cfg(test)]
mod tests;
