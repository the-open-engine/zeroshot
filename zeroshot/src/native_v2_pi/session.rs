//! Pi session and driver.
//!
//! The Pi session is addressed by a derived identifier rather than a provider-assigned one: Pi
//! creates the exact `--session-id` when it is missing, so Zeroshot already knows which session a
//! turn belongs to. The reported header still has to agree, because a mismatch would silently
//! split one logical node session across two provider sessions.

use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::WorkerOutcome;

use crate::execution::SessionScope;
use crate::native_v2_capsule::provider_process::{
    ProviderExecution, ProviderFilesystemConfig, ProviderFailure, ProviderFailureRetry,
    ProviderSessionCore, impl_provider_node_session,
};
use crate::native_v2_contract::{NodeInvocation, NodeRuntimeBinding, PiProvider};
use crate::native_v2_runner::{
    AgentResponse, AgentResponseState, DriverControl, DriverInvocation, NodeDriver,
    NodeRunnerError, NodeSession, ProviderSchemaDialect, ResolvedEnvironment, SessionFactory,
};

use super::session_id::pi_session_id;
use super::{PiAdapter, PiTurn, prompt, schema_dialect};

pub(crate) struct PiSession {
    pub(crate) core: ProviderSessionCore,
}

impl PiSession {
    fn new() -> Self {
        Self {
            core: ProviderSessionCore::new(),
        }
    }

    /// A session with no live provider process, for tests that exercise argv and identity only.
    /// Only the Unix-gated adapter tests use it, so it is compiled for the same platforms.
    #[cfg(all(test, unix))]
    pub(crate) fn for_test() -> Self {
        Self::new()
    }
}

impl_provider_node_session!(PiSession);

/// One logical node session: the derived Pi session ID plus the correction state.
pub(super) struct PiRunState {
    pub(super) session_id: String,
    /// The schema dialect matching the lane's wire protocol, shared with response resolution so a
    /// correction restates the same schema the initial prompt carried.
    pub(super) dialect: ProviderSchemaDialect,
    pub(super) response: AgentResponseState,
    pub(super) retry: ProviderFailureRetry,
}

pub(super) enum PiTurnAdvance {
    Response(AgentResponse),
    ProviderFailure { retryable: bool, diagnostic: String },
}

#[async_trait]
impl SessionFactory for PiAdapter {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        let NodeRuntimeBinding::Agent { .. } = &invocation.binding else {
            return Err(NodeRunnerError::SessionOpen);
        };
        Ok(Arc::new(PiSession::new()))
    }
}

#[async_trait]
impl NodeDriver for PiAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let invocation = self.memory_invocation(invocation);
        let session = invocation
            .session
            .as_any()
            .downcast_ref::<PiSession>()
            .ok_or(NodeRunnerError::Driver)?;
        let _turn = session.core.turn.lock().await;
        let execution = ProviderExecution::new(
            ProviderFilesystemConfig {
                runners: self.runners,
                root: &self.runtime_home,
                workspace: &self.workspace,
            },
            &invocation,
            &session.core,
        );
        let turn = PiTurn {
            invocation: &invocation,
            session,
            control: &control,
            execution: &execution,
        };
        let mut state = PiRunState::new(
            self.provider,
            &invocation,
            self.redactions(&invocation.environment),
        )?;
        loop {
            if let Some(outcome) = self
                .advance_run(&turn, &mut state, &control)
                .await
                .map_err(|error| state.retry.redact_error(error))?
            {
                return Ok(outcome);
            }
        }
    }
}

impl PiAdapter {
    async fn advance_run(
        &self,
        turn: &PiTurn<'_>,
        state: &mut PiRunState,
        control: &DriverControl,
    ) -> Result<Option<WorkerOutcome>, NodeRunnerError> {
        let prompt = state.response.prompt().to_owned();
        match self.advance_turn(turn, state, prompt).await? {
            PiTurnAdvance::Response(response) => {
                // A correction prompt is sent in the same session, so the derived ID is unchanged.
                state.response.accept("Pi", control, response).await
            }
            PiTurnAdvance::ProviderFailure {
                retryable,
                diagnostic,
            } => {
                state
                    .retry_provider_failure(turn, retryable, &diagnostic)
                    .await?;
                Ok(None)
            }
        }
    }
}

impl PiRunState {
    fn new(
        provider: PiProvider,
        invocation: &DriverInvocation,
        redactions: Vec<String>,
    ) -> Result<Self, NodeRunnerError> {
        let session_id = pi_session_id(&identity(invocation)?).map_err(|detail| {
            NodeRunnerError::DriverDetail(format!("Pi session identity is unusable: {detail}"))
        })?;
        let dialect = schema_dialect(provider, &invocation.environment)?;
        let prompt = prompt(dialect, invocation)?;
        Ok(Self {
            session_id,
            dialect,
            response: AgentResponseState::new(prompt.clone()),
            retry: ProviderFailureRetry::new("Pi", prompt, redactions),
        })
    }

    /// Sends the provider's documented continuation and correction prompts in the same session.
    async fn retry_provider_failure(
        &mut self,
        turn: &PiTurn<'_>,
        retryable: bool,
        detail: &str,
    ) -> Result<(), NodeRunnerError> {
        let prompt = self
            .retry
            .after_failure(
                turn.control,
                ProviderFailure {
                    detail: Some(detail),
                    retryable,
                    // Zeroshot derives the session ID, so a session always exists.
                    has_session: true,
                },
            )
            .await?;
        self.response.replace_prompt(prompt);
        Ok(())
    }
}

/// One session identity per scope: a node instance keeps its session across loop revisits, while
/// an execution always starts fresh.
pub(super) fn identity(invocation: &DriverInvocation) -> Result<String, NodeRunnerError> {
    let NodeRuntimeBinding::Agent { session_scope, .. } = &invocation.node.binding else {
        return Err(NodeRunnerError::Driver);
    };
    let execution = invocation.node.reference.execution.get();
    let instance = invocation.provider_session_slot.get();
    Ok(match session_scope {
        SessionScope::NodeInstance => format!("node-instance/{instance}"),
        SessionScope::Execution => format!("execution/{execution}"),
    })
}
