//! Claude CLI adapter for native-v2 graph nodes.
//!
//! One adapter serves one Anthropic, OpenRouter, gateway, or Amazon Bedrock lane.
//! Admission has already selected the model, effort, session scope, and declared environment for
//! each node. Only the local adapter inherits the current user's harness configuration environment.

#[path = "native_v2_claude/command.rs"]
mod command;
#[path = "native_v2_claude/permissions.rs"]
mod permissions;
#[path = "native_v2_claude/session.rs"]
mod session;
#[path = "native_v2_claude/transcript.rs"]
mod transcript;
use crate::native_v2_capsule::provider_process::turn as turn_process;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::execution::process::{HostedProcessPool, ProcessSessionCommand};
use crate::native_v2_capsule::provider_process::{
    ClosedSessionFailure, ProviderExecution, ProviderExecutionFiles, ProviderProcessRunners,
    CLAUDE_LOCAL_ENVIRONMENT, LocalHarnessEnvironment, agent_workspace_access, provider_redactions,
    with_driver_detail, merge_local_environment, extend_declared_environment,
};
use crate::native_v2_contract::{ClaudeProvider, NodeRuntimeBinding};
use crate::native_v2_runner::{
    AgentResponse, resolve_agent_response, DriverControl, DriverInvocation, LiveOutput,
    LiveOutputStream, NodeRunnerError, ProviderSchemaDialect, ResolvedEnvironment,
};
use command::{ClaudeTurnArguments, claude_arguments, configure_provider, prompt};
use session::{ClaudeRunState, ClaudeSession, attempt_session_id, observe_session};
use transcript::{ClaudeAttempt, ClaudeTranscript};
use turn_process::ProviderProcessStart as ClaudeProcessStart;

pub use crate::native_v2_capsule::HarnessProcessEnvironment as ClaudeProcessEnvironment;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ClaudeAdapterConfigError {
    #[error("Claude executable must not be empty")]
    EmptyExecutable,
}

/// Claude adapter for one provider lane.
pub struct ClaudeAdapter {
    provider: ClaudeProvider,
    executable: String,
    prefix_arguments: Vec<String>,
    workspace: PathBuf,
    runtime_home: PathBuf,
    local_user_home: Option<PathBuf>,
    base_environment: ClaudeProcessEnvironment,
    runners: ProviderProcessRunners,
    local_environment: BTreeMap<String, String>,
    active_memory: Option<std::sync::Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
    #[cfg(test)]
    test_permission_policy: Option<crate::native_v2_capsule::provider_process::PermissionPolicy>,
}

pub struct ClaudeAdapterConfig {
    pub provider: ClaudeProvider,
    pub executable: String,
    pub prefix_arguments: Vec<String>,
    pub workspace: PathBuf,
    pub runtime_home: PathBuf,
    /// Current-user home is available only to the built-in local target.
    pub local_user_home: Option<PathBuf>,
    /// Invoking-shell snapshot available only to the built-in local target.
    pub native_environment: LocalHarnessEnvironment,
    pub base_environment: ClaudeProcessEnvironment,
    pub process_pool: HostedProcessPool,
}

crate::active_memory::hooks::impl_active_memory_adapter!(ClaudeAdapter);

impl ClaudeAdapter {
    pub fn new(mut configuration: ClaudeAdapterConfig) -> Result<Self, ClaudeAdapterConfigError> {
        configuration.local_user_home = None;
        configuration.native_environment = LocalHarnessEnvironment::default();
        let runners = ProviderProcessRunners::hosted(configuration.process_pool);
        Self::configured(configuration, runners)
    }

    pub fn new_local(configuration: ClaudeAdapterConfig) -> Result<Self, ClaudeAdapterConfigError> {
        let inherit_selectors = configuration.provider == ClaudeProvider::Anthropic;
        let local_environment = configuration
            .native_environment
            .selected(CLAUDE_LOCAL_ENVIRONMENT);
        let mut adapter = Self::configured(configuration, ProviderProcessRunners::local())?;
        adapter.local_environment = local_environment;
        if !inherit_selectors {
            adapter.local_environment.retain(|name, _| {
                !matches!(
                    name.as_str(),
                    "ANTHROPIC_API_KEY"
                        | "ANTHROPIC_AUTH_TOKEN"
                        | "CLAUDE_CODE_OAUTH_TOKEN"
                        | "CLAUDE_CODE_OAUTH_REFRESH_TOKEN"
                )
            });
        }
        if !inherit_selectors {
            adapter
                .local_environment
                .retain(|name, _| !name.starts_with("CLAUDE_CODE_USE_"));
        }
        Ok(adapter)
    }

