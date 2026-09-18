mod identity_leases;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use fs2::FileExt;
use openengine_cluster_protocol::RunId;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, watch};

use crate::execution::process::{HostedProcessPool, HostedProcessScope};
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
    ExclusiveControllerClaim, RetainedAllocationRequest,
};
use crate::native_v2_codex::NativeV2CodexConfig;
use crate::native_v2_contract::{AdmittedRun, RuntimePlan};
use crate::native_v2_delivery::{GhCliAuthorityConfig, GhCliDeliveryAuthority, NativeV2DeliveryConfig};
use crate::native_v2_delivery::DeliveryTarget;
use crate::native_v2_portable_controller::WorkspaceIdentity;
use crate::native_v2_supervisor::RunRuntimeExit;
use crate::native_v2_target_authority::OperatorDiagnosticStore;
use serde::{Deserialize, Serialize};

use super::{ProductionHostingError, set_traversable_directory};
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

struct RetainedAllocationRollback<'a> {
    source_root: &'a Path,
    run_root: &'a Path,
    source_path: &'a Path,
    run_recovery_path: &'a Path,
    source: &'a HostedRecoveryDocument,
}

struct CleanupRunRequest<'a> {
    run_root: &'a Path,
    recovery_path: &'a Path,
    run_id: &'a RunId,
    delivery_run_id: &'a RunId,
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
        let run_root = run_directory(&self.config.storage_root, run_id);
        std::fs::create_dir(&run_root).map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        set_traversable_directory(&run_root).map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
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
            let _ = remove_run_directory(&run_root);
        }
        allocation
    }

    async fn build_capsule(
        &self,
        request: CapsuleBuildRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let CapsuleBuildRequest {
            run_id,
            delivery_run_id,
            adopt_existing_delivery,
            admitted,
            github_token,
            install_source,
            transfer_retained_workspace,
        } = request;
        let process_pool = self
            .process_pools
            .acquire()
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let active_process_pool = process_pool.process_pool();
        let run_root = run_directory(&self.config.storage_root, run_id);
        let workspace = run_root.join("workspace");
        let runtime_home = run_root.join("runtime");
        if transfer_retained_workspace {
            let writer = active_process_pool
                .identity(HostedProcessScope::Writer)
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            if transfer_workspace_ownership(&workspace, writer.uid(), writer.gid()).is_err() {
                let _ = transfer_workspace_ownership(&workspace, 0, 0);
                return Err(CapsuleAllocationUnavailable::Runtime);
            }
        }
        let allocation = async {
            let filesystem =
                (self.prepare_filesystem)(&workspace, &runtime_home, active_process_pool)?;
            let target = if install_source {
                let repository = admitted.source.repository.as_str();
                let source = self.repository_source(repository);
                install_repository(RepositoryInstall {
                    git_program: &self.config.git_program,
                    source: &source,
                    resolved: &admitted.source,
                    workspace: &filesystem.workspace,
                    process_pool: active_process_pool,
                    github_token,
                })
                .await
                .map_err(|error| {
                    error.record_diagnostic(run_id, &self.config.operator_diagnostics);
                    CapsuleAllocationUnavailable::SourceCheckout
                })?
            } else {
                DeliveryTarget::new(
                    admitted.source.repository.as_str(),
                    admitted.source.branch.as_str(),
                    admitted.source.revision.as_str(),
                )
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?
            };
            let github_config = GhCliAuthorityConfig {
                git_program: self.config.git_program.clone(),
                gh_program: self.config.gh_program.clone(),
                ..GhCliAuthorityConfig::hosted(runtime_home)
            };
            let candidate = build_native_v2_candidate(
                admitted,
                NativeV2CandidateConfig {
                    harness: self.harness(admitted, &filesystem, active_process_pool)?,
                    delivery: NativeV2DeliveryConfig::for_hosted_workspace(
                        delivery_run_id.clone(),
                        adopt_existing_delivery,
                        filesystem.workspace.clone(),
                        target,
                    ),
                    github: Arc::new(
                        GhCliDeliveryAuthority::new(github_config).with_operator_diagnostics(
                            run_id.clone(),
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
            let (loss_sender, loss) = watch::channel(false);
            let state = Arc::new(ProductionCapsuleState {
                endpoint,
                run_root,
                recovery_path: recovery_path(&self.config.storage_root, run_id),
                delivery_run_id: delivery_run_id.clone(),
                process_pool: Mutex::new(Some(process_pool)),
                _loss_sender: loss_sender.clone(),
                cleanup_turn: Mutex::new(false),
            });
            let replaced = self
                .active
                .lock()
                .await
                .insert(run_id.clone(), state.clone());
            if replaced.is_some() {
                return Err(CapsuleAllocationUnavailable::Runtime);
            }
            monitor_workspace_identity(filesystem.workspace, workspace_identity, loss_sender);
            let cleanup = Arc::new(ProductionCapsuleCleanup {
                run_id: run_id.clone(),
                state,
                active: Arc::downgrade(&self.active),
            });

            Ok(AllocatedCapsule {
                runner,
                cleanup,
                loss,
            })
        }
        .await;
        if allocation.is_err() && transfer_retained_workspace {
            transfer_workspace_ownership(&workspace, 0, 0)
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        }
        allocation
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
        if let Some(state) = self.active.lock().await.get(run_id).cloned() {
            cleanup_state(run_id, &state, &self.active, exit).await?;
        } else {
            cleanup_run_directory(CleanupRunRequest {
                run_root: &run_directory(&self.config.storage_root, run_id),
                recovery_path: &recovery_path(&self.config.storage_root, run_id),
                run_id,
                delivery_run_id: run_id,
                exit,
            })?;
        }
        Ok(CapsuleDestroyed::confirmed())
    }

    async fn allocate_from_retained(
        &self,
        request: RetainedAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let RetainedAllocationRequest {
            source_run_id,
            run_id,
            admitted,
            github_token,
        } = request;
        let _turn = self.allocation_turn.lock().await;
        let source_root = run_directory(&self.config.storage_root, source_run_id);
        let run_root = run_directory(&self.config.storage_root, run_id);
        let source_path = recovery_path(&self.config.storage_root, source_run_id);
        let run_recovery_path = recovery_path(&self.config.storage_root, run_id);
        let mut source =
            read_recovery(&source_path).ok_or(CapsuleAllocationUnavailable::Runtime)?;
        if !source.recoverable || source.successor_run_id.is_some() || run_root.exists() {
            return Err(CapsuleAllocationUnavailable::Runtime);
        }
        source.recoverable = false;
        source.run_id = Some(source_run_id.clone());
        let delivery_run_id =
            retained_delivery_run_id(&self.config.storage_root, source_run_id, &source)
                .ok_or(CapsuleAllocationUnavailable::Runtime)?;
        source.delivery_run_id = Some(delivery_run_id.clone());
        source.successor_run_id = Some(run_id.clone());
        let original_source = HostedRecoveryDocument {
            recoverable: true,
            run_id: Some(source_run_id.clone()),
            delivery_run_id: Some(delivery_run_id.clone()),
            resumed_from: source.resumed_from.clone(),
            successor_run_id: None,
        };
        let claim_result = write_recovery(&source_path, &source).and_then(|()| {
            write_recovery(
                &run_recovery_path,
                &HostedRecoveryDocument {
                    recoverable: false,
                    run_id: Some(run_id.clone()),
                    delivery_run_id: Some(delivery_run_id.clone()),
                    resumed_from: Some(source_run_id.clone()),
                    successor_run_id: None,
                },
            )
        });
        if claim_result.is_err() {
            let _ = write_recovery(&source_path, &original_source);
            let _ = remove_recovery_file(&run_recovery_path);
            return Err(CapsuleAllocationUnavailable::Runtime);
        }
        if std::fs::rename(&source_root, &run_root).is_err() {
            let _ = write_recovery(&source_path, &original_source);
            let _ = remove_recovery_file(&run_recovery_path);
            return Err(CapsuleAllocationUnavailable::Runtime);
        }
        let allocation = self
            .build_capsule(CapsuleBuildRequest {
                run_id,
                delivery_run_id: &delivery_run_id,
                adopt_existing_delivery: true,
                admitted,
                github_token,
                install_source: false,
                transfer_retained_workspace: true,
            })
            .await;
        if allocation.is_err() {
            let _ = restore_retained_allocation(RetainedAllocationRollback {
                source_root: &source_root,
                run_root: &run_root,
                source_path: &source_path,
                run_recovery_path: &run_recovery_path,
                source: &original_source,
            });
            return allocation;
        }
        self.allocated.lock().await.insert(run_id.clone());
        allocation
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

struct ProductionCapsuleState {
    endpoint: Arc<NativeCapsuleNodeEndpoint>,
    run_root: PathBuf,
    recovery_path: PathBuf,
    delivery_run_id: RunId,
    // Retains the run's disjoint Linux identities until endpoint and workspace cleanup complete.
    process_pool: Mutex<Option<ActiveRunProcessPool>>,
    // Keeps the controller-side loss receiver live during intentional cleanup. A whole-host loss
    // is observed on restart through durable reconciliation, never by allocating a replacement.
    _loss_sender: watch::Sender<bool>,
    cleanup_turn: Mutex<bool>,
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
    state.endpoint.disconnect().await;
    cleanup_run_directory(CleanupRunRequest {
        run_root: &state.run_root,
        recovery_path: &state.recovery_path,
        run_id,
        delivery_run_id: &state.delivery_run_id,
        exit,
    })?;
    active.lock().await.remove(run_id);
    state.process_pool.lock().await.take();
    *cleaned = true;
    Ok(())
}

fn cleanup_run_directory(request: CleanupRunRequest<'_>) -> Result<(), CapsuleCleanupUnavailable> {
    let CleanupRunRequest {
        run_root,
        recovery_path,
        run_id,
        delivery_run_id,
        exit,
    } = request;
    if !matches!(exit, RunRuntimeExit::Failed | RunRuntimeExit::RuntimeLost) {
        return remove_run_directory(run_root);
    }
    let metadata = match std::fs::symlink_metadata(run_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(CapsuleCleanupUnavailable),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CapsuleCleanupUnavailable);
    }
    let workspace_metadata = std::fs::symlink_metadata(run_root.join("workspace"));
    if !matches!(
        workspace_metadata,
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink()
    ) {
        return remove_run_directory(run_root);
    }
    let runtime = run_root.join("runtime");
    if runtime.exists() {
        std::fs::remove_dir_all(runtime).map_err(|_| CapsuleCleanupUnavailable)?;
    }
    transfer_workspace_ownership(&run_root.join("workspace"), 0, 0)
        .map_err(|_| CapsuleCleanupUnavailable)?;
    let mut document = read_recovery(recovery_path).unwrap_or_default();
    document.recoverable = true;
    document.run_id.get_or_insert_with(|| run_id.clone());
    document
        .delivery_run_id
        .get_or_insert_with(|| delivery_run_id.clone());
    write_recovery(recovery_path, &document)
}

fn restore_retained_allocation(
    rollback: RetainedAllocationRollback<'_>,
) -> Result<(), CapsuleCleanupUnavailable> {
    let RetainedAllocationRollback {
        source_root,
        run_root,
        source_path,
        run_recovery_path,
        source,
    } = rollback;
    let runtime = run_root.join("runtime");
    if runtime.exists() {
        std::fs::remove_dir_all(runtime).map_err(|_| CapsuleCleanupUnavailable)?;
    }
    transfer_workspace_ownership(&run_root.join("workspace"), 0, 0)
        .map_err(|_| CapsuleCleanupUnavailable)?;
    if source_root.exists() {
        remove_run_directory(source_root)?;
    }
    std::fs::rename(run_root, source_root).map_err(|_| CapsuleCleanupUnavailable)?;
    write_recovery(source_path, source)?;
    match std::fs::remove_file(run_recovery_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(CapsuleCleanupUnavailable),
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
        let Some(mut source) = read_recovery(&path) else {
            continue;
        };
        let (Some(source_run_id), Some(successor_run_id)) =
            (source.run_id.clone(), source.successor_run_id.clone())
        else {
            continue;
        };
        let source_root = run_directory(root, &source_run_id);
        let successor_root = run_directory(root, &successor_run_id);
        let successor_path = recovery_path(root, &successor_run_id);
        if source_root.exists() {
            if successor_root.exists() {
                return Err(CapsuleCleanupUnavailable);
            }
            source.recoverable = source_root.join("workspace").is_dir();
            source.successor_run_id = None;
            write_recovery(&path, &source)?;
            remove_recovery_file(&successor_path)?;
        } else if successor_root.exists() {
            let mut successor = read_recovery(&successor_path).unwrap_or_default();
            if successor
                .resumed_from
                .as_ref()
                .is_some_and(|resumed_from| resumed_from != &source_run_id)
            {
                return Err(CapsuleCleanupUnavailable);
            }
            successor.run_id = Some(successor_run_id);
            successor.delivery_run_id = source.delivery_run_id.clone();
            successor.resumed_from = Some(source_run_id);
            write_recovery(&successor_path, &successor)?;
        }
    }
    Ok(())
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

fn production_filesystem(
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
