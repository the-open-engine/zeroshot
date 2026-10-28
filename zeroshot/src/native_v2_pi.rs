//! Pi CLI adapter for native-v2 graph nodes.
//!
//! One adapter serves the effective Pi provider lane. Admission has already selected the model,
//! effort, session scope, and declared environment for each node. Pi exposes no permission
//! control of its own, so the adapter appends no permission argument and a contained run is bounded
//! by the disposable capsule; the shared prompt renderer carries the rest.
//!
//! The adapter drives `pi --mode json`: one process per turn, the prompt on stdin, and an LF-framed
//! JSONL event stream on stdout. Nothing here appends a permission argument, because Pi has no
//! bypass flag to append.

#[path = "native_v2_pi/command.rs"]
mod command;
#[path = "native_v2_pi/provider_document.rs"]
mod provider_document;
#[path = "native_v2_pi/session.rs"]
mod session;
#[path = "native_v2_pi/session_id.rs"]
mod session_id;
#[path = "native_v2_pi/transcript.rs"]
mod transcript;
use crate::native_v2_capsule::provider_process::turn as turn_process;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::execution::process::{HostedProcessPool, ProcessSessionCommand};
use crate::native_v2_capsule::provider_process::{
    ClosedSessionFailure, LocalHarnessEnvironment, PI_LOCAL_ENVIRONMENT, ProviderExecution,
    ProviderExecutionFiles, ProviderProcessRunners, provider_redactions, with_driver_detail,
};
use crate::native_v2_contract::{NodeRuntimeBinding, PiProvider};
use crate::native_v2_runner::{
    AgentResponse, DriverControl, DriverInvocation, LiveOutput, LiveOutputStream, NodeRunnerError,
    ProviderSchemaDialect, ResolvedEnvironment, resolve_agent_response_with_schema,
};
use command::{PiCommandRequest, command};
use provider_document::{
    OPENAI, declared_api, declared_base_url, gateway_base_url, write_endpoint, write_gateway,
};
use session::{PiRunState, PiSession, PiTurnAdvance};
use session_id::{agent_directory, observe_session, session_directory};
use transcript::{PiAttempt, PiResult, PiTranscript};
use turn_process::ProviderProcessStart as PiProcessStart;

/// The provider schema dialect that matches the wire protocol a lane actually speaks.
///
/// Pi has no response-schema flag, so the schema travels in the prompt. The dialect still has to
/// describe optional fields the way the resolver accepts them, so it follows the lane's protocol:
/// the built-in `openai` lane uses Responses and the `openrouter` lane uses OpenAI-compatible chat
/// completions, while the `anthropic` and `amazon-bedrock` lanes have no OpenAI strict mode and so
/// use the neutral schema. The gateway lane's protocol is caller-owned and named by `GATEWAY_API`,
/// never inferred from the opaque model identifier.
fn schema_dialect(
    provider: PiProvider,
    resolved: &ResolvedEnvironment,
) -> Result<ProviderSchemaDialect, NodeRunnerError> {
    Ok(match provider {
        PiProvider::Anthropic | PiProvider::Bedrock => ProviderSchemaDialect::Standard,
        PiProvider::OpenAi | PiProvider::OpenRouter => ProviderSchemaDialect::OpenAiStrict,
        PiProvider::Gateway => match declared_api(resolved, command::GATEWAY_API)?.as_str() {
            "anthropic-messages" => ProviderSchemaDialect::Standard,
            _ => ProviderSchemaDialect::OpenAiStrict,
        },
    })
}

/// Renders the shared provider-neutral node turn with the machine-readable response schema.
///
/// Pi has no response-schema flag, so the schema travels in the prompt. The contract object is
/// deliberately not shown: its `kind` discriminator is runtime bookkeeping that Pi otherwise copies
/// into its final response.
fn prompt(
    dialect: ProviderSchemaDialect,
    invocation: &DriverInvocation,
) -> Result<String, NodeRunnerError> {
    invocation
        .agent_prompt_with_schema(dialect)
        .map_err(|error| with_driver_detail(error, "Pi prompt could not be serialized"))
}

pub use crate::native_v2_capsule::HarnessProcessEnvironment as PiProcessEnvironment;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PiAdapterConfigError {
    #[error("Pi executable must not be empty")]
    EmptyExecutable,
}

/// Runtime capabilities required to launch Pi. None of these paths are durable run data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PiConfig {
    pub provider: PiProvider,
    pub executable: String,
    pub prefix_arguments: Vec<String>,
    pub workspace: PathBuf,
    pub runtime_home: PathBuf,
    /// Current-user home is available only to the built-in local target.
    pub local_user_home: Option<PathBuf>,
    /// Invoking-shell snapshot available only to the built-in local target.
    pub native_environment: LocalHarnessEnvironment,
    pub base_environment: PiProcessEnvironment,
    pub process_pool: HostedProcessPool,
}

