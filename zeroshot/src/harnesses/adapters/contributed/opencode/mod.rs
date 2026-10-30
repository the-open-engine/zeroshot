//! OpenCode integration contributed by the community.
//!
//! Uses the native `run --format json` interface: prompts travel over stdin and corrections
//! continue the exact reported session. OpenCode owns provider/model resolution. Native tool
//! results carry Active Memory through a private plugin rather than a separate memory engine.
//! Protocol references: [CLI](https://opencode.ai/docs/cli/) and
//! [plugins](https://opencode.ai/docs/plugins/).

mod auth;
mod command;
mod login;
mod memory;
mod permissions;
mod plugin;
mod registration;
mod transcript;

pub(crate) use registration::registration;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::WorkerOutcome;
use tokio::sync::Mutex;

use crate::execution::process::HostedProcessPool;
use crate::native_v2_capsule::provider_process::{
    ClosedSessionFailure, LocalHarnessEnvironment, ProviderAttempt, ProviderExecution,
    ProviderExecutionFiles, ProviderFailureRetry, ProviderFilesystemConfig, ProviderProcessRunners,
    ProviderResult, ProviderSessionCore, impl_provider_node_session, provider_redactions,
    redact_provider_error,
};
use crate::native_v2_capsule::provider_process::turn::{self, ProviderProcessStart};
use crate::native_v2_runner::{
    AgentResponseState, DriverControl, DriverInvocation, NodeDriver, NodeRunnerError,
    ProviderSchemaDialect, ResolvedEnvironment, resolve_agent_response_with_schema,
};

pub use crate::native_v2_capsule::HarnessProcessEnvironment as OpenCodeProcessEnvironment;

/// Host-owned paths and process capabilities. Credential values remain per-invocation inputs.
pub struct OpenCodeConfig {
    pub provider: String,
    pub executable: String,
    pub prefix_arguments: Vec<String>,
    pub workspace: PathBuf,
    pub runtime_home: PathBuf,
    pub local_user_home: Option<PathBuf>,
    pub native_environment: LocalHarnessEnvironment,
    pub base_environment: OpenCodeProcessEnvironment,
    pub process_pool: HostedProcessPool,
}

pub struct OpenCodeAdapter {
    config: OpenCodeConfig,
    runners: ProviderProcessRunners,
    local_environment: BTreeMap<String, String>,
    active_memory: Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
}

crate::active_memory::hooks::impl_active_memory_adapter!(OpenCodeAdapter);

impl OpenCodeAdapter {
    pub fn new(mut config: OpenCodeConfig) -> Result<Self, NodeRunnerError> {
        config.local_user_home = None;
        config.native_environment = LocalHarnessEnvironment::default();
        let runners = ProviderProcessRunners::hosted(config.process_pool);
        Self::configured(config, runners)
    }

    pub fn new_local(config: OpenCodeConfig) -> Result<Self, NodeRunnerError> {
        let mut local_environment = config
            .native_environment
            .selected(command::LOCAL_ENVIRONMENT);
        if config.provider == "bedrock" {
            local_environment.extend(config.native_environment.selected(command::AWS_ENVIRONMENT));
        }
        let mut adapter = Self::configured(config, ProviderProcessRunners::local())?;
        adapter.local_environment = local_environment;
        Ok(adapter)
    }

    fn configured(
        config: OpenCodeConfig,
        runners: ProviderProcessRunners,
    ) -> Result<Self, NodeRunnerError> {
        if config.executable.is_empty() || command::credential_field(&config.provider).is_none() {
            return Err(NodeRunnerError::DriverDetail(
                "OpenCode configuration is invalid".to_owned(),
            ));
        }
        Ok(Self {
            config,
            runners,
            local_environment: BTreeMap::new(),
            active_memory: None,
        })
    }

    async fn run_session(
        &self,
        invocation: &DriverInvocation,
        session: &OpenCodeSession,
        control: &DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let _turn = session.core.turn.lock().await;
        session
            .core
            .ensure_live(ClosedSessionFailure::SessionLost)?;
        let execution = ProviderExecution::new(
            ProviderFilesystemConfig {
                runners: self.runners,
                root: &self.config.runtime_home,
                workspace: &self.config.workspace,
            },
            invocation,
            &session.core,
        );
        let files = execution.prepare(control).await.map_err(|error| {
            if control.is_cancelled() {
                NodeRunnerError::Cancelled
            } else {
                NodeRunnerError::DriverDetail(format!(
                    "OpenCode execution could not be prepared: {error}"
                ))
            }
        })?;
        let dialect = if self.config.provider == "anthropic" {
            ProviderSchemaDialect::Standard
        } else {
            ProviderSchemaDialect::OpenAiStrict
        };
        let turn = OpenCodeTurn {
            invocation,
            session,
            control,
            files,
            dialect,
        };
        self.advance_session(turn).await
    }

