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
}

/// Complete callback context and the steers awaiting a carrier acknowledgement.
#[derive(Default)]
pub struct HookDelivery {
    pub context: String,
    pub steers: Vec<openengine_cluster_protocol::VectorId>,
}

/// The host owns the immutable index, relevance budget, session deduplication and supplied receipts.
/// Returned strings must contain complete cards and stay within the configured context budget.
pub trait ActiveMemorySource: Send + Sync {
    fn tool_delivery(&self, query: &HookQuery) -> HookDelivery {
        HookDelivery {
            context: self.tool_context(query),
            steers: Vec::new(),
        }
    }
    fn acknowledged(&self, _query: &HookQuery, _delivery: &HookDelivery) {}
    fn unacknowledged(&self, _query: &HookQuery, _delivery: &HookDelivery) {}

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
        self.broker.retire(execution).await;
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

    pub(crate) fn configure_networks(
        &self,
        networks: &std::collections::BTreeMap<
            openengine_cluster_protocol::NodeName,
            openengine_cluster_protocol::SteeringNetwork,
        >,
    ) {
        self.source.configure_networks(networks);
    }

    pub(crate) fn publishing_authorized(&self, invocation: &DriverInvocation) -> bool {
        !auxiliary_invocation(invocation)
            && self
                .source
                .publishing_authorized(&invocation.node.reference.node)
    }

    /// The same host-owned capability supports publication in every native lane.
    pub(crate) fn configure_network_command(
        &self,
        command: &mut ProcessSessionCommand,
        invocation: &DriverInvocation,
        home: &Path,
    ) -> io::Result<()> {
        let capability = if self.publishing_authorized(invocation) {
            self.capability_file(invocation, &command.workspace.current_dir, home)?
        } else {
            None
        };
        self.configure_network_environment(
            &mut command.environment,
            invocation,
            capability.as_deref(),
        );
        Ok(())
    }

    /// Reuse the native carrier's file rather than replacing its live token.
    pub(crate) fn configure_network_environment(
        &self,
        environment: &mut std::collections::BTreeMap<String, String>,
        invocation: &DriverInvocation,
        capability: Option<&Path>,
    ) {
        environment.retain(|name, _| {
            if cfg!(windows) {
                !name.eq_ignore_ascii_case("ZEROSHOT_STEERING_CAPABILITY")
            } else {
                name != "ZEROSHOT_STEERING_CAPABILITY"
            }
        });
        if self.publishing_authorized(invocation)
            && let Some(path) = capability
        {
            environment.insert(
                "ZEROSHOT_STEERING_CAPABILITY".into(),
                path.to_string_lossy().into_owned(),
            );
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
        self.configure_network_environment(&mut command.environment, installation.invocation, None);
        let result = self
            .capability_file(
                installation.invocation,
                &command.workspace.current_dir,
                installation.home,
            )
            .and_then(|capability| match capability {
                Some(path) => {
                    self.configure_network_environment(
                        &mut command.environment,
                        installation.invocation,
                        Some(&path),
                    );
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

    pub(crate) async fn tool_delivery(
        &self,
        invocation: &DriverInvocation,
        input: &Value,
        workspace: &Path,
    ) -> (HookQuery, HookDelivery) {
        let query = broker::HookScope::new(invocation, workspace).query(input);
        let delivery = if auxiliary_invocation(invocation) {
            HookDelivery::default()
        } else {
            self.source.delivery(&query).await
        };
        (query, delivery)
    }
    pub(crate) async fn discard_delivery(&self, query: &HookQuery, delivery: &HookDelivery) {
        self.source.discard(query, delivery).await;
    }

    pub(crate) async fn acknowledge(&self, query: &HookQuery, delivery: &HookDelivery) {
        self.source.receipt(query, delivery).await;
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

/// Publish through an execution capability; request fields never establish sender authority.
/// Unlike the informational native hook, publication failures are explicit.
pub fn publish_network(
    capability_path: &Path,
    params: openengine_cluster_protocol::NetworkSteerParams,
) -> io::Result<openengine_cluster_protocol::RunSteerResult> {
    broker::publish_network(capability_path, params)
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
