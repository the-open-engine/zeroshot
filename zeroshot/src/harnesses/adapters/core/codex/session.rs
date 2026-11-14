use async_trait::async_trait;
use openengine_cluster_protocol::{TokenCount, WorkerOutcome};
use tokio::sync::Mutex;

use crate::native_v2_capsule::provider_process::{ProviderSessionCore, impl_provider_node_session};
use crate::native_v2_contract::{NodeRuntimeBinding, TokenUsageDelta};
use crate::native_v2_runner::{
    AgentResponse, DriverControl, DriverInvocation, NodeDriver, NodeRunnerError,
};

use super::NativeV2CodexAdapter;
use super::output::CodexOutput;

pub(super) struct CodexSession {
    pub(super) core: ProviderSessionCore,
    pub(super) thread_id: Mutex<Option<String>>,
    pub(super) usage: Mutex<Option<TokenUsageDelta>>,
    // Unlike the in-run accounting baseline, this must cover the latest provider process.
    pub(super) checkpoint_usage: Mutex<Option<TokenUsageDelta>>,
}

impl CodexSession {
    pub(super) fn new() -> Self {
        Self {
            core: ProviderSessionCore::new(),
            thread_id: Mutex::new(None),
            usage: Mutex::new(None),
            checkpoint_usage: Mutex::new(None),
        }
    }

    pub(super) async fn usage_delta(
        &self,
        observed: Option<TokenUsageDelta>,
        resumed: bool,
    ) -> Option<TokenUsageDelta> {
        let observed = observed?;
        let previous = *self.usage.lock().await;
        let mut delta = previous.filter(|_| resumed).map_or(observed, |previous| {
            if usage_reset(previous, observed) {
                observed
            } else {
                TokenUsageDelta {
                    input_tokens: token_delta(previous.input_tokens, observed.input_tokens),
                    output_tokens: token_delta(previous.output_tokens, observed.output_tokens),
                    cache_read_input_tokens: optional_token_delta(
                        previous.cache_read_input_tokens,
                        observed.cache_read_input_tokens,
                    ),
                    cache_creation_input_tokens: optional_token_delta(
                        previous.cache_creation_input_tokens,
                        observed.cache_creation_input_tokens,
                    ),
                }
            }
        });
        // Normalize only after the native cumulative delta; commit_usage retains native counters.
        // The API's ordinary-input calculation excludes both cache reads and writes:
        // https://developers.openai.com/api/docs/guides/prompt-caching (Monitor cache performance).
        // Codex maps those counters unchanged. See openai/codex at
        // c0cc9fc820f62e4dbdde68214dd6d792d6c8a7ee:
        // codex-rs/protocol/src/protocol.rs (TokenUsage::non_cached_input) and
        // codex-rs/codex-api/src/sse/responses.rs (From<ResponseCompletedUsage>,
        // parses_cache_write_token_usage: input=100 includes read=40 and write=60).
        delta.input_tokens = token_delta(
            delta.cache_read_input_tokens.unwrap_or_default(),
            delta.input_tokens,
        );
        delta.input_tokens = token_delta(
            delta.cache_creation_input_tokens.unwrap_or_default(),
            delta.input_tokens,
        );
        Some(delta)
    }

    pub(super) async fn commit_usage(&self, observed: Option<TokenUsageDelta>, resumed: bool) {
        *self.checkpoint_usage.lock().await = observed;
        // Missing usage preserves only the baseline of the thread being resumed.
        if !resumed || observed.is_some() {
            *self.usage.lock().await = observed;
        }
    }

    pub(super) async fn record_thread(
        &self,
        observed: Option<&str>,
        resumed: Option<&str>,
    ) -> Result<(), &'static str> {
        let expected = resumed
            .or(observed)
            .ok_or("Codex output did not provide a thread ID")?;
        if expected.is_empty() {
            return Err("Codex output provided an empty thread ID");
        }
        if expected.contains('\0') {
            return Err("Codex output thread ID contained a NUL byte");
        }
        if observed.is_some_and(|value| value != expected) {
            return Err("Codex output thread ID did not match the resumed session");
        }
        let mut thread_id = self.thread_id.lock().await;
        match thread_id.as_deref() {
            Some(current) if current != expected => {
                Err("Codex output thread ID changed across turns")
            }
            Some(_) => Ok(()),
            None => {
                *thread_id = Some(expected.to_owned());
                Ok(())
            }
        }
    }

    pub(super) async fn record_attempt_thread(
        &self,
        output: &CodexOutput,
        resumed: Option<&str>,
    ) -> Result<(), &'static str> {
        if output.thread_id.is_none() && resumed.is_none() {
            return Ok(());
        }
        self.record_thread(output.thread_id.as_deref(), resumed)
            .await
    }

    pub(super) async fn missing_required_thread(
        &self,
        invocation: &DriverInvocation,
        response: &AgentResponse,
    ) -> Option<&'static str> {
        if self.thread_id.lock().await.is_some() {
            return None;
        }
        if matches!(response, AgentResponse::Correction { .. }) {
            return Some("Codex output did not provide a thread ID required for correction");
        }
        matches!(
            &invocation.node.binding,
            NodeRuntimeBinding::Agent {
                session_scope: crate::execution::SessionScope::NodeInstance,
                ..
            }
        )
        .then_some("Codex output did not provide a thread ID required for reusable session")
    }
}

fn usage_reset(previous: TokenUsageDelta, observed: TokenUsageDelta) -> bool {
    observed.input_tokens < previous.input_tokens
        || observed.output_tokens < previous.output_tokens
        || optional_counter_decreased(
            previous.cache_read_input_tokens,
            observed.cache_read_input_tokens,
        )
        || optional_counter_decreased(
            previous.cache_creation_input_tokens,
            observed.cache_creation_input_tokens,
        )
}

fn optional_counter_decreased(previous: Option<TokenCount>, observed: Option<TokenCount>) -> bool {
    matches!((previous, observed), (Some(previous), Some(observed)) if observed < previous)
}

fn token_delta(previous: TokenCount, observed: TokenCount) -> TokenCount {
    TokenCount::new(observed.get().saturating_sub(previous.get())).unwrap_or(observed)
}

fn optional_token_delta(
    previous: Option<TokenCount>,
    observed: Option<TokenCount>,
) -> Option<TokenCount> {
    match (previous, observed) {
        (Some(previous), Some(observed)) => Some(token_delta(previous, observed)),
        (None, Some(observed)) => Some(observed),
        (_, None) => None,
    }
}

impl_provider_node_session!(CodexSession);

crate::native_v2_capsule::provider_process::impl_provider_session_factory!(
    NativeV2CodexAdapter,
    CodexSession::new()
);

#[async_trait]
impl NodeDriver for NativeV2CodexAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let invocation = self.memory_invocation(invocation);
        let session = crate::native_v2_capsule::provider_process::invocation_session::<CodexSession>(
            &invocation,
        )?;
        self.run_turn(&invocation, session, control).await
    }
}

#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;
