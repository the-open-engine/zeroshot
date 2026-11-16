//! Native harness bridges for run-owned memory and steering delivery.
//!
//! Only a private, run-scoped capability crosses into a harness. Memory matching uses the
//! resident index; steering uses the run record. Hooks receive neither store nor credentials.

mod broker;
pub(crate) mod native;
mod steering;
#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openengine_cluster_protocol::NodeRuntimeBinding;

use crate::execution::SessionScope;
use serde_json::Value;

use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_runner::DriverInvocation;

/// An observable tool signal, normalized by the harness bridge before resident lookup.
#[derive(Clone, Debug)]
pub struct HookQuery {
    pub node_id: String,
    pub session_key: String,
    pub execution_id: u64,
    pub native_session_id: Option<String>,
    pub tool_name: String,
    pub tool_input: Value,
    pub cwd: PathBuf,
    /// A native subagent's tool call; context returned to it never reaches this conversation.
    pub subagent: bool,
    pub steers: SteerCarrier,
    /// Whether memory context returned at a tool call stays in the conversation. Copilot applies
    /// it to the turn in progress and writes none of it to its session, so a later execution of
    /// the same conversation is given a matching lesson again.
    pub context_kept: bool,
}

/// How a steer reaches a conversation at a tool call. A steer is the operator's own words: where
/// the harness has a channel for user messages it travels there, never inside a tool result.
/// A conversation that was handed a steer either way is not given it again.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SteerCarrier {
    /// Inside the tool call's hook context. The carrier's acknowledgement is the hand-over.
    #[default]
    HookContext,
    /// As a user message the carrier stores in the conversation before it acknowledges (OpenCode).
    StoredMessage,
    /// As a user message whose arrival in the conversation the harness reports in its event
    /// stream; the adapter confirms the hand-over then (Claude Code, Copilot, Pi).
    ShownMessage,
}

/// What one tool call hands over: memory context, and the steers selected for it. `steer` holds
/// their framed text when they travel as a message of their own instead of inside `context`.
#[derive(Default)]
pub struct HookDelivery {
    pub context: String,
    pub steers: Vec<openengine_cluster_protocol::VectorId>,
    pub steer: String,
}

/// The host owns the immutable index, relevance budget, session deduplication and supplied receipts.
/// Returned strings must contain complete cards and stay within the configured context budget.
pub trait ActiveMemorySource: Send + Sync {
    fn tool_delivery(&self, query: &HookQuery) -> HookDelivery {
        HookDelivery {
            context: self.tool_context(query),
            ..HookDelivery::default()
        }
    }
    fn acknowledged(&self, _query: &HookQuery, _delivery: &HookDelivery) {}

    fn tool_context(&self, query: &HookQuery) -> String;
    fn graph_context(&self, node_id: &str, session_key: &str) -> String;
    fn unavailable(&self, _reason: &str) {}
}

/// Shared by all effective runtime lanes in one run. Dropping it closes the callback listener.
pub struct ActiveMemoryHooks {
    source: Arc<steering::DeliverySource>,
    broker: broker::Broker,
}

pub(crate) struct HookInstallation<'a> {
    pub invocation: &'a DriverInvocation,
    pub home: &'a Path,
    pub steers: SteerRoute,
}

/// Where the broker sends the steers a tool call selects.
#[derive(Clone, Default)]
pub(crate) enum SteerRoute {
    /// Inside the hook context it returns.
    #[default]
    HookContext,
    /// Beside the memory context, to the adapter's extension, which sends each as a message of
    /// its own. `stored` says its acknowledgement follows the harness storing the steer;
    /// otherwise the adapter confirms the steer with `ActiveMemoryHooks::steer_shown`.
    Extension { stored: bool },
    /// To the adapter, which writes them into the conversation and confirms each with
    /// `ActiveMemoryHooks::steer_shown`.
    Adapter(tokio::sync::mpsc::UnboundedSender<String>),
}

impl SteerRoute {
    pub(super) fn carrier(&self) -> SteerCarrier {
        match self {
            Self::HookContext => SteerCarrier::HookContext,
            Self::Extension { stored: true } => SteerCarrier::StoredMessage,
            Self::Extension { stored: false } | Self::Adapter(_) => SteerCarrier::ShownMessage,
        }
    }
}