    async fn advance_session(
        &self,
        turn: OpenCodeTurn<'_>,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let invocation = turn.invocation;
        let initial = invocation.agent_prompt_with_schema(turn.dialect)?;
        let mut feedback = ProviderFailureRetry::new(
            "OpenCode",
            initial.clone(),
            self.redactions(&invocation.environment),
        );
        let mut response = AgentResponseState::new(initial);
        loop {
            let (result, redactions) = self.execute_turn(&turn, response.prompt()).await?;
            feedback.extend_redactions(&redactions);
            let answer = resolve_agent_response_with_schema(
                &invocation.response,
                &result.message,
                turn.dialect,
            )?;
            feedback
                .report_correction(turn.control, &answer, response.corrections_exhausted())
                .await?;
            if let Some(outcome) = response.accept("OpenCode", turn.control, answer).await? {
                return Ok(outcome);
            }
        }
    }

    async fn execute_turn(
        &self,
        turn: &OpenCodeTurn<'_>,
        prompt: &str,
    ) -> Result<(ProviderResult, Vec<String>), NodeRunnerError> {
        turn.session
            .core
            .ensure_live(ClosedSessionFailure::SessionLost)?;
        let expected = turn.session.id.lock().await.clone();
        let mut command = command::command(command::CommandRequest {
            adapter: self,
            invocation: turn.invocation,
            files: &turn.files,
            session_id: expected.as_deref(),
        })?;
        let mut redactions = self.redactions(&turn.invocation.environment);
        self.apply_permission_default(&mut command);
        redactions.extend(command::configuration_secrets(&command.environment));
        let transcript = transcript::OpenCodeTranscript::new(expected.clone(), redactions.clone());
        let attempt = match turn::open(turn.files.clone(), command, turn.control).await? {
            ProviderProcessStart::Ready(mut process) => {
                turn::finish_process(&mut process, prompt.as_bytes(), transcript, turn.control)
                    .await?
            }
            ProviderProcessStart::Failed(attempt) => attempt,
        };
        let result = completed_attempt(attempt)?;
        let reported = result.session_id.as_ref().ok_or_else(|| {
            NodeRunnerError::DriverDetail("OpenCode did not report its session identity".to_owned())
        })?;
        *turn.session.id.lock().await = Some(reported.clone());
        Ok((result, redactions))
    }

    fn redactions(&self, environment: &ResolvedEnvironment) -> Vec<String> {
        let mut values = provider_redactions(environment, &self.local_environment);
        values.extend(command::configuration_secrets(&self.local_environment));
        values.extend(
            command::AWS_SECRETS
                .iter()
                .filter_map(|name| self.local_environment.get(*name))
                .filter(|value| !value.is_empty())
                .cloned(),
        );
        values
    }
}

fn completed_attempt(attempt: ProviderAttempt) -> Result<ProviderResult, NodeRunnerError> {
    match attempt {
        ProviderAttempt::Failed(failure) => Err(NodeRunnerError::DriverDetail(failure.diagnostic)),
        ProviderAttempt::Complete(result) => Ok(result),
    }
}

struct OpenCodeTurn<'a> {
    invocation: &'a DriverInvocation,
    session: &'a OpenCodeSession,
    control: &'a DriverControl,
    files: Arc<ProviderExecutionFiles>,
    dialect: ProviderSchemaDialect,
}

struct OpenCodeSession {
    core: ProviderSessionCore,
    id: Mutex<Option<String>>,
}

impl_provider_node_session!(OpenCodeSession);

crate::native_v2_capsule::provider_process::impl_provider_session_factory!(
    OpenCodeAdapter,
    OpenCodeSession {
        core: ProviderSessionCore::new(),
        id: Mutex::new(None)
    }
);

#[async_trait]
impl NodeDriver for OpenCodeAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let invocation = self.memory_invocation(invocation);
        let session = crate::native_v2_capsule::provider_process::invocation_session::<
            OpenCodeSession,
        >(&invocation)?;
        let redactions = self.redactions(&invocation.environment);
        self.run_session(&invocation, session, &control)
            .await
            .map_err(|error| redact_provider_error(error, &redactions))
    }
}

#[cfg(test)]
mod tests;
