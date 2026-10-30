//! GitHub Copilot CLI harness over its pinned headless JSON-RPC protocol.
//!
//! Each execution owns one contained runtime. Corrections share its session; node-instance
//! revisits resume the persisted session in the existing private provider home. Credentials
//! cross private RPC only and are never placed in the process or tool environment.

mod access;
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
    ProviderProcess, open_provider_process, require_process_cleanup, provider_failure_diagnostic,
    redact_provider_error, report_cleanup_failure,
};
use crate::native_v2_runner::{
    AgentResponseState, DriverControl, DriverInvocation, LiveOutput, LiveOutputStream,
    NodeRunnerError, ResolvedEnvironment,
};
use session::CopilotSession;
use crate::native_v2_contract::CopilotProvider;

const MAX_GRAMMAR_RATE_LIMIT_RETRIES: usize = 3;

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

/// Copilot's native harness. Model names remain caller-owned opaque values.
pub struct CopilotAdapter {
    provider: CopilotProvider,
    config: CopilotConfig,
    runners: ProviderProcessRunners,
    local_token: Option<String>,
    local_provider: Option<provider::LocalProviderContext>,
    active_memory: Option<std::sync::Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
}

crate::active_memory::hooks::impl_active_memory_adapter!(CopilotAdapter);

impl CopilotAdapter {
    #[must_use]
    pub fn new(mut config: CopilotConfig) -> Self {
        config.local_user = None;
        auth::remove_local_credentials(&mut config.base_environment);
        provider::remove_local_configuration(&mut config.base_environment);
        config.local_command_environment.clear();
        let runners = ProviderProcessRunners::hosted(config.process_pool);
        Self {
            provider: CopilotProvider::Github,
            config,
            runners,
            local_token: None,
            local_provider: None,
            active_memory: None,
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
            provider: CopilotProvider::Github,
            config,
            runners: ProviderProcessRunners::local(),
            local_token,
            local_provider,
            active_memory: None,
        }
    }

    /// Select one provider for this adapter while preserving the legacy GitHub default.
    #[must_use]
    pub fn with_provider(mut self, provider: CopilotProvider) -> Self {
        self.provider = provider;
        self
    }

    fn authentication<'a>(
        &'a self,
        environment: &'a ResolvedEnvironment,
    ) -> Result<auth::CopilotAuthentication<'a>, NodeRunnerError> {
        if self.provider == CopilotProvider::Github {
            auth::CopilotAuthentication::new(
                environment,
                self.local_token.as_deref(),
                self.config.local_user.is_some(),
                &self.config.base_environment,
            )
        } else {
            auth::CopilotAuthentication::external(environment)
        }
    }

    async fn run_turn(
        &self,
        invocation: &DriverInvocation,
        session: &CopilotSession,
        control: &DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let _turn = session.core.turn.lock().await;
        let prompt = invocation.agent_prompt()?;
        let mut response = AgentResponseState::new(prompt);
        let mut retries = 0;
        loop {
            let result = self
                .run_attempt(
                    invocation,
                    control,
                    CopilotAttempt {
                        session,
                        response: &mut response,
                    },
                )
                .await;
            let retryable_grammar_rate_limit = result.retryable_grammar_rate_limit;

            match result.outcome {
                Err(NodeRunnerError::DriverDetail(_))
                    if retryable_grammar_rate_limit && retries < MAX_GRAMMAR_RATE_LIMIT_RETRIES =>
                {
                    wait_for_grammar_retry(session, retries, control).await?;
                    retries += 1;
                }
                Err(NodeRunnerError::DriverDetail(message)) if retryable_grammar_rate_limit => {
                    return Err(rpc::failure(format!(
                        "Copilot grammar compilation rate limit persisted after {retries} retries: {message}"
                    )));
                }
                result => return result,
            }
        }
    }

    async fn run_attempt(
        &self,
        invocation: &DriverInvocation,
        control: &DriverControl,
        turn: CopilotAttempt<'_>,
    ) -> FinishedRpc {
        let mut redactions = self.provider_redactions(&invocation.environment, None);
        redactions.extend(self.local_token.iter().cloned());
        let mut result = async {
            turn.session
                .core
                .ensure_live(ClosedSessionFailure::Driver)?;
            let authentication = self.authentication(&invocation.environment)?;
            let local_provider = self.provider_configuration(
                &invocation.environment,
                &authentication,
                command::agent_model(invocation)?,
            )?;
            redactions.extend(
                local_provider
                    .iter()
                    .flat_map(provider::LocalProvider::redactions),
            );
            let execution = ProviderExecution::new(
                ProviderFilesystemConfig {
                    runners: self.runners,
                    root: &self.config.runtime_home,
                    workspace: &self.config.workspace,
                },
                invocation,
                &turn.session.core,
            );
            let files = execution
                .prepare(control)
                .await
                .map_err(rpc::process_error)?;
            let command =
                command::command(&self.config, invocation, &files, local_provider.as_ref())?;
            let mut process = open_provider_process(files.clone(), command, control)
                .await?
                .map_err(rpc::process_error)?;
            let mut connection = rpc::CopilotRpc::new(
                process.detach_stdout(),
                invocation,
                control,
                rpc::CopilotRpcNative {
                    authentication,
                    provider: local_provider.as_ref(),
                    redactions: redactions.clone(),
                },
            )
            .with_active_memory(self.active_memory.clone(), files.workspace.clone());
            let outcome = exchange_rpc(
                &mut connection,
                &process,
                rpc::CopilotRpcTurn {
                    session: turn.session,
                    files: &files,
                    agent_response: turn.response,
                },
            )
            .await;
            Ok(finish_rpc(&mut connection, &mut process, control, outcome).await)
        }
        .await
        .unwrap_or_else(|error| FinishedRpc {
            outcome: Err(error),
            retryable_grammar_rate_limit: false,
        });
        result.outcome = result
            .outcome
            .map_err(|error| redact_provider_error(error, &redactions));
        result
    }

    fn provider_configuration(
        &self,
        environment: &ResolvedEnvironment,
        authentication: &auth::CopilotAuthentication<'_>,
        model: &str,
    ) -> Result<Option<provider::LocalProvider>, NodeRunnerError> {
        if self.provider != CopilotProvider::Github {
            return provider::resolve_explicit_configuration(
                self.provider,
                self.local_provider.as_ref(),
                environment,
                model,
            )
            .map(Some);
        }
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

struct CopilotAttempt<'a> {
    session: &'a CopilotSession,
    response: &'a mut AgentResponseState,
}

