mod identity_leases;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use fs2::FileExt;
use openengine_cluster_protocol::{CheckpointId, RunCheckpointsParams, RunCheckpointsResult, RunId};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, watch};

use crate::execution::process::{HostedProcessIdentity, HostedProcessPool, HostedProcessScope};
use crate::native_v2_admission::writer_nodes;
use crate::native_v2_candidate::{
    NativeV2CandidateConfig, NativeV2HarnessConfig, build_native_v2_candidate,
};
use crate::native_v2_capsule::{
    CapsuleFilesystem, CapsuleFilesystemSpec, NativeCapsuleNodeEndpoint, RemoteCapsuleNodeRunner,
    prepare_capsule_filesystem,
};
use crate::native_v2_claude::{ClaudeAdapterConfig, ClaudeProcessEnvironment};
use crate::native_v2_cloud::{
    AllocatedCapsule, CapsuleAllocationUnavailable, CapsuleAllocator, CapsuleCleanup,
    CapsuleCleanupUnavailable, CapsuleDestroyed, ControllerClaimUnavailable,
    ExclusiveControllerClaim, RetainedAllocationRequest, RetainedAllocationUnavailable,
};
use crate::native_v2_codex::NativeV2CodexConfig;
use crate::native_v2_contract::{AdmittedRun, RuntimePlan};
use crate::native_v2_delivery::{
    DeliveryLineage, GhCliAuthorityConfig, GhCliDeliveryAuthority, NativeV2DeliveryConfig,
};
use crate::native_v2_delivery::DeliveryTarget;
use crate::native_v2_portable_controller::WorkspaceIdentity;
use crate::native_v2_supervisor::RunRuntimeExit;
use crate::native_v2_supervisor::checkpoints::{
    self, CheckpointError, CheckpointRestore, FilesystemCheckpointStore,
};
use crate::native_v2_target_authority::OperatorDiagnosticStore;
use serde::{Deserialize, Serialize};

use super::{
    HostedWorkspace, HostedWorkspaceRequest, HostedWorkspaceStorage, ProductionHostingError,
    set_traversable_directory,
};
use super::repository::{RepositoryInstall, install_repository, production_source};
use identity_leases::{ActiveRunProcessPool, ActiveRunProcessPools};

pub(super) struct ProductionCapsuleConfig {
    pub storage_root: PathBuf,
    pub copilot_executable: PathBuf,
    pub codex_executable: PathBuf,
    pub claude_executable: String,
    pub claude_prefix_arguments: Vec<String>,
    pub claude_process_environment: ClaudeProcessEnvironment,
    pub executable_search_path: String,
    pub git_program: PathBuf,
    pub gh_program: PathBuf,
    pub process_pool: HostedProcessPool,
    pub operator_diagnostics: Arc<OperatorDiagnosticStore>,
    pub workspace_storage: Option<Arc<dyn HostedWorkspaceStorage>>,
}

type FilesystemPreparer =
    fn(&Path, &Path, HostedProcessPool) -> Result<CapsuleFilesystem, CapsuleAllocationUnavailable>;

const RECOVERY_DIRECTORY: &str = "workspace-recovery";
const MAX_RECOVERY_LINEAGE_DEPTH: usize = 64;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HostedRecoveryDocument {
    recoverable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resumed_from: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    successor_run_id: Option<RunId>,
}

struct CapsuleBuildRequest<'a> {
    run_id: &'a RunId,
    delivery_run_id: &'a RunId,
    adopt_existing_delivery: bool,
    admitted: &'a AdmittedRun,
    github_token: Option<&'a str>,
    install_source: bool,
    transfer_retained_workspace: bool,
}

struct WorkspacePreparation<'a> {
    run_id: &'a RunId,
    workspace: &'a Path,
    admitted: &'a AdmittedRun,
    state: &'a ProductionCapsuleState,
    process_pool: HostedProcessPool,
}

struct CapsuleBuildPaths {
    workspace: PathBuf,
    runtime_home: PathBuf,
}

impl CapsuleBuildPaths {
    fn new(storage_root: &Path, run_id: &RunId) -> Self {
        let run_root = run_directory(storage_root, run_id);
        Self {
            workspace: run_root.join("workspace"),
            runtime_home: run_root.join("runtime"),
        }
    }
}

struct RetainedAllocationPaths {
    source_root: PathBuf,
    run_root: PathBuf,
    source_recovery: PathBuf,
    run_recovery: PathBuf,
}

impl RetainedAllocationPaths {
    fn new(storage_root: &Path, source_run_id: &RunId, run_id: &RunId) -> Self {
        Self {
            source_root: run_directory(storage_root, source_run_id),
            run_root: run_directory(storage_root, run_id),
            source_recovery: recovery_path(storage_root, source_run_id),
            run_recovery: recovery_path(storage_root, run_id),
        }
    }
}

struct RetainedAllocationClaim {
    delivery_run_id: RunId,
    original_source: HostedRecoveryDocument,
}

struct CleanupRunRequest<'a> {
    run_root: &'a Path,
    checkpoint_directory: &'a Path,
    recovery_path: &'a Path,
    run_id: &'a RunId,
    delivery_run_id: &'a RunId,
    recovery_eligible: bool,
    exit: RunRuntimeExit,
}

pub(super) struct ProductionCapsuleAllocator {
    config: Arc<ProductionCapsuleConfig>,
    process_pools: ActiveRunProcessPools,
    active: Arc<Mutex<BTreeMap<RunId, Arc<ProductionCapsuleState>>>>,
    allocated: Mutex<BTreeSet<RunId>>,
    allocation_turn: Mutex<()>,
    prepare_filesystem: FilesystemPreparer,
    #[cfg(test)]
    source_override: Option<PathBuf>,
}

