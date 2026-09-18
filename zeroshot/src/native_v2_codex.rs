//! Native-v2 Codex harness for the OpenAI, OpenRouter, gateway, and Amazon Bedrock provider lanes.
//!
//! The graph-wide provider is fixed when the adapter is constructed. Model, effort, session
//! scope, input, and declared environment remain per-node admitted values. Provider sessions are
//! harness-owned runtime state and never enter the durable runner contract.

mod command;
mod output;
mod permissions;
mod process;
mod schema_file;
#[path = "native_v2_codex/session.rs"]
mod session;
mod turn;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openengine_cluster_protocol::WorkerOutcome;

use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::{HostedProcessPool, ProcessSessionCommand};
use crate::native_v2_capsule::provider_process::{
    ClosedSessionFailure, ProviderFailure, ProviderFailureRetry, ProviderProcessRunners,
    ProviderExecution, ProviderFilesystemConfig, CODEX_LOCAL_ENVIRONMENT, local_environment,
    agent_workspace_access, provider_redactions, with_driver_detail,
};
use crate::native_v2_contract::CodexProvider;
use crate::native_v2_runner::{
    AgentResponse, AgentResponseState, render_agent_prompt, resolve_agent_response_with_dialect,
    DriverControl, DriverInvocation, LiveOutput, LiveOutputStream, NodeRunnerError,
    ProviderSchemaDialect, ResolvedEnvironment,
};

use command::{
    add_node_args, add_provider_args, add_resume_command, add_session_target, agent_selection,
    configure_provider_auth, process_environment, path_text,
};
use output::CodexOutput;
use process::{ProcessOpen, exchange_turn, open_process};
use schema_file::CodexSchemaFile;
use session::CodexSession;
use turn::{CodexCommandInput, CodexTurnProcess, CodexTurnProcessOpen};

/// Runtime capabilities required to launch Codex. None of these paths are durable run data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeV2CodexConfig {
    pub provider: CodexProvider,
    pub executable: PathBuf,
    pub workspace: PathBuf,
    pub runtime_home: PathBuf,
    /// Current-user homes are available only to the built-in local target.
    pub local_user: Option<NativeV2CodexUser>,
    /// Explicit executable search path for Codex and commands launched by the agent.
    pub search_path: String,
    pub process_pool: HostedProcessPool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeV2CodexUser {
    pub home: PathBuf,
    pub codex_home: PathBuf,
}

/// One graph-wide Codex provider adapter.
pub struct NativeV2CodexAdapter {
    config: NativeV2CodexConfig,
    runners: ProviderProcessRunners,
    local_environment: BTreeMap<String, String>,
    #[cfg(test)]
    test_permission_policy: Option<crate::native_v2_capsule::provider_process::PermissionPolicy>,
}