#[cfg(test)]
impl<'a> ToolCall<'a> {
    /// A tool call of a carrier that takes everything as hook context and keeps it.
    pub(crate) fn hook(
        invocation: &'a DriverInvocation,
        input: &'a Value,
        workspace: &'a Path,
    ) -> Self {
        Self {
            invocation,
            input,
            workspace,
            steers: SteerCarrier::HookContext,
            context_kept: true,
        }
    }
}

/// A tool call an adapter observes in process, with what its harness does with the answer.
pub(crate) struct ToolCall<'a> {
    pub invocation: &'a DriverInvocation,
    pub input: &'a Value,
    pub workspace: &'a Path,
    pub steers: SteerCarrier,
    /// See `HookQuery::context_kept`.
    pub context_kept: bool,
}

impl ActiveMemoryHooks {
    pub(crate) fn empty() -> io::Result<Self> {
        Self::new(Arc::new(steering::EmptySource))
    }
    pub(crate) fn bind_ledger(
        &self,
        run: openengine_cluster_protocol::RunId,
        ledger: Arc<dyn crate::v2_run_ledger::RunLedger>,
    ) {
        self.source.bind(run, ledger);
    }
    pub(crate) fn restore_steers(
        &self,
        instance: u64,
        receipts: crate::native_v2_runner::conversations::SteeringReceipts,
    ) {
        self.source.restore(instance, receipts);
    }
    pub(crate) fn capture_steers(
        &self,
    ) -> std::collections::BTreeMap<
        crate::native_v2_contract::NodeInstanceId,
        crate::native_v2_runner::conversations::SteeringReceipts,
    > {
        self.source.capture()
    }
    pub(crate) async fn prepare_steers(
        &self,
        invocation: &mut DriverInvocation,
        continued: bool,
    ) -> Option<u64> {
        if auxiliary_invocation(invocation) {
            return None;
        }
        self.source.prepare(invocation, continued).await
    }
    /// Settles an execution or releases unconfirmed prompt offers before a native retry.
    pub(crate) async fn finish_steers(&self, execution: u64, success: bool) {
        self.broker.retire(execution);
        self.source.finish(execution, success).await;
    }
    pub(crate) async fn steer_prompt(
        &self,
        invocation: &DriverInvocation,
        control: &crate::native_v2_runner::DriverControl,
        prompt: String,
    ) -> String {
        let mut supplemental = invocation.clone();
        supplemental.host_context = None;
        self.prepare_steers(&mut supplemental, control.is_restored_conversation())
            .await;
        match supplemental.host_context {
            Some(context) => DriverInvocation::prepend_host_context(prompt, &context),
            None => prompt,
        }
    }

    pub fn new(source: Arc<dyn ActiveMemorySource>) -> io::Result<Self> {
        let source = Arc::new(steering::DeliverySource::new(source));
        Ok(Self {
            broker: broker::Broker::new(source.clone())?,
            source,
        })
    }

    /// Creates the same bounded, run-scoped callback capability for every native bridge.
    /// The adapter owns hook/extension installation and acknowledges context it returns.
    pub(crate) fn capability_file(
        &self,
        invocation: &DriverInvocation,
        workspace: &Path,
        home: &Path,
    ) -> io::Result<Option<PathBuf>> {
        if auxiliary_invocation(invocation) {
            return Ok(None);
        }
        let capability = self.broker.capability(invocation, workspace)?;
        let bytes = serde_json::to_vec(&capability)?;
        crate::native_v2_capsule::provider_process::write_provider_file(
            home,
            "active-memory-capability.json",
            &bytes,
        )
        .map(Some)
    }

    /// The host owns the callback capability; the adapter installs its native carrier.
    pub(crate) fn configure_with(
        &self,
        command: &mut ProcessSessionCommand,
        installation: HookInstallation<'_>,
        install: fn(&mut ProcessSessionCommand, &Path) -> io::Result<()>,
    ) {
        let result = self
            .capability_file(
                installation.invocation,
                &command.workspace.current_dir,
                installation.home,
            )
            .and_then(|capability| match capability {
                Some(path) => {
                    self.broker
                        .route(installation.invocation, installation.steers);
                    install(command, &path)
                }
                None => Ok(()),
            });
        if result.is_err() {
            self.source
                .unavailable("Active Memory hook configuration was unavailable");
        }
    }