/// One effective Pi provider lane.
pub struct PiAdapter {
    provider: PiProvider,
    executable: String,
    prefix_arguments: Vec<String>,
    workspace: PathBuf,
    runtime_home: PathBuf,
    /// Current-user home for the built-in local target; `None` for hosted placements.
    local_user_home: Option<PathBuf>,
    base_environment: PiProcessEnvironment,
    runners: ProviderProcessRunners,
    local_environment: BTreeMap<String, String>,
    active_memory: Option<std::sync::Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
}

crate::active_memory::hooks::impl_active_memory_adapter!(PiAdapter);

impl PiAdapter {
    /// Hosted adapter. It never inherits ambient configuration or a stored login.
    pub fn new(mut configuration: PiConfig) -> Result<Self, PiAdapterConfigError> {
        configuration.local_user_home = None;
        configuration.native_environment = LocalHarnessEnvironment::default();
        let runners = ProviderProcessRunners::hosted(configuration.process_pool);
        Self::configured(configuration, runners)
    }

    /// Local adapter. A native-local lane reuses the user's stored Pi login; every other lane
    /// receives a private agent directory and drops the ambient credentials of its lane.
    pub fn new_local(configuration: PiConfig) -> Result<Self, PiAdapterConfigError> {
        let provider = configuration.provider;
        let local_environment = configuration
            .native_environment
            .selected(PI_LOCAL_ENVIRONMENT);
        let mut adapter = Self::configured(configuration, ProviderProcessRunners::local())?;
        adapter.local_environment = local_environment;
        // A Bedrock region is foreign to every lane but Bedrock. Forwarding it would make the
        // credential boundary reject an otherwise valid non-bedrock turn, while the bedrock lane
        // still reads its own ambient region. Everything else is a lane credential, and a foreign
        // ambient credential that reached the child would be rejected by the credential check.
        let foreign = |name: &str| {
            is_pi_credential(name)
                || (provider != PiProvider::Bedrock && name == command::AWS_REGION)
        };
        if adapter.native_local() {
            // Keep the selected lane's own credential so a user who exports a key instead of
            // storing one still authenticates, but drop every other lane's.
            let own = command::native_local_credentials(provider);
            adapter
                .local_environment
                .retain(|name, _| !foreign(name) || own.contains(&name.as_str()));
        } else {
            adapter.local_environment.retain(|name, _| !foreign(name));
        }
        Ok(adapter)
    }