impl ProductionCapsuleAllocator {
    pub fn new(config: ProductionCapsuleConfig) -> Result<Self, ProductionHostingError> {
        if config.copilot_executable.as_os_str().is_empty()
            || config.codex_executable.as_os_str().is_empty()
            || config.claude_executable.is_empty()
            || config.git_program.as_os_str().is_empty()
            || config.gh_program.as_os_str().is_empty()
        {
            return Err(ProductionHostingError::CapsuleConfiguration);
        }
        let process_pools = ActiveRunProcessPools::new(config.process_pool)
            .map_err(|_| ProductionHostingError::CapsuleConfiguration)?;
        reconcile_retained_allocations(&config.storage_root)
            .map_err(|_| ProductionHostingError::CapsuleConfiguration)?;
        Ok(Self {
            config: Arc::new(config),
            process_pools,
            active: Arc::new(Mutex::new(BTreeMap::new())),
            allocated: Mutex::new(BTreeSet::new()),
            allocation_turn: Mutex::new(()),
            prepare_filesystem: production_filesystem,
            #[cfg(test)]
            source_override: None,
        })
    }

    async fn allocate_one(
        &self,
        run_id: &RunId,
        admitted: &AdmittedRun,
        github_token: Option<&str>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let allocation = self
            .build_capsule(CapsuleBuildRequest {
                run_id,
                delivery_run_id: run_id,
                adopt_existing_delivery: false,
                admitted,
                github_token,
                install_source: true,
                transfer_retained_workspace: false,
            })
            .await;
        if allocation.is_err() {
            // Failed cleanup deliberately retains the active state and lease. The controller
            // confirms destruction through allocator authority before recording a terminal error.
            let state = self.active.lock().await.get(run_id).cloned();
            if let Some(state) = state {
                let _ =
                    cleanup_state(run_id, &state, &self.active, RunRuntimeExit::Completed).await;
            }
        }
        allocation
    }

    async fn build_capsule(
        &self,
        request: CapsuleBuildRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let process_pool = self
            .process_pools
            .acquire()
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let active_process_pool = process_pool.process_pool();
        let git_identity = self.prepare_git_identity(active_process_pool)?;
        let (loss_sender, _) = watch::channel(false);
        let state = Arc::new(ProductionCapsuleState {
            endpoint: OnceLock::new(),
            run_root: run_directory(&self.config.storage_root, request.run_id),
            checkpoint_directory: checkpoint_directory(&self.config.storage_root, request.run_id),
            run_root_identity: OnceLock::new(),
            recovery_path: recovery_path(&self.config.storage_root, request.run_id),
            delivery_run_id: request.delivery_run_id.clone(),
            restored_delivery_run_id: OnceLock::new(),
            inherited_retained_workspace: request.transfer_retained_workspace,
            process_pool: Mutex::new(Some(process_pool)),
            #[cfg(test)]
            portable_processes: self.portable_test_processes(),
            _loss_sender: loss_sender,
            cleanup_turn: Mutex::new(false),
        });
        // Retain cleanup authority and the identity lease before checkout or retained-workspace
        // ownership transfer can start.
        let mut active = self.active.lock().await;
        if active.contains_key(request.run_id) {
            return Err(CapsuleAllocationUnavailable::Runtime);
        }
        active.insert(request.run_id.clone(), state.clone());
        drop(active);
        self.build_capsule_state(
            PendingCapsule {
                run_id: request.run_id,
                state,
                process_pool: active_process_pool,
                git_identity,
            },
            &request,
        )
        .await
    }

    async fn build_capsule_state(
        &self,
        pending: PendingCapsule<'_>,
        request: &CapsuleBuildRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let paths = CapsuleBuildPaths::new(&self.config.storage_root, pending.run_id);
        let filesystem = self.prepare_capsule_workspace(&pending, request, &paths)?;
        let PendingCapsule {
            run_id,
            state,
            process_pool: active_process_pool,
            git_identity,
        } = pending;
        let target = self
            .capsule_delivery_target(request, &filesystem, active_process_pool)
            .await?;
        let hosted_workspace = self
            .prepare_hosted_workspace(WorkspacePreparation {
                run_id,
                workspace: &filesystem.workspace,
                admitted: request.admitted,
                state: &state,
                process_pool: active_process_pool,
            })
            .await?;
        let delivery_run_id = state
            .restored_delivery_run_id
            .get()
            .unwrap_or(request.delivery_run_id);
        let github_config = GhCliAuthorityConfig {
            git_identity: Some(git_identity),
            git_program: self.config.git_program.clone(),
            gh_program: self.config.gh_program.clone(),
            ..GhCliAuthorityConfig::hosted(paths.runtime_home)
        };
        let candidate = build_native_v2_candidate(
            request.admitted,
            NativeV2CandidateConfig {
                harness: self.harness(request.admitted, &filesystem, active_process_pool)?,
                delivery: NativeV2DeliveryConfig::for_hosted_workspace(
                    DeliveryLineage::new(
                        delivery_run_id.clone(),
                        request.adopt_existing_delivery || delivery_run_id != run_id,
                    ),
                    filesystem.workspace.clone(),
                    target,
                    git_identity,
                ),
                github: Arc::new(
                    GhCliDeliveryAuthority::new(github_config).with_operator_diagnostics(
                        request.run_id.clone(),
                        self.config.operator_diagnostics.clone(),
                    ),
                ),
            },
        )
        .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let workspace_identity = WorkspaceIdentity::capture(&filesystem.workspace)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let endpoint = Arc::new(NativeCapsuleNodeEndpoint::new(Arc::new(candidate)));
        let runner = Arc::new(RemoteCapsuleNodeRunner::new(endpoint.clone()));
        state
            .endpoint
            .set(endpoint)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let loss = state._loss_sender.subscribe();
        monitor_workspace_identity(
            filesystem.workspace,
            workspace_identity,
            state._loss_sender.clone(),
        );
        let cleanup = Arc::new(ProductionCapsuleCleanup {
            run_id: run_id.clone(),
            state,
            active: Arc::downgrade(&self.active),
        });
        Ok(AllocatedCapsule {
            runner,
            cleanup,
            loss,
            checkpoints: Some(hosted_workspace.checkpoints),
            execution_seed: hosted_workspace.execution_seed,
        })
    }

