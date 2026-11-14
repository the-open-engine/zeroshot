use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformRunReceipt {
    pub run_id: RunId,
    pub deduped: bool,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CapsuleAllocationUnavailable {
    #[error("capsule allocation is unavailable")]
    Runtime,
    #[error("source checkout failed: repository access, branch, or revision is unavailable")]
    SourceCheckout,
    #[error("environment setup failed; inspect the preparation log")]
    EnvironmentSetup,
    #[error("environment startup failed; inspect the preparation log")]
    EnvironmentStartup,
}

impl CapsuleAllocationUnavailable {
    #[must_use]
    pub const fn failure_code(self) -> &'static str {
        match self {
            Self::Runtime => "runtime_unavailable",
            Self::SourceCheckout => "source_checkout_unavailable",
            Self::EnvironmentSetup => "environment_setup_failed",
            Self::EnvironmentStartup => "environment_startup_failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("exclusive controller authority is unavailable")]
pub struct ControllerClaimUnavailable;

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("capsule destruction could not be confirmed")]
pub struct CapsuleCleanupUnavailable;

/// Distinguishes a safely settled retained allocation failure from unconfirmed cleanup.
///
/// A settled failure leaves no live capsule and one durable recovery owner for the workspace.
/// Cleanup failure keeps the successor nonterminal so replacement-controller reconciliation can
/// confirm runtime cleanup before recording a terminal result.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RetainedAllocationUnavailable {
    #[error(transparent)]
    Settled(#[from] CapsuleAllocationUnavailable),
    #[error(transparent)]
    CleanupUnconfirmed(#[from] CapsuleCleanupUnavailable),
}

/// Opaque acknowledgement from allocator authority that the disposable runtime no longer exists.
///
/// For a live capsule this follows successful destruction. After an observed connection loss the
/// same receipt confirms that allocator authority observes the capsule absent; loss therefore
/// cannot strand an otherwise terminalizable run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapsuleDestroyed {
    _closed: (),
}

impl CapsuleDestroyed {
    #[must_use]
    pub const fn confirmed() -> Self {
        Self { _closed: () }
    }
}

#[async_trait]
pub trait CapsuleCleanup: Send + Sync {
    async fn destroy_or_confirm_absent(
        &self,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable>;
}

pub struct AllocatedCapsule {
    pub vector_seed: Vec<openengine_cluster_protocol::RunVector>,
    pub checkpoints: Option<Arc<dyn crate::native_v2_supervisor::checkpoints::RunCheckpointStore>>,
    pub execution_seed: Vec<crate::full_v1_reducer::DurableExecution>,
    pub runner: Arc<dyn NodeRunner>,
    pub loss: watch::Receiver<bool>,
    pub cleanup: Arc<dyn CapsuleCleanup>,
}

impl AllocatedCapsule {
    #[must_use]
    pub fn new(
        runner: Arc<dyn NodeRunner>,
        loss: watch::Receiver<bool>,
        cleanup: Arc<dyn CapsuleCleanup>,
    ) -> Self {
        Self {
            runner,
            loss,
            cleanup,
            checkpoints: None,
            execution_seed: Vec::new(),
            vector_seed: Vec::new(),
        }
    }
}

/// Run-owned preparation output. Implementations persist safe lines before returning.
#[async_trait]
pub trait PreparationProgress: Send + Sync {
    async fn log(&self, line: &str) -> Result<(), CapsuleAllocationUnavailable>;
}

#[derive(Clone)]
pub struct CapsulePreparation {
    pub environment: RunEnvironment,
    pub progress: Arc<dyn PreparationProgress>,
}

impl CapsulePreparation {
    pub fn quiet(
        runtime: &crate::native_v2_contract::RuntimePlan,
    ) -> Result<Self, crate::native_v2_supervisor::RunEnvironmentError> {
        Ok(Self {
            environment: RunEnvironment::exact(runtime, None, Default::default())?,
            progress: Arc::new(QuietPreparation),
        })
    }
}

struct QuietPreparation;

#[async_trait]
impl PreparationProgress for QuietPreparation {
    async fn log(&self, _line: &str) -> Result<(), CapsuleAllocationUnavailable> {
        Ok(())
    }
}

/// Ephemeral checkout and forge authority, excluded from immutable run state, with the
/// host-selected author and co-author of commits created by delivery.
#[derive(Clone, Default)]
pub struct RunSourceCredentials {
    pub github_token: Option<String>,
    pub git_credentials: Option<openengine_cluster_protocol::GitTransportCredentials>,
    pub commit_identity: Option<openengine_cluster_protocol::CommitIdentity>,
    pub commit_co_author: Option<openengine_cluster_protocol::CommitIdentity>,
}

impl std::fmt::Debug for RunSourceCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunSourceCredentials")
            .field(
                "github_token",
                &self.github_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("git_credentials", &self.git_credentials)
            .field("commit_identity", &self.commit_identity)
            .field("commit_co_author", &self.commit_co_author)
            .finish()
    }
}

pub struct CapsuleAllocationRequest<'a> {
    pub run_id: &'a RunId,
    pub admitted: &'a AdmittedRun,
    pub github_token: Option<&'a str>,
    pub git_credentials: Option<&'a openengine_cluster_protocol::GitTransportCredentials>,
    pub commit_identity: Option<&'a openengine_cluster_protocol::CommitIdentity>,
    pub commit_co_author: Option<&'a openengine_cluster_protocol::CommitIdentity>,
    pub preparation: CapsulePreparation,
}

pub struct RetainedAllocationRequest<'a> {
    pub preparation: CapsulePreparation,
    pub selection: crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection,
    pub source_run_id: &'a RunId,
    pub run_id: &'a RunId,
    pub admitted: &'a AdmittedRun,
    pub github_token: Option<&'a str>,
    pub git_credentials: Option<&'a openengine_cluster_protocol::GitTransportCredentials>,
    pub commit_identity: Option<&'a openengine_cluster_protocol::CommitIdentity>,
    pub commit_co_author: Option<&'a openengine_cluster_protocol::CommitIdentity>,
}