    fn configured(
        configuration: ClaudeAdapterConfig,
        runners: ProviderProcessRunners,
    ) -> Result<Self, ClaudeAdapterConfigError> {
        if configuration.executable.is_empty() {
            return Err(ClaudeAdapterConfigError::EmptyExecutable);
        }
        Ok(Self {
            provider: configuration.provider,
            executable: configuration.executable,
            prefix_arguments: configuration.prefix_arguments,
            workspace: configuration.workspace,
            runtime_home: configuration.runtime_home,
            local_user_home: configuration.local_user_home,
            base_environment: configuration.base_environment,
            runners,
            local_environment: BTreeMap::new(),
            active_memory: None,
            #[cfg(test)]
            test_permission_policy: None,
        })
    }

    #[cfg(all(test, unix))]
    fn new_for_test(configuration: ClaudeAdapterConfig) -> Result<Self, ClaudeAdapterConfigError> {
        let mut adapter = Self::new_local(configuration)?;
        adapter.local_environment.clear();
        adapter.test_permission_policy =
            Some(crate::native_v2_capsule::provider_process::PermissionPolicy::Unset);
        Ok(adapter)
    }

    fn command(
        &self,
        turn: &ClaudeTurn<'_>,
        input: ClaudeCommandInput<'_>,
    ) -> Result<ProcessSessionCommand, NodeRunnerError> {
        let invocation = turn.invocation;
        let NodeRuntimeBinding::Agent { model, effort, .. } = &invocation.node.binding else {
            return Err(NodeRunnerError::DriverDetail(
                "Claude command requires an agent runtime binding".to_owned(),
            ));
        };
        let access = agent_workspace_access(invocation.role).map_err(|error| {
            with_driver_detail(error, "Claude workspace policy rejected the node role")
        })?;
        let argv = claude_arguments(
            self.prefix_arguments.clone(),
            ClaudeTurnArguments {
                model: model.as_str(),
                effort: *effort,
                resume_id: input.resume_id,
                json_schema: serde_json::to_string(
                    &invocation
                        .response
                        .provider_schema(ProviderSchemaDialect::Standard),
                )
                .map_err(|_| {
                    NodeRunnerError::DriverDetail(
                        "Claude response schema could not be serialized".to_owned(),
                    )
                })?,
            },
        );

        let mut environment = self
            .process_environment(&invocation.environment, input.files.home())
            .map_err(|error| with_driver_detail(error, "Claude provider environment is invalid"))?;
        environment.insert(
            "TMPDIR".to_owned(),
            input
                .files
                .scratch_text()
                .map_err(|error| NodeRunnerError::DriverDetail(error.to_string()))?,
        );
        let mut command = ProcessSessionCommand {
            program: self.executable.clone(),
            argv,
            environment,
            workspace: crate::execution::driver::WorkspaceCapability {
                current_dir: input.files.workspace.clone(),
                mode: access,
            },
            deadline: None,
        };
        if let Some(memory) = &self.active_memory {
            memory.configure(
                crate::active_memory::hooks::Harness::Claude,
                &mut command,
                crate::active_memory::hooks::HookInstallation {
                    invocation,
                    home: input.files.home(),
                },
            );
        }
        Ok(command)
    }

    fn process_environment(
        &self,
        resolved: &ResolvedEnvironment,
        runtime_home: &Path,
    ) -> Result<BTreeMap<String, String>, NodeRunnerError> {
        let mut environment = self.base_environment.clone_values();
        let runtime_home = runtime_home
            .to_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                NodeRunnerError::DriverDetail(
                    "Claude runtime home is not a valid non-empty platform path".to_owned(),
                )
            })?;
        let provider_home = self
            .local_user_home
            .as_deref()
            .and_then(Path::to_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(runtime_home);
        environment.insert("HOME".to_owned(), provider_home.to_owned());
        // Hosted targets may expose a read-only or root-owned shared `/tmp`. Keep Claude's
        // sockets and temporary files inside the same provider-private session home instead.
        environment.insert("TMPDIR".to_owned(), runtime_home.to_owned());
        extend_declared_environment(&mut environment, resolved).map_err(|error| {
            with_driver_detail(
                error,
                "Claude declared environment conflicts with reserved process configuration",
            )
        })?;
        merge_local_environment(
            &mut environment,
            resolved,
            &self.local_environment,
            &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
            ],
        );
        configure_provider(&mut environment, self.provider).map_err(|error| {
            with_driver_detail(
                error,
                "Claude provider credentials are missing or conflict with other credentials",
            )
        })?;
        Ok(environment)
    }
}