    async fn prepare_hosted_workspace(
        &self,
        request: WorkspacePreparation<'_>,
    ) -> Result<HostedWorkspace, CapsuleAllocationUnavailable> {
        let directory = checkpoint_directory(&self.config.storage_root, request.run_id);
        let writers = writer_nodes(request.admitted);
        if let Some(storage) = &self.config.workspace_storage {
            let prepared = storage
                .prepare(HostedWorkspaceRequest {
                    run_id: request.run_id,
                    workspace: request.workspace,
                    checkpoint_directory: &directory,
                    writers,
                })
                .await
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            transfer_retained_workspace_to_writer(request.workspace, request.process_pool)?;
            if let Some(id) = &prepared.delivery_run_id {
                request
                    .state
                    .restored_delivery_run_id
                    .set(id.clone())
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            }
            return Ok(prepared);
        }
        Ok(HostedWorkspace {
            checkpoints: Arc::new(FilesystemCheckpointStore::new(
                directory,
                request.workspace.to_owned(),
                writers,
            )),
            execution_seed: Vec::new(),
            delivery_run_id: None,
        })
    }

    async fn build_retained_capsule(
        &self,
        request: CapsuleBuildRequest<'_>,
        source_run_id: &RunId,
        checkpoint_id: Option<&CheckpointId>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let execution_seed = match checkpoint_id {
            Some(checkpoint_id) => checkpoints::restore(
                &CheckpointRestore {
                    directory: checkpoint_directory(&self.config.storage_root, source_run_id),
                    checkpoint_id: checkpoint_id.clone(),
                },
                &CapsuleBuildPaths::new(&self.config.storage_root, request.run_id).workspace,
            )
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
            None => Vec::new(),
        };
        // Restore while the allocation lock owns the moved workspace, before a writer UID
        // receives it. There is no cancellable background restore that could outlive cleanup.
        let mut capsule = self.build_capsule(request).await?;
        capsule.execution_seed = execution_seed;
        Ok(capsule)
    }

    fn prepare_capsule_workspace(
        &self,
        pending: &PendingCapsule<'_>,
        request: &CapsuleBuildRequest<'_>,
        paths: &CapsuleBuildPaths,
    ) -> Result<CapsuleFilesystem, CapsuleAllocationUnavailable> {
        if request.install_source {
            pending.state.create_run_directory()?;
        } else {
            pending.state.capture_run_directory()?;
        }
        if request.transfer_retained_workspace {
            transfer_retained_workspace_to_writer(&paths.workspace, pending.process_pool)?;
        }
        (self.prepare_filesystem)(&paths.workspace, &paths.runtime_home, pending.process_pool)
    }

    async fn capsule_delivery_target(
        &self,
        request: &CapsuleBuildRequest<'_>,
        filesystem: &CapsuleFilesystem,
        process_pool: HostedProcessPool,
    ) -> Result<DeliveryTarget, CapsuleAllocationUnavailable> {
        if !request.install_source {
            return DeliveryTarget::new(
                request.admitted.source.repository.as_str(),
                request.admitted.source.branch.as_str(),
                request.admitted.source.revision.as_str(),
            )
            .map_err(|_| CapsuleAllocationUnavailable::Runtime);
        }
        let repository = request.admitted.source.repository.as_str();
        let source = self.repository_source(repository);
        install_repository(RepositoryInstall {
            git_program: &self.config.git_program,
            source: &source,
            resolved: &request.admitted.source,
            workspace: &filesystem.workspace,
            process_pool,
            github_token: request.github_token,
        })
        .await
        .map_err(|error| {
            error.record_diagnostic(request.run_id, &self.config.operator_diagnostics);
            CapsuleAllocationUnavailable::SourceCheckout
        })
    }

    fn validate_retained_checkpoint(
        &self,
        source_run_id: &RunId,
        checkpoint_id: Option<&CheckpointId>,
    ) -> Result<(), CapsuleAllocationUnavailable> {
        if let Some(checkpoint_id) = checkpoint_id {
            checkpoints::validate_selection(&CheckpointRestore {
                directory: checkpoint_directory(&self.config.storage_root, source_run_id),
                checkpoint_id: checkpoint_id.clone(),
            })
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        }
        Ok(())
    }