/// Allocator-owned proof that this is the only active controller for one run.
///
/// The allocator must keep the claim exclusive until the last reference is dropped. This is a
/// hosting authority contract, not a product-local distributed lease implementation.
pub trait ExclusiveControllerClaim: Send + Sync {}

#[async_trait]
pub trait CapsuleAllocator: Send + Sync {
    /// None refuses suspension and wait plans; hosts with remote storage explicitly opt in.
    fn suspension_policy(&self) -> Option<crate::native_v2_wait::WaitPolicy> {
        None
    }

    /// Acquires exclusive controller authority for the supplied public run identity.
    async fn claim_controller(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable>;

    /// Retains cleanup authority by run identity before starting runtime processes, including
    /// when this future is cancelled or returns an error. The caller must confirm destruction
    /// through [`Self::destroy_or_confirm_absent`] before recording a terminal allocation failure.
    /// Once allocation succeeds, cleanup authority is also carried by [`AllocatedCapsule`].
    async fn allocate(
        &self,
        request: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable>;

    /// Destroys an allocator-known capsule for a controller-reconstructed run, or confirms that
    /// it is already absent. This operation never allocates a replacement.
    async fn destroy_or_confirm_absent(
        &self,
        run_id: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable>;

    /// Durably reserves the retained workspace for one successor before resume is acknowledged.
    /// This performs the existing ownership handoff without credentials or runtime preparation.
    async fn reserve_retained_workspace(
        &self,
        _source_run_id: &RunId,
        _run_id: &RunId,
        _selection: &crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection,
    ) -> Result<(), RetainedAllocationUnavailable> {
        Err(CapsuleAllocationUnavailable::Runtime.into())
    }

    /// Prepares the successor using its reserved retained workspace.
    async fn allocate_from_retained(
        &self,
        _request: RetainedAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, RetainedAllocationUnavailable> {
        Err(CapsuleAllocationUnavailable::Runtime.into())
    }

    async fn workspace_recovery(
        &self,
        _run_id: &RunId,
    ) -> openengine_cluster_protocol::WorkspaceRecovery {
        Default::default()
    }

    /// Reconcile predecessor ledger facts into the existing lineage before any successor claim.
    /// This must not capture workspace bytes or restore graph execution history.
    async fn retain_delivery_state(
        &self,
        _run_id: &RunId,
        _state: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "delivery recovery storage is unavailable",
        )
        .into())
    }

    async fn checkpoints(
        &self,
        _params: openengine_cluster_protocol::RunCheckpointsParams,
    ) -> Result<
        openengine_cluster_protocol::RunCheckpointsResult,
        crate::native_v2_supervisor::checkpoints::CheckpointError,
    > {
        Err(crate::native_v2_supervisor::checkpoints::CheckpointError(
            std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "workspace checkpoints are unavailable",
            ),
        ))
    }

    async fn discard_workspace(&self, _run_id: &RunId) -> Result<bool, CapsuleCleanupUnavailable> {
        Ok(false)
    }
}