    /// Supplement only the private invocation. The admitted graph, role and response contract
    /// remain unchanged, and learned text cannot replace any authored instruction.
    pub(crate) fn invocation(&self, mut invocation: DriverInvocation) -> DriverInvocation {
        if auxiliary_invocation(&invocation) {
            return invocation;
        }
        if invocation.node.instructions.is_none() {
            return invocation;
        }
        let session = provider_session_key(&invocation);
        let context = self
            .source
            .graph_context(invocation.node.reference.node.as_str(), &session);
        if context.is_empty() {
            return invocation;
        }
        invocation.host_context = Some(match invocation.host_context.take() {
            Some(host) if !host.is_empty() => format!("{host}\n\n{context}"),
            _ => context,
        });
        invocation
    }

    pub(crate) fn execution_failed(&self) {
        self.source.unavailable(
            "Active Memory delivery could not be confirmed for a failed provider execution",
        );
    }

    #[cfg(test)]
    pub(crate) fn tool_context(
        &self,
        invocation: &DriverInvocation,
        input: &Value,
        workspace: &Path,
    ) -> String {
        if auxiliary_invocation(invocation) {
            return String::new();
        }
        let query = broker::HookScope::new(invocation, workspace).query(input);
        native::bounded_context(self.source.tool_context(&query))
    }

    pub(crate) async fn tool_delivery(&self, call: ToolCall<'_>) -> (HookQuery, HookDelivery) {
        let mut query = broker::HookScope::new(call.invocation, call.workspace).query(call.input);
        query.steers = call.steers;
        query.context_kept = call.context_kept;
        let delivery = if auxiliary_invocation(call.invocation) {
            HookDelivery::default()
        } else {
            self.source.delivery(&query).await
        };
        (query, delivery)
    }
    /// Acknowledges a delivery made in process, as the broker does once its carrier answered.
    #[cfg(test)]
    pub(crate) async fn acknowledge(&self, query: &HookQuery, delivery: &HookDelivery) {
        self.source.receipt(query, delivery).await;
    }
    /// Confirms a steer message sent at a tool call: the harness showed `text` in the
    /// conversation of `execution`. Text that is no such message is ignored.
    pub(crate) async fn steer_shown(&self, execution: u64, text: &str) {
        self.source.shown(execution, text).await;
    }
}

fn provider_session_key(invocation: &DriverInvocation) -> String {
    match &invocation.node.binding {
        NodeRuntimeBinding::Agent {
            session_scope: SessionScope::NodeInstance,
            ..
        } => format!("node-instance/{}", invocation.provider_home_slot.get()),
        _ => format!("execution/{}", invocation.provider_home_slot.get()),
    }
}

fn auxiliary_invocation(invocation: &DriverInvocation) -> bool {
    invocation.auxiliary
}

/// Hidden CLI entry point. All failures return empty informational output and allow the native
/// operation to continue. The private file contains only a callback capability, never model keys.
pub fn run_hook_helper(capability_path: &Path) -> io::Result<()> {
    broker::run_helper(capability_path)
}

pub(crate) use native::command_handler;

/// All four adapters attach the same optional run-owned capability. Keep their builder and
/// private-invocation behavior together so a future policy change reaches every harness.
macro_rules! impl_active_memory_adapter {
    ($adapter:ident) => {
        impl $adapter {
            #[must_use]
            pub fn with_active_memory(
                mut self,
                memory: Option<std::sync::Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
            ) -> Self {
                self.active_memory = memory;
                self
            }

            fn memory_invocation(
                &self,
                invocation: crate::native_v2_runner::DriverInvocation,
            ) -> crate::native_v2_runner::DriverInvocation {
                match &self.active_memory {
                    Some(memory) => memory.invocation(invocation),
                    None => invocation,
                }
            }
        }
    };
}

pub(crate) use impl_active_memory_adapter;