    fn claim_retained_allocation(
        &self,
        source_run_id: &RunId,
        run_id: &RunId,
        paths: &RetainedAllocationPaths,
    ) -> Result<RetainedAllocationClaim, RetainedAllocationUnavailable> {
        let mut source =
            read_recovery(&paths.source_recovery).ok_or(CapsuleAllocationUnavailable::Runtime)?;
        if !source.recoverable || source.successor_run_id.is_some() || paths.run_root.exists() {
            return Err(CapsuleAllocationUnavailable::Runtime.into());
        }
        let delivery_run_id =
            retained_delivery_run_id(&self.config.storage_root, source_run_id, &source)
                .ok_or(CapsuleAllocationUnavailable::Runtime)?;
        let original_source = HostedRecoveryDocument {
            recoverable: true,
            run_id: Some(source_run_id.clone()),
            delivery_run_id: Some(delivery_run_id.clone()),
            resumed_from: source.resumed_from.clone(),
            successor_run_id: None,
        };
        source.recoverable = false;
        source.run_id = Some(source_run_id.clone());
        source.delivery_run_id = Some(delivery_run_id.clone());
        source.successor_run_id = Some(run_id.clone());
        let successor = HostedRecoveryDocument {
            recoverable: false,
            run_id: Some(run_id.clone()),
            delivery_run_id: Some(delivery_run_id.clone()),
            resumed_from: Some(source_run_id.clone()),
            successor_run_id: None,
        };
        if write_recovery(&paths.source_recovery, &source)
            .and_then(|()| write_recovery(&paths.run_recovery, &successor))
            .is_err()
        {
            return Err(retained_claim_failure(paths, &original_source));
        }
        Ok(RetainedAllocationClaim {
            delivery_run_id,
            original_source,
        })
    }

    fn prepare_git_identity(
        &self,
        process_pool: HostedProcessPool,
    ) -> Result<HostedProcessIdentity, CapsuleAllocationUnavailable> {
        let identity = process_pool
            .identity(HostedProcessScope::Writer)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        #[cfg(test)]
        if self.portable_test_processes() {
            return Ok(identity);
        }
        // Never reuse a surviving identity after checkout failure or target restart.
        // No checkout credential may reach this UID until its domain is proven empty.
        identity
            .prepare_command_domain()
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        Ok(identity)
    }

    fn harness(
        &self,
        admitted: &AdmittedRun,
        filesystem: &CapsuleFilesystem,
        process_pool: HostedProcessPool,
    ) -> Result<NativeV2HarnessConfig, CapsuleAllocationUnavailable> {
        match &admitted.runtime {
            RuntimePlan::Copilot { .. } => Ok(NativeV2HarnessConfig::Copilot(
                crate::native_v2_copilot::CopilotConfig {
                    executable: self.config.copilot_executable.clone(),
                    workspace: filesystem.workspace.clone(),
                    runtime_home: filesystem.runtime_home.clone(),
                    search_path: self.config.executable_search_path.clone(),
                    process_pool,
                },
            )),
            RuntimePlan::Codex { provider, .. } => {
                Ok(NativeV2HarnessConfig::Codex(NativeV2CodexConfig {
                    provider: *provider,
                    executable: self.config.codex_executable.clone(),
                    workspace: filesystem.workspace.clone(),
                    runtime_home: filesystem.runtime_home.clone(),
                    local_user: None,
                    search_path: self.config.executable_search_path.clone(),
                    process_pool,
                }))
            }
            RuntimePlan::Claude { provider, .. } => {
                let base_environment = self
                    .config
                    .claude_process_environment
                    .for_capsule(
                        &filesystem.runtime_home,
                        &self.config.executable_search_path,
                    )
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
                Ok(NativeV2HarnessConfig::Claude(ClaudeAdapterConfig {
                    provider: *provider,
                    executable: self.config.claude_executable.clone(),
                    prefix_arguments: self.config.claude_prefix_arguments.clone(),
                    workspace: filesystem.workspace.clone(),
                    runtime_home: filesystem.runtime_home.clone(),
                    local_user_home: None,
                    base_environment,
                    process_pool,
                }))
            }
        }
    }

