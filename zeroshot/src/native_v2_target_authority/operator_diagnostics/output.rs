use std::fmt;

use openengine_cluster_protocol::TargetOperatorDiagnostic;
use tokio::sync::broadcast;

pub(super) const DIAGNOSTIC_OUTPUT_CAPACITY: usize = 128;

/// Optional host-owned collection of private operator diagnostics.
///
/// Publishing never waits for the collector or invokes host code. The channel retains at most
/// 128 records, each with at most 4 KiB of stdout and 4 KiB of stderr. A slow receiver reports
/// [`broadcast::error::RecvError::Lagged`] with the number of overwritten records, even when no
/// further diagnostics arrive. Dropping the receiver disables collection without affecting runs
/// or the target's small private HTTP snapshot.
///
/// This is an in-memory handoff, not durable storage. The host owns persistence, export, retention,
/// and recording lag as incomplete diagnostics. Create a separate channel for each target factory,
/// install it before building the target, and drain continuously into host-owned storage. On
/// shutdown, stop the target's producers, drop all
/// output/configuration clones, and drain the receiver through [`broadcast::error::RecvError::Closed`]
/// before disposing the workspace. Abrupt process termination can lose records not yet persisted.
///
/// Records are bounded and sanitized by the existing producers. They remain private operator data,
/// outside public run observation. Hosts must restrict access and namespace each record's `id` by
/// its target process/attempt; IDs restart with a new store. The host supplies deployment identity
/// and collection timestamps in its own storage envelope.
///
/// A fatal runtime error emits a `supervisor.drive` record before recovery waits. A recovery error
/// adds a `supervisor.fail_runtime` record with the same `runtime_failed` code and run ID. Group
/// failure summaries by run ID and use `operation` to distinguish these stages.
#[derive(Clone)]
pub struct OperatorDiagnosticOutput {
    sender: broadcast::Sender<TargetOperatorDiagnostic>,
}

impl OperatorDiagnosticOutput {
    /// Creates the bounded output and its collector without requiring a Tokio runtime.
    ///
    /// ```
    /// use zeroshot_engine::native_v2_hosting::ProductionHostingConfig;
    /// use zeroshot_engine::native_v2_target_authority::OperatorDiagnosticOutput;
    ///
    /// let (output, receiver) = OperatorDiagnosticOutput::channel();
    /// let config = ProductionHostingConfig {
    ///     operator_diagnostic_output: Some(output),
    ///     ..ProductionHostingConfig::default()
    /// };
    /// // The host owns `receiver`; handle both records and Lagged before building with `config`.
    /// ```
    #[must_use]
    pub fn channel() -> (Self, broadcast::Receiver<TargetOperatorDiagnostic>) {
        let (sender, receiver) = broadcast::channel(DIAGNOSTIC_OUTPUT_CAPACITY);
        (Self { sender }, receiver)
    }

    pub(super) fn publish(&self, diagnostic: TargetOperatorDiagnostic) {
        // Run execution and the private snapshot remain independent of collector availability.
        let _ = self.sender.send(diagnostic);
    }
}

impl fmt::Debug for OperatorDiagnosticOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperatorDiagnosticOutput(..)")
    }
}