    fn configured(
        configuration: PiConfig,
        runners: ProviderProcessRunners,
    ) -> Result<Self, PiAdapterConfigError> {
        if configuration.executable.is_empty() {
            return Err(PiAdapterConfigError::EmptyExecutable);
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
        })
    }

    /// Per-session private homes. The agent directory holds Pi's configuration, model catalog, and
    /// `models.json`; sessions live in a `sessions` child directory so a node instance keeps one
    /// session across loop revisits.
    fn homes(&self, files: &ProviderExecutionFiles) -> (PathBuf, PathBuf) {
        let home = files.home();
        (agent_directory(home), session_directory(home))
    }

    fn command(
        &self,
        invocation: &DriverInvocation,
        files: &ProviderExecutionFiles,
        session_id: &str,
    ) -> Result<ProcessSessionCommand, NodeRunnerError> {
        let (agent_dir, sessions) = self.homes(files);
        let provider_override = self.configure_provider(invocation, &agent_dir)?;
        // A caller-owned endpoint ends native reuse: the credential and the endpoint then both
        // come from the connection, so inheriting the user's own agent directory would send a
        // stored login to a caller-owned host.
        let native_local = self.native_local()
            && !provider_override
            && !command::credential_declared(self.provider, &invocation.environment);
        let mut command = command(PiCommandRequest {
            provider: self.provider,
            native_local,
            contained: self.runners.is_hosted(),
            executable: &self.executable,
            prefix_arguments: &self.prefix_arguments,
            invocation,
            files,
            local_environment: &self.local_environment,
            agent_dir: &agent_dir,
            session_dir: &sessions,
            session_id,
        })?;
        // Every Pi child needs a home: Pi resolves its native agent directory, and its tools their
        // user-relative state, from `HOME`. A local run uses the invoking user's home so a
        // native-local lane finds `~/.pi/agent`; a contained run has no user home of its own and
        // falls back to its private runtime home, matching the capsule base environment.
        let runtime_home = self
            .runtime_home
            .to_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                NodeRunnerError::DriverDetail(
                    "Pi runtime home is not a valid non-empty platform path".to_owned(),
                )
            })?;
        let provider_home = self
            .local_user_home
            .as_deref()
            .and_then(Path::to_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(runtime_home);
        command
            .environment
            .entry("HOME".to_owned())
            .or_insert_with(|| provider_home.to_owned());
        if let Some(memory) = &self.active_memory {
            memory.configure(
                crate::active_memory::hooks::Harness::Pi,
                &mut command,
                crate::active_memory::hooks::HookInstallation {
                    invocation,
                    home: files.home(),
                },
            );
        }
        Ok(command)
    }

    /// Writes Pi's provider document and reports whether an endpoint override is present.
    fn configure_provider(
        &self,
        invocation: &DriverInvocation,
        agent_dir: &Path,
    ) -> Result<bool, NodeRunnerError> {
        // A caller-owned endpoint becomes Pi's own provider override in the private agent
        // directory. The secret stays in the declared connection, referenced by interpolation.
        // The gateway lane registers its own provider, carrying the caller's endpoint, key, and
        // wire protocol. The anthropic lane only retargets the endpoint and keeps Pi's Messages
        // protocol, the same escape hatch Claude Code offers through `ANTHROPIC_BASE_URL`.
        let (model, reasoning, context_window, max_tokens) = match &invocation.node.binding {
            NodeRuntimeBinding::Agent {
                model,
                effort,
                context_window,
                max_tokens,
                ..
            } => (
                model.as_str(),
                effort.is_some(),
                *context_window,
                *max_tokens,
            ),
            _ => {
                return Err(NodeRunnerError::DriverDetail(
                    "Pi command requires an agent runtime binding".to_owned(),
                ));
            }
        };
        let document = match self.provider {
            PiProvider::Gateway => {
                let base_url = gateway_base_url(&invocation.environment)?;
                let api = declared_api(&invocation.environment, command::GATEWAY_API)?;
                Some(write_gateway(
                    agent_dir,
                    provider_document::GatewayDocument {
                        base_url: &base_url,
                        api: &api,
                        model,
                        reasoning,
                        context_window,
                        max_tokens,
                    },
                ))
            }
            PiProvider::Anthropic => {
                // A declared endpoint wins; otherwise a local run honors the invoking shell's
                // `ANTHROPIC_BASE_URL`, the same setting the Claude lane inherits. Pi itself
                // ignores the variable for endpoint selection, so it becomes a provider override
                // here or it would be forwarded and silently dropped.
                let base_url =
                    declared_base_url(&invocation.environment, command::ANTHROPIC_BASE_URL)
                        .or_else(|| self.ambient_base_url(command::ANTHROPIC_BASE_URL));
                base_url.map(|base_url| {
                    write_endpoint(
                        agent_dir,
                        provider_document::ANTHROPIC,
                        &base_url,
                        anthropic_api_key_field(&invocation.environment),
                    )
                })
            }
            PiProvider::OpenAi => {
                // The OpenAI lane honors its own endpoint variables, which Codex already inherits,
                // so Pi reaches an OpenAI-compatible proxy instead of silently using the public
                // endpoint. The built-in Responses protocol still applies.
                let base_url = declared_base_url(&invocation.environment, command::OPENAI_BASE_URL)
                    .or_else(|| {
                        declared_base_url(&invocation.environment, command::OPENAI_API_BASE)
                    })
                    .or_else(|| self.ambient_base_url(command::OPENAI_BASE_URL))
                    .or_else(|| self.ambient_base_url(command::OPENAI_API_BASE));
                base_url.map(|base_url| {
                    write_endpoint(agent_dir, OPENAI, &base_url, Some(OPENAI.api_key_field))
                })
            }
            _ => None,
        };
        let overridden = document.is_some();
        document.transpose().map(|_| overridden).map_err(|error| {
            NodeRunnerError::DriverDetail(format!("Pi provider configuration failed: {error}"))
        })
    }

    /// The launch command for one turn, exposed so tests can assert the endpoint and home
    /// decisions without launching a process. Only the Unix-gated adapter tests use it, so it is
    /// compiled for the same platforms.
    #[cfg(all(test, unix))]
    pub(crate) fn command_for_test(
        &self,
        invocation: &DriverInvocation,
        files: &ProviderExecutionFiles,
        session_id: &str,
    ) -> Result<ProcessSessionCommand, NodeRunnerError> {
        self.command(invocation, files, session_id)
    }

    fn redactions(&self, resolved: &ResolvedEnvironment) -> Vec<String> {
        provider_redactions(resolved, &self.local_environment)
    }

    /// A non-empty endpoint value captured from the invoking shell, available only to a local
    /// adapter. Hosted adapters clear their ambient environment, so this is always `None` there.
    fn ambient_base_url(&self, field: &str) -> Option<String> {
        self.local_environment
            .get(field)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    }

    /// True when the lane reuses the user's stored Pi login for its provider. A declared credential
    /// or caller-owned endpoint ends that reuse for the turn, because Pi reads its agent directory
    /// before the process environment and would otherwise outrank both.
    #[must_use]
    pub(super) fn native_local(&self) -> bool {
        !self.runners.is_hosted() && self.provider.natively_authenticated()
    }
}