    #[cfg(test)]
    fn portable_test_processes(&self) -> bool {
        // Existing checkout tests replace containment with a caller-owned filesystem. Never
        // inspect or kill the test runner's UID; production has no identity-isolation opt-out.
        #[cfg(unix)]
        {
            self.source_override.is_some() && unsafe { libc::geteuid() } != 0
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    fn repository_source(&self, repository: &str) -> std::ffi::OsString {
        #[cfg(test)]
        if let Some(path) = &self.source_override {
            return super::repository::path_source(path);
        }
        production_source(repository)
    }
}

#[async_trait]
impl CapsuleAllocator for ProductionCapsuleAllocator {
    async fn claim_controller(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        let lock_path = controller_lock_path(&self.config.storage_root, run_id);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|_| ControllerClaimUnavailable)?;
        file.try_lock_exclusive()
            .map_err(|_| ControllerClaimUnavailable)?;
        Ok(Arc::new(ProductionControllerClaim { _file: file }))
    }

    async fn allocate(
        &self,
        run_id: &RunId,
        admitted: &AdmittedRun,
        github_token: Option<&str>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let _turn = self.allocation_turn.lock().await;
        if !self.allocated.lock().await.insert(run_id.clone()) {
            return Err(CapsuleAllocationUnavailable::Runtime);
        }
        self.allocate_one(run_id, admitted, github_token).await
    }

    async fn destroy_or_confirm_absent(
        &self,
        run_id: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        let _turn = self.allocation_turn.lock().await;
        let state = self.active.lock().await.get(run_id).cloned();
        if let Some(state) = state {
            cleanup_state(run_id, &state, &self.active, exit).await?;
        } else {
            let path = run_directory(&self.config.storage_root, run_id);
            // A failed attempt that never registered ownership cannot delete a directory that
            // predates it. Reconstructed runs still use the allocator's restart cleanup path.
            if self.allocated.lock().await.contains(run_id) {
                let absent = matches!(
                    std::fs::symlink_metadata(&path),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound
                );
                if !absent
                    && !confirmed_retained_workspace(
                        &path,
                        &recovery_path(&self.config.storage_root, run_id),
                        run_id,
                    )?
                {
                    return Err(CapsuleCleanupUnavailable);
                }
            } else {
                cleanup_run_directory(CleanupRunRequest {
                    run_root: &path,
                    checkpoint_directory: &checkpoint_directory(&self.config.storage_root, run_id),
                    recovery_path: &recovery_path(&self.config.storage_root, run_id),
                    run_id,
                    delivery_run_id: run_id,
                    recovery_eligible: true,
                    exit,
                })?;
            }
        }
        Ok(CapsuleDestroyed::confirmed())
    }

    async fn allocate_from_retained(
        &self,
        request: RetainedAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, RetainedAllocationUnavailable> {
        let RetainedAllocationRequest {
            checkpoint_id,
            source_run_id,
            run_id,
            admitted,
            github_token,
        } = request;
        let _turn = self.allocation_turn.lock().await;
        self.validate_retained_checkpoint(source_run_id, checkpoint_id)?;
        let paths = RetainedAllocationPaths::new(&self.config.storage_root, source_run_id, run_id);
        let claim = self.claim_retained_allocation(source_run_id, run_id, &paths)?;
        if std::fs::rename(&paths.source_root, &paths.run_root).is_err() {
            return Err(retained_claim_failure(&paths, &claim.original_source));
        }
        let allocation = self
            .build_retained_capsule(
                CapsuleBuildRequest {
                    run_id,
                    delivery_run_id: &claim.delivery_run_id,
                    adopt_existing_delivery: true,
                    admitted,
                    github_token,
                    install_source: false,
                    transfer_retained_workspace: true,
                },
                source_run_id,
                checkpoint_id,
            )
            .await;
        match allocation {
            Ok(capsule) => {
                self.allocated.lock().await.insert(run_id.clone());
                Ok(capsule)
            }
            Err(error) => {
                let state = self.active.lock().await.get(run_id).cloned();
                let cleanup = match state {
                    Some(state) => {
                        cleanup_state(run_id, &state, &self.active, RunRuntimeExit::RuntimeLost)
                            .await
                    }
                    None => cleanup_run_directory(CleanupRunRequest {
                        run_root: &paths.run_root,
                        checkpoint_directory: &checkpoint_directory(
                            &self.config.storage_root,
                            run_id,
                        ),
                        recovery_path: &paths.run_recovery,
                        run_id,
                        delivery_run_id: &claim.delivery_run_id,
                        recovery_eligible: true,
                        exit: RunRuntimeExit::RuntimeLost,
                    }),
                };
                match cleanup {
                    Ok(()) => Err(RetainedAllocationUnavailable::Settled(error)),
                    Err(cleanup) => Err(RetainedAllocationUnavailable::CleanupUnconfirmed(cleanup)),
                }
            }
        }
    }

    async fn checkpoints(
        &self,
        params: RunCheckpointsParams,
    ) -> Result<RunCheckpointsResult, CheckpointError> {
        checkpoints::list(
            &checkpoint_directory(&self.config.storage_root, &params.run_id),
            params,
        )
    }

    async fn workspace_recovery(
        &self,
        run_id: &RunId,
    ) -> openengine_cluster_protocol::WorkspaceRecovery {
        let document = read_recovery(&recovery_path(&self.config.storage_root, run_id));
        match document {
            Some(document) => openengine_cluster_protocol::WorkspaceRecovery {
                recoverable: document.recoverable
                    && run_directory(&self.config.storage_root, run_id)
                        .join("workspace")
                        .is_dir(),
                connection_requirements: Default::default(),
                resumed_from: document.resumed_from,
                successor_run_id: document.successor_run_id,
            },
            None => Default::default(),
        }
    }

    async fn discard_workspace(&self, run_id: &RunId) -> Result<bool, CapsuleCleanupUnavailable> {
        let _turn = self.allocation_turn.lock().await;
        let root = run_directory(&self.config.storage_root, run_id);
        let metadata_path = recovery_path(&self.config.storage_root, run_id);
        let Some(mut document) = read_recovery(&metadata_path) else {
            return Ok(false);
        };
        if !document.recoverable {
            return Ok(false);
        }
        remove_run_directory(&checkpoint_directory(&self.config.storage_root, run_id))?;
        remove_run_directory(&root)?;
        document.recoverable = false;
        write_recovery(&metadata_path, &document)?;
        Ok(true)
    }
}

struct ProductionControllerClaim {
    _file: File,
}

impl ExclusiveControllerClaim for ProductionControllerClaim {}

struct PendingCapsule<'a> {
    run_id: &'a RunId,
    state: Arc<ProductionCapsuleState>,
    process_pool: HostedProcessPool,
    git_identity: HostedProcessIdentity,
}

struct ProductionCapsuleState {
    endpoint: OnceLock<Arc<NativeCapsuleNodeEndpoint>>,
    run_root: PathBuf,
    checkpoint_directory: PathBuf,
    run_root_identity: OnceLock<WorkspaceIdentity>,
    recovery_path: PathBuf,
    delivery_run_id: RunId,
    restored_delivery_run_id: OnceLock<RunId>,
    inherited_retained_workspace: bool,
    // Retains the run's disjoint Linux identities until endpoint and workspace cleanup complete.
    process_pool: Mutex<Option<ActiveRunProcessPool>>,
    #[cfg(test)]
    portable_processes: bool,
    // Keeps the controller-side loss receiver live during intentional cleanup. A whole-host loss
    // is observed on restart through durable reconciliation, never by allocating a replacement.
    _loss_sender: watch::Sender<bool>,
    cleanup_turn: Mutex<bool>,
}

impl ProductionCapsuleState {
    fn create_run_directory(&self) -> Result<(), CapsuleAllocationUnavailable> {
        std::fs::create_dir(&self.run_root).map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        self.capture_run_directory()?;
        set_traversable_directory(&self.run_root)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        Ok(())
    }