async fn wait_for_grammar_retry(
    session: &CopilotSession,
    retries: usize,
    control: &DriverControl,
) -> Result<(), NodeRunnerError> {
    let session_id = session.id.lock().await.clone().unwrap_or_default();
    let delay = rpc::grammar_rate_limit_retry_delay(retries, &session_id);
    control
        .emit(LiveOutput::new(
            LiveOutputStream::System,
            format!(
                "Copilot grammar compilation rate limited; retrying ({}/{}) in {} ms.",
                retries + 1,
                MAX_GRAMMAR_RATE_LIMIT_RETRIES,
                delay.as_millis(),
            ),
        )?)
        .await?;
    rpc::wait_for_grammar_rate_limit_retry(control, delay).await?;
    Ok(())
}

struct FinishedRpc {
    outcome: Result<WorkerOutcome, NodeRunnerError>,
    retryable_grammar_rate_limit: bool,
}

async fn finish_rpc(
    connection: &mut rpc::CopilotRpc<'_>,
    process: &mut ProviderProcess,
    control: &DriverControl,
    outcome: Result<WorkerOutcome, NodeRunnerError>,
) -> FinishedRpc {
    // Stop the owned tree while draining its remaining events, including token usage.
    // Releasing an intentionally completed headless server is not a provider cancellation.
    let (completion, drained) = tokio::join!(process.release(), connection.drain());
    let retryable_grammar_rate_limit = drained.is_ok()
        && matches!(
            &outcome,
            Err(NodeRunnerError::DriverDetail(detail))
                if rpc::is_grammar_compilation_rate_limit(detail)
        );
    let outcome = match drained {
        Ok(()) => outcome,
        Err(error) => Err(error),
    };
    let completion = match require_process_cleanup(&completion) {
        Ok(completion) => completion,
        Err(_) => {
            return FinishedRpc {
                outcome: Err(rpc_cleanup_failure(&outcome, &connection.redactions, control).await),
                retryable_grammar_rate_limit: false,
            };
        }
    };
    let outcome = if control.is_cancelled() && outcome.is_ok() {
        Err(NodeRunnerError::Cancelled)
    } else {
        outcome.map_err(|error| connection.failure_diagnostic(error, completion))
    };
    FinishedRpc {
        outcome,
        retryable_grammar_rate_limit,
    }
}

// Cleanup owns settlement, while the earlier provider explanation remains diagnostic evidence.
async fn rpc_cleanup_failure(
    outcome: &Result<WorkerOutcome, NodeRunnerError>,
    redactions: &[String],
    control: &DriverControl,
) -> NodeRunnerError {
    if let Err(NodeRunnerError::DriverDetail(detail)) = outcome {
        let diagnostic = provider_failure_diagnostic("Copilot", Some(detail), None, redactions);
        report_cleanup_failure(control, diagnostic).await;
    }
    NodeRunnerError::CleanupUnconfirmed
}

async fn exchange_rpc(
    connection: &mut rpc::CopilotRpc<'_>,
    process: &ProviderProcess,
    turn: rpc::CopilotRpcTurn<'_>,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    let mut cancellation = connection.control.cancellation();
    let exchange = connection.run(sender, turn);
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

mod registration;
pub(crate) use registration::registration;