fn is_pi_credential(name: &str) -> bool {
    matches!(
        name,
        command::ANTHROPIC_KEY
            | "ANTHROPIC_AUTH_TOKEN"
            | "ANTHROPIC_OAUTH_TOKEN"
            | command::OPENAI_KEY
            | command::OPENROUTER_KEY
            | command::AWS_BEARER_TOKEN_BEDROCK
    )
}

/// The credential field an anthropic endpoint override may interpolate as `apiKey`.
///
/// Pi's own environment auth covers `ANTHROPIC_AUTH_TOKEN` and `ANTHROPIC_OAUTH_TOKEN`, but a
/// `models.json` `apiKey` that fails to resolve suppresses that fallback. So reference
/// `ANTHROPIC_API_KEY` only when the declared connection actually supplies it; otherwise omit
/// `apiKey` and let Pi resolve the token from the environment.
fn anthropic_api_key_field(resolved: &ResolvedEnvironment) -> Option<&'static str> {
    resolved
        .iter()
        .any(|(name, value)| name.as_str() == command::ANTHROPIC_KEY && !value.is_empty())
        .then_some(command::ANTHROPIC_KEY)
}

impl PiAdapter {
    async fn advance_turn(
        &self,
        turn: &PiTurn<'_>,
        state: &mut PiRunState,
        prompt: String,
    ) -> Result<PiTurnAdvance, NodeRunnerError> {
        turn.session
            .core
            .ensure_live(ClosedSessionFailure::SessionLost)?;
        let attempt = self.execute_turn(turn, state, prompt).await?;
        let expected = state.session_id.clone();
        match attempt {
            PiAttempt::Complete(result) => {
                observe_session(result.session_id.as_deref(), &expected)
                    .map_err(|detail| NodeRunnerError::DriverDetail(detail.to_owned()))?;
                resolve_pi_response(turn, result, state).await
            }
            PiAttempt::Failed(failure) => {
                observe_session(failure.session_id.as_deref(), &expected)
                    .map_err(|detail| NodeRunnerError::DriverDetail(detail.to_owned()))?;
                Ok(PiTurnAdvance::ProviderFailure {
                    retryable: failure.retryable,
                    diagnostic: failure.diagnostic,
                })
            }
        }
    }

    async fn execute_turn(
        &self,
        turn: &PiTurn<'_>,
        state: &PiRunState,
        prompt: String,
    ) -> Result<PiAttempt, NodeRunnerError> {
        let files = match turn.execution.prepare(turn.control).await {
            Ok(files) => files,
            Err(error) => {
                return match turn_process::failed_before_start(error, turn.control)? {
                    PiProcessStart::Ready(_) => {
                        unreachable!("a preparation failure cannot be ready")
                    }
                    PiProcessStart::Failed(attempt) => Ok(attempt),
                };
            }
        };
        let mut command = self.command(turn.invocation, &files, &state.session_id)?;
        // The minimal base environment supplies the executable search path and any run tool
        // directory. It is never read from the ambient process, and a declared connection cannot
        // shadow these names, so the base value always fills an unset name.
        for (name, value) in self.base_environment.clone_values() {
            command
                .environment
                .entry(name)
                .or_insert_with(|| value.clone());
        }
        let transcript = PiTranscript::new(self.redactions(&turn.invocation.environment));
        let mut process = match turn_process::open(files, command, turn.control).await? {
            PiProcessStart::Ready(process) => process,
            PiProcessStart::Failed(attempt) => return Ok(attempt),
        };
        turn_process::finish_process(&mut process, prompt.as_bytes(), transcript, turn.control)
            .await
    }
}

async fn resolve_pi_response(
    turn: &PiTurn<'_>,
    result: PiResult,
    state: &PiRunState,
) -> Result<PiTurnAdvance, NodeRunnerError> {
    let response = resolve_agent_response_with_schema(
        &turn.invocation.response,
        &result.message,
        state.dialect,
    )?;
    state
        .retry
        .report_correction(
            turn.control,
            &response,
            state.response.corrections_exhausted(),
        )
        .await?;
    if matches!(response, AgentResponse::Correction { .. }) {
        turn.control
            .emit(LiveOutput::new(
                LiveOutputStream::System,
                "Pi final output rejected; requesting correction",
            )?)
            .await?;
    }
    Ok(PiTurnAdvance::Response(response))
}

pub(crate) struct PiTurn<'a> {
    pub(super) invocation: &'a DriverInvocation,
    pub(super) session: &'a PiSession,
    pub(super) control: &'a DriverControl,
    pub(super) execution: &'a ProviderExecution<'a>,
}

#[cfg(test)]
#[path = "native_v2_pi/tests.rs"]
mod tests;