    fn capture_run_directory(&self) -> Result<(), CapsuleAllocationUnavailable> {
        self.run_root_identity
            .set(
                WorkspaceIdentity::capture(&self.run_root)
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
            )
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        Ok(())
    }

    fn cleanup_owned_directory(
        &self,
        run_id: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<(), CapsuleCleanupUnavailable> {
        // A failed create or a replaced directory is not this allocation's disposable workspace.
        // Keep authority until that path is absent rather than deleting unrelated retained work.
        if self
            .run_root
            .try_exists()
            .map_err(|_| CapsuleCleanupUnavailable)?
            && !self
                .run_root_identity
                .get()
                .is_some_and(|identity| identity.is_current(&self.run_root))
        {
            return Err(CapsuleCleanupUnavailable);
        }
        cleanup_run_directory(CleanupRunRequest {
            run_root: &self.run_root,
            checkpoint_directory: &self.checkpoint_directory,
            recovery_path: &self.recovery_path,
            run_id,
            delivery_run_id: self
                .restored_delivery_run_id
                .get()
                .unwrap_or(&self.delivery_run_id),
            recovery_eligible: self.endpoint.get().is_some() || self.inherited_retained_workspace,
            exit,
        })
    }
}

pub(super) fn monitor_workspace_identity(
    workspace: PathBuf,
    identity: WorkspaceIdentity,
    loss: watch::Sender<bool>,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if !identity.is_current(&workspace) {
                loss.send_replace(true);
                return;
            }
        }
    });
}

struct ProductionCapsuleCleanup {
    run_id: RunId,
    state: Arc<ProductionCapsuleState>,
    active: Weak<Mutex<BTreeMap<RunId, Arc<ProductionCapsuleState>>>>,
}

#[async_trait]
impl CapsuleCleanup for ProductionCapsuleCleanup {
    async fn destroy_or_confirm_absent(
        &self,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        let Some(active) = self.active.upgrade() else {
            return Err(CapsuleCleanupUnavailable);
        };
        cleanup_state(&self.run_id, &self.state, &active, exit).await?;
        Ok(CapsuleDestroyed::confirmed())
    }
}

async fn cleanup_state(
    run_id: &RunId,
    state: &ProductionCapsuleState,
    active: &Mutex<BTreeMap<RunId, Arc<ProductionCapsuleState>>>,
    exit: RunRuntimeExit,
) -> Result<(), CapsuleCleanupUnavailable> {
    let mut cleaned = state.cleanup_turn.lock().await;
    if *cleaned {
        return Ok(());
    }
    if let Some(endpoint) = state.endpoint.get() {
        endpoint.disconnect().await;
    }
    let mut process_pool = state.process_pool.lock().await;
    let identity = process_pool
        .as_ref()
        .ok_or(CapsuleCleanupUnavailable)?
        .process_pool()
        .identity(HostedProcessScope::Writer)
        .map_err(|_| CapsuleCleanupUnavailable)?;
    // A failed delivery may still own credential-bearing helpers. Keep its workspace and
    // numeric identity lease until cleanup proves that a later run cannot inherit them.
    #[cfg(test)]
    let requires_cleanup = !state.portable_processes;
    #[cfg(not(test))]
    let requires_cleanup = true;
    if requires_cleanup && !identity.cleanup().await.proves_tree_empty() {
        return Err(CapsuleCleanupUnavailable);
    }
    state.cleanup_owned_directory(run_id, exit)?;
    active.lock().await.remove(run_id);
    process_pool.take();
    *cleaned = true;
    Ok(())
}

enum FailedRunDirectoryState {
    Absent,
    Disposable,
    Retained,
}

fn confirmed_retained_workspace(
    run_root: &Path,
    recovery_path: &Path,
    run_id: &RunId,
) -> Result<bool, CapsuleCleanupUnavailable> {
    let Some(document) = read_recovery(recovery_path) else {
        return Ok(false);
    };
    if !document.recoverable || document.run_id.as_ref() != Some(run_id) {
        return Ok(false);
    }
    Ok(matches!(
        failed_run_directory_state(run_root)?,
        FailedRunDirectoryState::Retained
    ))
}

fn cleanup_run_directory(request: CleanupRunRequest<'_>) -> Result<(), CapsuleCleanupUnavailable> {
    if !request.recovery_eligible
        || !matches!(
            request.exit,
            RunRuntimeExit::Failed | RunRuntimeExit::RuntimeLost
        )
    {
        return dispose_run_directory(request);
    }
    match failed_run_directory_state(request.run_root)? {
        FailedRunDirectoryState::Absent | FailedRunDirectoryState::Disposable => {
            dispose_run_directory(request)
        }
        FailedRunDirectoryState::Retained => retain_failed_workspace(request),
    }
}