impl ClaudeAdapter {
    async fn advance_turn(
        &self,
        turn: &ClaudeTurn<'_>,
        state: &mut ClaudeRunState,
    ) -> Result<ClaudeTurnAdvance, NodeRunnerError> {
        turn.session
            .core
            .ensure_live(ClosedSessionFailure::SessionLost)?;
        let attempt = self
            .execute_turn(
                turn,
                state.resume_id.as_deref(),
                state.response.prompt().to_owned(),
            )
            .await?;
        if let Err(diagnostic) = observe_session(&mut state.resume_id, attempt_session_id(&attempt))
        {
            return Ok(ClaudeTurnAdvance::ProviderFailure {
                retryable: false,
                diagnostic: diagnostic.to_owned(),
            });
        }
        resolve_claude_attempt(turn, state, attempt).await
    }

    async fn execute_turn(
        &self,
        turn: &ClaudeTurn<'_>,
        resume_id: Option<&str>,
        prompt: String,
    ) -> Result<ClaudeAttempt, NodeRunnerError> {
        let mut process = match self.open_turn_process(turn, resume_id).await? {
            ClaudeProcessStart::Ready(process) => process,
            ClaudeProcessStart::Failed(attempt) => return Ok(attempt),
        };
        let transcript = ClaudeTranscript::new(provider_redactions(
            &turn.invocation.environment,
            &self.local_environment,
        ));
        turn_process::finish_process(&mut process, prompt.as_bytes(), transcript, turn.control)
            .await
    }

    async fn open_turn_process(
        &self,
        turn: &ClaudeTurn<'_>,
        resume_id: Option<&str>,
    ) -> Result<ClaudeProcessStart, NodeRunnerError> {
        let files = match turn.execution.prepare(turn.control).await {
            Ok(process) => process,
            Err(error) => return turn_process::failed_before_start(error, turn.control),
        };
        let mut command = self.command(
            turn,
            ClaudeCommandInput {
                resume_id,
                files: &files,
            },
        )?;
        self.apply_permission_default(files.clone(), &mut command, turn.control)
            .await?;
        turn_process::open(files, command, turn.control).await
    }
}

async fn resolve_claude_attempt(
    turn: &ClaudeTurn<'_>,
    state: &ClaudeRunState,
    attempt: ClaudeAttempt,
) -> Result<ClaudeTurnAdvance, NodeRunnerError> {
    let result = match attempt {
        ClaudeAttempt::Complete(result) => result,
        ClaudeAttempt::Failed(failure) => {
            return Ok(ClaudeTurnAdvance::ProviderFailure {
                retryable: failure.retryable,
                diagnostic: failure.diagnostic,
            });
        }
    };
    let response = resolve_agent_response(&turn.invocation.response, &result.message)?;
    state
        .retry
        .report_correction(
            turn.control,
            &response,
            state.response.corrections_exhausted(),
        )
        .await?;
    if matches!(response, AgentResponse::Correction { .. }) {
        if state.resume_id.is_none() {
            return Ok(ClaudeTurnAdvance::ProviderFailure {
                retryable: false,
                diagnostic:
                    "Claude output did not provide a session identifier required for correction"
                        .to_owned(),
            });
        }
        turn.control
            .emit(LiveOutput::new(
                LiveOutputStream::System,
                "Claude final output rejected; requesting correction",
            )?)
            .await?;
    }
    Ok(ClaudeTurnAdvance::Response(response))
}

struct ClaudeTurn<'a> {
    invocation: &'a DriverInvocation,
    session: &'a ClaudeSession,
    control: &'a DriverControl,
    execution: &'a ProviderExecution<'a>,
}

enum ClaudeTurnAdvance {
    Response(AgentResponse),
    ProviderFailure { retryable: bool, diagnostic: String },
}

struct ClaudeCommandInput<'a> {
    resume_id: Option<&'a str>,
    files: &'a ProviderExecutionFiles,
}

#[cfg(test)]
#[path = "native_v2_claude/tests.rs"]
mod tests;
