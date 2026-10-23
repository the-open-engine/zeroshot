//! Native harness bridges for deterministic, run-owned Active Memory retrieval.
//!
//! Only a private, run-scoped capability crosses into a harness. Matching remains in the
//! controller's resident index; hooks never receive the persistent store or provider credentials.

mod broker;
mod native;
#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openengine_cluster_protocol::{NodeInstructions, NodeRuntimeBinding};

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
}

/// The host owns the immutable index, relevance budget, session deduplication and supplied receipts.
/// Returned strings must contain complete cards and stay within the configured context budget.
pub trait ActiveMemorySource: Send + Sync {
    fn tool_context(&self, query: &HookQuery) -> String;
    fn graph_context(&self, node_id: &str, session_key: &str) -> String;
    fn unavailable(&self, _reason: &str) {}
}

/// Shared by all effective runtime lanes in one run. Dropping it closes the callback listener.
pub struct ActiveMemoryHooks {
    source: Arc<dyn ActiveMemorySource>,
    broker: broker::Broker,
}

pub(crate) struct HookInstallation<'a> {
    pub invocation: &'a DriverInvocation,
    pub home: &'a Path,
}

impl ActiveMemoryHooks {
    pub fn new(source: Arc<dyn ActiveMemorySource>) -> io::Result<Self> {
        Ok(Self {
            broker: broker::Broker::new(Arc::clone(&source))?,
            source,
        })
    }

    pub(crate) fn configure(
        &self,
        harness: native::Harness,
        command: &mut ProcessSessionCommand,
        installation: HookInstallation<'_>,
    ) {
        if auxiliary_invocation(installation.invocation) {
            return;
        }
        let result = self
            .broker
            .capability(installation.invocation, &command.workspace.current_dir)
            .and_then(|capability| {
                native::configure(harness, command, installation.home, &capability)
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
        let Some(instructions) = invocation.node.instructions.as_ref() else {
            return invocation;
        };
        let session = provider_session_key(&invocation);
        let context = self
            .source
            .graph_context(invocation.node.reference.node.as_str(), &session);
        if context.is_empty() {
            return invocation;
        }
        let augmented = format!("{}{}", instructions.as_str(), context,);
        match NodeInstructions::new(augmented) {
            Ok(instructions) => invocation.node.instructions = Some(instructions),
            Err(_) => self
                .source
                .unavailable("Active Memory graph guidance exceeded instruction bounds"),
        }
        invocation
    }

    pub(crate) fn execution_failed(&self) {
        self.source.unavailable(
            "Active Memory delivery could not be confirmed for a failed provider execution",
        );
    }

    pub(crate) fn copilot_context(
        &self,
        invocation: &DriverInvocation,
        input: &Value,
        workspace: &Path,
    ) -> String {
        if auxiliary_invocation(invocation) {
            return String::new();
        }
        let scope = broker::HookScope::new(invocation, workspace);
        let query = scope.query(input);
        native::bounded_context(self.source.tool_context(&query))
    }
}

fn provider_session_key(invocation: &DriverInvocation) -> String {
    match &invocation.node.binding {
        NodeRuntimeBinding::Agent {
            session_scope: SessionScope::NodeInstance,
            ..
        } => format!("node-instance/{}", invocation.provider_session_slot.get()),
        _ => format!("execution/{}", invocation.node.reference.execution.get()),
    }
}

fn auxiliary_invocation(invocation: &DriverInvocation) -> bool {
    let offset = crate::native_v2_runner::AUXILIARY_PROVIDER_SLOT_OFFSET;
    invocation.node.reference.execution.get() > offset
        || invocation.provider_session_slot.get() > offset
}

/// Hidden CLI entry point. All failures return empty informational output and allow the native
/// operation to continue. The private file contains only a callback capability, never model keys.
pub fn run_hook_helper(capability_path: &Path) -> io::Result<()> {
    broker::run_helper(capability_path)
}

pub(crate) use native::Harness;

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