fn dispose_run_directory(request: CleanupRunRequest<'_>) -> Result<(), CapsuleCleanupUnavailable> {
    remove_run_directory(request.run_root)?;
    let ancestor = read_recovery(request.recovery_path)
        .is_some_and(|recovery| recovery.successor_run_id.is_some());
    if !ancestor {
        remove_run_directory(request.checkpoint_directory)?;
    }
    Ok(())
}

fn failed_run_directory_state(
    run_root: &Path,
) -> Result<FailedRunDirectoryState, CapsuleCleanupUnavailable> {
    let metadata = match std::fs::symlink_metadata(run_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FailedRunDirectoryState::Absent);
        }
        Err(_) => return Err(CapsuleCleanupUnavailable),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CapsuleCleanupUnavailable);
    }
    let workspace = std::fs::symlink_metadata(run_root.join("workspace"));
    if matches!(
        workspace,
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink()
    ) {
        Ok(FailedRunDirectoryState::Retained)
    } else {
        Ok(FailedRunDirectoryState::Disposable)
    }
}

fn retain_failed_workspace(
    request: CleanupRunRequest<'_>,
) -> Result<(), CapsuleCleanupUnavailable> {
    let runtime = request.run_root.join("runtime");
    if runtime.exists() {
        std::fs::remove_dir_all(runtime).map_err(|_| CapsuleCleanupUnavailable)?;
    }
    transfer_workspace_ownership(&request.run_root.join("workspace"), 0, 0)
        .map_err(|_| CapsuleCleanupUnavailable)?;
    let mut document = read_recovery(request.recovery_path).unwrap_or_default();
    document.recoverable = true;
    document
        .run_id
        .get_or_insert_with(|| request.run_id.clone());
    document
        .delivery_run_id
        .get_or_insert_with(|| request.delivery_run_id.clone());
    write_recovery(request.recovery_path, &document)
}

fn transfer_retained_workspace_to_writer(
    workspace: &Path,
    process_pool: HostedProcessPool,
) -> Result<(), CapsuleAllocationUnavailable> {
    let writer = process_pool
        .identity(HostedProcessScope::Writer)
        .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
    if transfer_workspace_ownership(workspace, writer.uid(), writer.gid()).is_err() {
        let _ = transfer_workspace_ownership(workspace, 0, 0);
        return Err(CapsuleAllocationUnavailable::Runtime);
    }
    Ok(())
}

fn rollback_retained_claim(
    paths: &RetainedAllocationPaths,
    source: &HostedRecoveryDocument,
) -> Result<(), CapsuleCleanupUnavailable> {
    let source_result = write_recovery(&paths.source_recovery, source);
    let successor_result = remove_recovery_file(&paths.run_recovery);
    source_result.and(successor_result)
}

fn retained_claim_failure(
    paths: &RetainedAllocationPaths,
    source: &HostedRecoveryDocument,
) -> RetainedAllocationUnavailable {
    match rollback_retained_claim(paths, source) {
        Ok(()) => RetainedAllocationUnavailable::Settled(CapsuleAllocationUnavailable::Runtime),
        Err(error) => RetainedAllocationUnavailable::CleanupUnconfirmed(error),
    }
}

fn transfer_workspace_ownership(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        if unsafe { libc::geteuid() } != 0 {
            return Ok(());
        }
        transfer_entry_ownership(path, uid, gid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, uid, gid);
        Err(std::io::Error::other(
            "hosted workspace ownership transfer requires Linux",
        ))
    }
}

#[cfg(target_os = "linux")]
fn transfer_entry_ownership(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        for entry in std::fs::read_dir(path)? {
            transfer_entry_ownership(&entry?.path(), uid, gid)?;
        }
    }
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
}

fn recovery_path(root: &Path, run_id: &RunId) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(b"zeroshot/native-v2/workspace-recovery/v1\0");
    digest.update(run_id.as_str().as_bytes());
    root.join(RECOVERY_DIRECTORY)
        .join(format!("{:x}.json", digest.finalize()))
}

fn read_recovery(path: &Path) -> Option<HostedRecoveryDocument> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_recovery(
    path: &Path,
    document: &HostedRecoveryDocument,
) -> Result<(), CapsuleCleanupUnavailable> {
    let parent = path.parent().ok_or(CapsuleCleanupUnavailable)?;
    std::fs::create_dir_all(parent).map_err(|_| CapsuleCleanupUnavailable)?;
    let bytes = serde_json::to_vec(document).map_err(|_| CapsuleCleanupUnavailable)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes).map_err(|_| CapsuleCleanupUnavailable)?;
    std::fs::rename(temporary, path).map_err(|_| CapsuleCleanupUnavailable)
}

fn remove_recovery_file(path: &Path) -> Result<(), CapsuleCleanupUnavailable> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(CapsuleCleanupUnavailable),
    }
}

fn reconcile_retained_allocations(root: &Path) -> Result<(), CapsuleCleanupUnavailable> {
    let recovery_root = root.join(RECOVERY_DIRECTORY);
    let entries = match std::fs::read_dir(&recovery_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(CapsuleCleanupUnavailable),
    };
    for entry in entries {
        let path = entry.map_err(|_| CapsuleCleanupUnavailable)?.path();
        reconcile_retained_document(root, &path)?;
    }
    Ok(())
}

