//! Private host-owned composition for delivery validation.

use std::path::Path;

use async_trait::async_trait;
use openengine_cluster_protocol::RunId;

use crate::execution::process::HostedProcessPool;
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_delivery::validation::TrustedDeliveryValidator;
use crate::native_v2_runner::NodeRunnerError;

/// Immutable allocation facts, supplied after checkout and before any agent execution.
///
/// The admitted graph and input identify the accepted request; they do not authorize access to
/// another host resource. The factory must resolve any external authority from its private state.
pub struct HostedDeliveryValidationRequest<'a> {
    pub run_id: &'a RunId,
    pub delivery_run_id: &'a RunId,
    pub admitted: &'a AdmittedRun,
    pub workspace: &'a Path,
    pub runtime_home: &'a Path,
    pub process_pool: HostedProcessPool,
}

/// Optional private host integration at the existing trusted delivery boundary.
///
/// Hosts select and prepare the executable outside the checkout; no authored executable path or
/// credential is accepted. Repository commands must use the allocated process pool, never the
/// controller identity. Preparation must retain no unowned work after cancellation or failure.
#[async_trait]
pub trait HostedDeliveryValidationFactory: Send + Sync {
    async fn prepare(
        &self,
        request: HostedDeliveryValidationRequest<'_>,
    ) -> Result<Option<TrustedDeliveryValidator>, NodeRunnerError>;
}