impl NativeV2CodexAdapter {
    #[must_use]
    pub fn new(mut config: NativeV2CodexConfig) -> Self {
        config.local_user = None;
        let process_pool = config.process_pool;
        Self {
            config,
            runners: ProviderProcessRunners::hosted(process_pool),
            #[cfg(test)]
            test_permission_policy: None,
            local_environment: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn new_local(config: NativeV2CodexConfig) -> Self {
        let local_environment = local_environment(CODEX_LOCAL_ENVIRONMENT);
        Self {
            config,
            runners: ProviderProcessRunners::local(),
            #[cfg(test)]
            test_permission_policy: None,
            local_environment,
        }
    }

    #[cfg(all(test, unix))]
    fn new_for_test(config: NativeV2CodexConfig) -> Self {
        let mut adapter = Self::new_local(config);
        adapter.local_environment.clear();
        adapter.test_permission_policy =
            Some(crate::native_v2_capsule::provider_process::PermissionPolicy::Unset);
        adapter
    }

    fn command(
        &self,
        turn: &CodexTurn<'_>,
        input: CodexCommandInput<'_>,
    ) -> Result<ProcessSessionCommand, NodeRunnerError> {
        let invocation = turn.invocation;
        let (model, effort) = agent_selection(&invocation.node.binding).map_err(|error| {
            with_driver_detail(error, "Codex command requires an agent runtime binding")
        })?;
        let access = agent_workspace_access(invocation.role).map_err(|error| {
            with_driver_detail(error, "Codex workspace policy rejected the node role")
        })?;
        let executable = path_text(&self.config.executable).map_err(|error| {
            with_driver_detail(error, "Codex executable path is not valid on this platform")
        })?;
        let workspace = input.files.workspace.clone();
        let mut environment = self
            .provider_environment(&invocation.environment, input.files.home())
            .map_err(|error| with_driver_detail(error, "Codex provider environment is invalid"))?;
        let mut argv = vec!["exec".to_owned()];
        add_resume_command(&mut argv, input.resume);
        // Local OpenAI-compatible routing belongs to the user's Codex configuration.
        if !(self.config.local_user.is_some() && self.config.provider == CodexProvider::OpenAi) {
            add_provider_args(&mut argv, self.config.provider, &environment)?;
        }
        add_node_args(&mut argv, model.as_str(), effort.copied());
        argv.extend([
            "--output-schema".to_owned(),
            path_text(input.schema_path).map_err(|error| {
                with_driver_detail(
                    error,
                    "Codex response schema path is not valid on this platform",
                )
            })?,
        ]);
        add_session_target(&mut argv, input.resume);

        environment.entry("TMPDIR".to_owned()).or_insert(
            input
                .files
                .scratch_text()
                .map_err(|error| NodeRunnerError::DriverDetail(error.to_string()))?,
        );
        Ok(ProcessSessionCommand {
            program: executable,
            argv,
            environment,
            workspace: WorkspaceCapability {
                current_dir: workspace,
                mode: access,
            },
            deadline: None,
        })
    }

    fn provider_environment(
        &self,
        environment: &ResolvedEnvironment,
        runtime_home: &Path,
    ) -> Result<BTreeMap<String, String>, NodeRunnerError> {
        let (home, codex_home) = match &self.config.local_user {
            Some(local) => (
                path_text(&local.home).map_err(|error| {
                    with_driver_detail(error, "Codex user home is not a valid platform path")
                })?,
                path_text(&local.codex_home).map_err(|error| {
                    with_driver_detail(
                        error,
                        "Codex configuration home is not a valid platform path",
                    )
                })?,
            ),
            None => {
                let runtime_home = path_text(runtime_home).map_err(|error| {
                    with_driver_detail(error, "Codex runtime home is not a valid platform path")
                })?;
                (runtime_home.clone(), runtime_home)
            }
        };
        let mut values = process_environment(
            environment,
            home,
            codex_home,
            self.config.search_path.clone(),
        )
        .map_err(|error| {
            with_driver_detail(
                error,
                "Codex declared environment conflicts with reserved runtime configuration",
            )
        })?;
        for (name, value) in &self.local_environment {
            values.entry(name.clone()).or_insert_with(|| value.clone());
        }
        configure_provider_auth(
            &mut values,
            self.config.provider,
            self.config.local_user.is_some(),
        )
        .map_err(|error| {
            with_driver_detail(
                error,
                "Codex provider credentials are missing or conflict with reserved credentials",
            )
        })?;
        Ok(values)
    }

    async fn run_turn(
        &self,
        invocation: &DriverInvocation,
        session: &CodexSession,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let _turn = session.core.turn.lock().await;
        let execution = ProviderExecution::new(
            ProviderFilesystemConfig {
                runners: self.runners,
                root: &self.config.runtime_home,
                workspace: &self.config.workspace,
            },
            invocation,
            &session.core,
        );
        let turn = CodexTurn {
            invocation,
            session,
            control: &control,
            execution: &execution,
        };
        let prompt = render_agent_prompt(
            invocation.agent_instructions()?,
            &invocation.node.input,
            &invocation.response,
        )
        .map_err(|error| with_driver_detail(error, "Codex prompt could not be serialized"))?;
        let mut state = CodexRunState::new(
            prompt,
            provider_redactions(&invocation.environment, &self.local_environment),
        );
        loop {
            if let Some(outcome) = self.advance_run(&turn, &mut state).await? {
                return Ok(outcome);
            }
        }
    }

    async fn advance_run(
        &self,
        turn: &CodexTurn<'_>,
        state: &mut CodexRunState,
    ) -> Result<Option<WorkerOutcome>, NodeRunnerError> {
        match self.advance_turn(turn, state.response.prompt()).await {
            Ok(CodexTurnAdvance::Response(response)) => state.accept_response(turn, response).await,
            Ok(CodexTurnAdvance::ProviderFailure(detail)) => {
                state.retry_provider_failure(turn, Some(&detail)).await?;
                Ok(None)
            }
            Err(NodeRunnerError::Driver) => {
                state.retry_provider_failure(turn, None).await?;
                Ok(None)
            }
            Err(NodeRunnerError::DriverDetail(detail)) => {
                state.retry_provider_failure(turn, Some(&detail)).await?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    async fn advance_turn(
        &self,
        turn: &CodexTurn<'_>,
        prompt: &str,
    ) -> Result<CodexTurnAdvance, NodeRunnerError> {
        turn.session
            .core
            .ensure_live(ClosedSessionFailure::Driver)?;
        let resume = turn.session.thread_id.lock().await.clone();
        let output = self.execute_turn(turn, resume.as_deref(), prompt).await?;
        if let Err(detail) = turn
            .session
            .record_attempt_thread(&output, resume.as_deref())
            .await
        {
            return Ok(CodexTurnAdvance::ProviderFailure(detail.to_owned()));
        }
        resolve_codex_output(turn, output).await
    }

    async fn execute_turn(
        &self,
        turn: &CodexTurn<'_>,
        resume: Option<&str>,
        prompt: &str,
    ) -> Result<CodexOutput, NodeRunnerError> {
        let mut turn_process = match self.open_turn_process(turn, resume).await? {
            CodexTurnProcessOpen::Ready(process) => process,
            CodexTurnProcessOpen::ProviderFailure(detail) => {
                return Ok(CodexOutput::provider_failure(detail));
            }
        };
        let redactions = provider_redactions(&turn.invocation.environment, &self.local_environment);
        exchange_turn(&mut turn_process.process, prompt, turn.control, &redactions).await
    }

    async fn open_turn_process(
        &self,
        turn: &CodexTurn<'_>,
        resume: Option<&str>,
    ) -> Result<CodexTurnProcessOpen, NodeRunnerError> {
        let files = match turn.execution.prepare(turn.control).await {
            Ok(resources) => resources,
            Err(error) => {
                return Ok(CodexTurnProcessOpen::ProviderFailure(format!(
                    "provider process setup failed: {error}"
                )));
            }
        };
        let schema = match CodexSchemaFile::create(
            files.home(),
            &turn
                .invocation
                .response
                .provider_schema(ProviderSchemaDialect::OpenAiStrict),
        ) {
            Ok(schema) => schema,
            Err(error) => {
                return Ok(CodexTurnProcessOpen::ProviderFailure(error.to_string()));
            }
        };
        let mut command = self.command(
            turn,
            CodexCommandInput {
                resume,
                files: &files,
                schema_path: schema.path(),
            },
        )?;
        self.apply_permission_default(files.clone(), &mut command, turn.control)
            .await?;
        let process = match open_process(files, command, turn.control).await? {
            ProcessOpen::Ready(process) => process,
            ProcessOpen::ProviderFailure(detail) => {
                return Ok(CodexTurnProcessOpen::ProviderFailure(detail));
            }
        };
        Ok(CodexTurnProcessOpen::Ready(CodexTurnProcess {
            process,
            _schema: schema,
        }))
    }
}

struct CodexTurn<'a> {
    invocation: &'a DriverInvocation,
    session: &'a CodexSession,
    control: &'a DriverControl,
    execution: &'a ProviderExecution<'a>,
}

struct CodexRunState {
    response: AgentResponseState,
    retry: ProviderFailureRetry,
}

impl CodexRunState {
    fn new(prompt: String, redactions: Vec<String>) -> Self {
        Self {
            retry: ProviderFailureRetry::new("Codex", prompt.clone(), redactions),
            response: AgentResponseState::new(prompt),
        }
    }

    async fn accept_response(
        &mut self,
        turn: &CodexTurn<'_>,
        response: AgentResponse,
    ) -> Result<Option<WorkerOutcome>, NodeRunnerError> {
        self.response.accept("Codex", turn.control, response).await
    }

    async fn retry_provider_failure(
        &mut self,
        turn: &CodexTurn<'_>,
        detail: Option<&str>,
    ) -> Result<(), NodeRunnerError> {
        let has_session = turn.session.thread_id.lock().await.is_some();
        let prompt = self
            .retry
            .after_failure(
                turn.control,
                ProviderFailure {
                    detail,
                    retryable: true,
                    has_session,
                },
            )
            .await?;
        self.response.replace_prompt(prompt);
        Ok(())
    }
}

enum CodexTurnAdvance {
    Response(AgentResponse),
    ProviderFailure(String),
}

async fn resolve_codex_output(
    turn: &CodexTurn<'_>,
    output: CodexOutput,
) -> Result<CodexTurnAdvance, NodeRunnerError> {
    if let Some(failure) = output.failure_message() {
        return Ok(CodexTurnAdvance::ProviderFailure(failure.to_owned()));
    }
    let response = resolve_agent_response_with_dialect(
        &turn.invocation.response,
        output.final_message()?,
        ProviderSchemaDialect::OpenAiStrict,
    )?;
    if let Some(diagnostic) = turn
        .session
        .missing_required_thread(turn.invocation, &response)
        .await
    {
        return Ok(CodexTurnAdvance::ProviderFailure(diagnostic.to_owned()));
    }
    if matches!(response, AgentResponse::Correction(_)) {
        turn.control
            .emit(LiveOutput::new(
                LiveOutputStream::System,
                "Codex final output rejected; requesting correction",
            )?)
            .await?;
    }
    Ok(CodexTurnAdvance::Response(response))
}

#[cfg(test)]
mod tests;