fn reconcile_retained_document(
    root: &Path,
    source_path: &Path,
) -> Result<(), CapsuleCleanupUnavailable> {
    let Some(source) = read_recovery(source_path) else {
        return Ok(());
    };
    let (Some(source_run_id), Some(successor_run_id)) =
        (source.run_id.clone(), source.successor_run_id.clone())
    else {
        return Ok(());
    };
    let source_root = run_directory(root, &source_run_id);
    let successor_root = run_directory(root, &successor_run_id);
    let successor_path = recovery_path(root, &successor_run_id);
    match (source_root.exists(), successor_root.exists()) {
        (true, true) => Err(CapsuleCleanupUnavailable),
        (true, false) => {
            rollback_reconciled_claim(source_path, &successor_path, source, &source_root)
        }
        (false, true) => {
            complete_reconciled_handoff(&successor_path, source, source_run_id, successor_run_id)
        }
        (false, false) => Ok(()),
    }
}

fn rollback_reconciled_claim(
    source_path: &Path,
    successor_path: &Path,
    mut source: HostedRecoveryDocument,
    source_root: &Path,
) -> Result<(), CapsuleCleanupUnavailable> {
    source.recoverable = source_root.join("workspace").is_dir();
    source.successor_run_id = None;
    write_recovery(source_path, &source)?;
    remove_recovery_file(successor_path)
}

fn complete_reconciled_handoff(
    successor_path: &Path,
    source: HostedRecoveryDocument,
    source_run_id: RunId,
    successor_run_id: RunId,
) -> Result<(), CapsuleCleanupUnavailable> {
    let mut successor = read_recovery(successor_path).unwrap_or_default();
    if successor
        .resumed_from
        .as_ref()
        .is_some_and(|resumed_from| resumed_from != &source_run_id)
    {
        return Err(CapsuleCleanupUnavailable);
    }
    successor.run_id = Some(successor_run_id);
    successor.delivery_run_id = source.delivery_run_id;
    successor.resumed_from = Some(source_run_id);
    write_recovery(successor_path, &successor)
}

fn retained_delivery_run_id(
    root: &Path,
    run_id: &RunId,
    document: &HostedRecoveryDocument,
) -> Option<RunId> {
    let mut lineage_run_id = run_id.clone();
    let mut lineage = document.clone();
    for _ in 0..MAX_RECOVERY_LINEAGE_DEPTH {
        if let Some(delivery_run_id) = lineage.delivery_run_id {
            return Some(delivery_run_id);
        }
        let Some(predecessor) = lineage.resumed_from else {
            return Some(lineage_run_id);
        };
        lineage_run_id = predecessor;
        let Some(predecessor_document) = read_recovery(&recovery_path(root, &lineage_run_id))
        else {
            return Some(lineage_run_id);
        };
        lineage = predecessor_document;
    }
    None
}

pub(super) fn production_filesystem(
    workspace: &Path,
    runtime_home: &Path,
    process_pool: HostedProcessPool,
) -> Result<CapsuleFilesystem, CapsuleAllocationUnavailable> {
    prepare_capsule_filesystem(CapsuleFilesystemSpec {
        workspace,
        runtime_home,
        process_pool,
    })
    .map_err(|_| CapsuleAllocationUnavailable::Runtime)
}

fn run_directory(root: &Path, run_id: &RunId) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(b"zeroshot/native-v2/workspace/v1\0");
    digest.update(run_id.as_str().as_bytes());
    root.join("runs").join(format!("{:x}", digest.finalize()))
}

fn checkpoint_directory(root: &Path, run_id: &RunId) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(b"zeroshot/native-v2/workspace-checkpoints/v1\0");
    digest.update(run_id.as_str().as_bytes());
    root.join("checkpoints")
        .join(format!("{:x}", digest.finalize()))
}

fn controller_lock_path(root: &Path, run_id: &RunId) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(b"zeroshot/native-v2/controller-lease/v1\0");
    digest.update(run_id.as_str().as_bytes());
    root.join(format!("controller-{:x}.lock", digest.finalize()))
}

fn remove_run_directory(path: &Path) -> Result<(), CapsuleCleanupUnavailable> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(CapsuleCleanupUnavailable),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CapsuleCleanupUnavailable);
    }
    std::fs::remove_dir_all(path).map_err(|_| CapsuleCleanupUnavailable)
}

#[cfg(all(test, unix))]
impl ProductionCapsuleAllocator {
    pub(super) fn with_test_filesystem_and_source(
        mut self,
        source: PathBuf,
        prepare: FilesystemPreparer,
    ) -> Self {
        self.source_override = Some(source);
        self.prepare_filesystem = prepare;
        self
    }

    pub(super) fn run_path(&self, run_id: &RunId) -> PathBuf {
        run_directory(&self.config.storage_root, run_id)
    }

    pub(super) fn recovery_delivery_run_id(&self, run_id: &RunId) -> Option<RunId> {
        read_recovery(&recovery_path(&self.config.storage_root, run_id))?.delivery_run_id
    }

    pub(super) fn set_test_filesystem(&mut self, prepare: FilesystemPreparer) {
        self.prepare_filesystem = prepare;
    }

    pub(super) fn write_test_recovery(
        &self,
        run_id: &RunId,
        state: (bool, Option<RunId>, Option<RunId>),
    ) {
        let (recoverable, resumed_from, successor_run_id) = state;
        write_recovery(
            &recovery_path(&self.config.storage_root, run_id),
            &HostedRecoveryDocument {
                recoverable,
                run_id: Some(run_id.clone()),
                delivery_run_id: Some(run_id.clone()),
                resumed_from,
                successor_run_id,
            },
        )
        .expect("test recovery metadata should be writable");
    }

    pub(super) fn reconcile_test_recovery(&self) {
        reconcile_retained_allocations(&self.config.storage_root)
            .expect("test recovery metadata should reconcile");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod cleanup_tests;

#[cfg(all(test, target_os = "linux"))]
mod allocation_tests;
