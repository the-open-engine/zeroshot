//! Production composition for one native-v2 target.
//!
//! The target authority owns one SQLite ledger and one controller. Each admitted run receives one
//! disposable directory containing its repository checkout and provider-private runtime homes.
//! The allocator constructs only the existing native candidate and private capsule boundary; it
//! has no retry, credential-store, or Node compatibility path.

mod allocator;
mod connections;
mod environment;
mod repository;
mod workspaces;

pub use workspaces::{HostedWorkspace, HostedWorkspaceRequest, HostedWorkspaceStorage};

#[cfg(test)]
mod tests;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{NativeV2RunValueError, RunSubmitParams, RuntimePlan};
use thiserror::Error;

use crate::execution::process::HostedProcessPool;
use crate::native_v2_admission::DeliveryPolicy;
use crate::native_v2_candidate::{ProviderAccessPlacement, materialize_provider_access};
use crate::native_v2_claude::ClaudeProcessEnvironment;
use crate::native_v2_cloud::{NativeV2CloudController, NativeV2CloudError};
use crate::native_v2_pi::PiProcessEnvironment;
use crate::native_v2_cloud::submission_digest;
use crate::native_v2_target_authority::{
    NativeV2TargetAuthority, OperatorDiagnosticOutput, OperatorDiagnosticStore,
    TargetAuthorityError, TargetControllerFactory, TargetRunReceipt, TargetRunRequest,
};
use crate::v2_run_ledger::RunLedger;
use crate::v2_run_ledger::RunLedgerError;
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use crate::native_v2_supervisor::RunEnvironment;

use allocator::ProductionCapsuleAllocator;
pub(crate) use connections::build_connection_resolver;

/// Adds missing provider connection requirements for each agent's effective lane.
///
/// Hosts call this before storing remote profiles or resolving run credentials.
/// Compatible authored connection fields are preserved; repeated calls are idempotent.
/// This updates declarations only and never reads or resolves credential values.
pub fn materialize_contained_provider_access(
    runtime: &mut RuntimePlan,
) -> Result<(), NativeV2RunValueError> {
    materialize_provider_access(runtime, ProviderAccessPlacement::Contained)
}

/// Composes the production target around exact sourceful run requests.
pub async fn build_production_target_authority(
    config: ProductionHostingConfig,
) -> Result<NativeV2TargetAuthority, ProductionHostingError> {
    prepare_storage_root(&config.storage_root)?;
    let factory = Arc::new(ProductionTargetControllerFactory::new(config));
    Ok(NativeV2TargetAuthority::new_with_operator_diagnostics(
        factory.clone(),
        factory.operator_diagnostics.clone(),
    ))
}

/// Host-owned non-secret capabilities used to compose one installed target.
#[derive(Clone)]
pub struct ProductionHostingConfig {
    /// A host-owned namespace. Cloud embedders select their own tenant-scoped storage.
    pub memory_storage: Option<PathBuf>,
    /// Private diagnostics collection; the host owns the receiver and durable storage.
    pub operator_diagnostic_output: Option<OperatorDiagnosticOutput>,
    pub workspace_storage: Option<Arc<dyn HostedWorkspaceStorage>>,
    pub storage_root: PathBuf,
    pub copilot_executable: PathBuf,
    pub codex_executable: PathBuf,
    pub claude_executable: String,
    pub claude_prefix_arguments: Vec<String>,
    pub claude_process_environment: ClaudeProcessEnvironment,
    pub pi_executable: String,
    pub pi_prefix_arguments: Vec<String>,
    pub pi_process_environment: PiProcessEnvironment,
    pub executable_search_path: String,
    pub git_program: PathBuf,
    pub gh_program: PathBuf,
    pub process_pool: HostedProcessPool,
}

impl fmt::Debug for ProductionHostingConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductionHostingConfig")
            .field("storage_root", &self.storage_root)
            .field("codex_executable", &self.codex_executable)
            .field("claude_executable", &self.claude_executable)
            .field("git_program", &self.git_program)
            .field("gh_program", &self.gh_program)
            .finish_non_exhaustive()
    }
}

/// Factory installed behind [`crate::native_v2_target_authority::NativeV2TargetAuthority`].
#[derive(Clone, Debug)]
pub struct ProductionTargetControllerFactory {
    config: Arc<ProductionHostingConfig>,
    operator_diagnostics: Arc<OperatorDiagnosticStore>,
}

impl ProductionTargetControllerFactory {
    #[must_use]
    pub fn new(config: ProductionHostingConfig) -> Self {
        let operator_diagnostics = Arc::new(OperatorDiagnosticStore::new(
            config.operator_diagnostic_output.clone(),
        ));
        Self {
            config: Arc::new(config),
            operator_diagnostics,
        }
    }

    pub async fn create_controller(
        &self,
    ) -> Result<Arc<NativeV2CloudController>, ProductionHostingError> {
        let root = prepare_storage_root(&self.config.storage_root)?;
        let ledger_path = root.join("runs.sqlite3");
        prepare_production_ledger(&ledger_path)?;
        let ledger: Arc<dyn RunLedger> = Arc::new(
            SqliteRunLedger::open(ledger_path).map_err(|_| ProductionHostingError::Ledger)?,
        );
        let mut capsule_config = self.config.as_ref().clone();
        capsule_config.storage_root = root;
        let allocator = Arc::new(ProductionCapsuleAllocator::new(
            Arc::new(capsule_config),
            self.operator_diagnostics.clone(),
        )?);
        let controller = NativeV2CloudController::new_with_delivery_policy(
            ledger,
            allocator,
            DeliveryPolicy::Optional,
        )
        .await
        .map_err(|_| ProductionHostingError::Controller)?
        .with_operator_diagnostics(self.operator_diagnostics.clone());
        Ok(Arc::new(controller))
    }
}

#[async_trait]
impl TargetControllerFactory for ProductionTargetControllerFactory {
    fn supports_workspace_recovery(&self) -> bool {
        true
    }

    fn supports_workspace_checkpoints(&self) -> bool {
        true
    }

    fn supports_git_transport(&self) -> bool {
        true
    }

    async fn create(&self) -> Result<Arc<NativeV2CloudController>, TargetAuthorityError> {
        self.create_controller()
            .await
            .map_err(|error| TargetAuthorityError::unavailable(error.to_string()))
    }

    async fn submit(
        &self,
        controller: &NativeV2CloudController,
        request: TargetRunRequest,
    ) -> Result<TargetRunReceipt, TargetAuthorityError> {
        let TargetRunRequest {
            run_id,
            mut submission,
            connections,
            connection_resolver,
            github_token,
            git_credentials,
        } = request;
        prepare_contained_submission(&mut submission)?;
        let digest = submission_digest(&submission)
            .map_err(|error| TargetAuthorityError::invalid(error.to_string()))?;
        if let Some(receipt) = controller
            .resolve_submission(&submission.submission_key, &digest)
            .await
            .map_err(project_cloud_error)?
        {
            return Ok(TargetRunReceipt {
                run_id: receipt.run_id,
            });
        }
        let environment = match connection_resolver {
            Some(wire) => {
                let resolution = build_connection_resolver(run_id.clone(), wire)?;
                RunEnvironment::with_resolver_for_submission(&submission, connections, resolution)
            }
            None => RunEnvironment::exact_for_submission(&submission, connections),
        }
        .map_err(|error| TargetAuthorityError::invalid(error.to_string()))?;
        let receipt = controller
            .submit_with_exact_environment_and_source_credentials(
                RunSubmitParams { run_id, submission },
                environment,
                crate::native_v2_cloud::RunSourceCredentials {
                    github_token,
                    git_credentials,
                },
            )
            .await
            .map_err(project_cloud_error)?;
        Ok(TargetRunReceipt {
            run_id: receipt.run_id,
        })
    }
}

fn prepare_contained_submission(
    submission: &mut openengine_cluster_protocol::RunSubmission,
) -> Result<(), TargetAuthorityError> {
    if submission.source.is_none() {
        return Err(TargetAuthorityError::invalid(
            "contained runs require a resolved Git source",
        ));
    }
    materialize_contained_provider_access(&mut submission.runtime)
        .map_err(|error| TargetAuthorityError::invalid(error.to_string()))
}

fn project_cloud_error(error: NativeV2CloudError) -> TargetAuthorityError {
    let message = error.to_string();
    match error {
        NativeV2CloudError::Admission(_)
        | NativeV2CloudError::Environment(_)
        | NativeV2CloudError::SubmissionIdentity
        | NativeV2CloudError::Ledger(RunLedgerError::AdmittedRunTooLarge) => {
            TargetAuthorityError::invalid(message)
        }
        NativeV2CloudError::Ledger(
            RunLedgerError::SubmissionConflict { .. } | RunLedgerError::RunIdConflict,
        ) => TargetAuthorityError::conflict(message),
        _ => TargetAuthorityError::unavailable(message),
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProductionHostingError {
    #[error("native-v2 target storage could not be prepared")]
    Storage,
    #[error("native-v2 target ledger could not be opened")]
    Ledger,
    #[error("native-v2 capsule configuration is invalid")]
    CapsuleConfiguration,
    #[error("native-v2 target controller could not be started")]
    Controller,
}

fn prepare_storage_root(path: &PathBuf) -> Result<PathBuf, ProductionHostingError> {
    std::fs::create_dir_all(path).map_err(|_| ProductionHostingError::Storage)?;
    let root = canonical_directory(path)?;
    let runs = prepare_runs_directory(&root)?;
    set_traversable_directory(&root).map_err(|_| ProductionHostingError::Storage)?;
    set_traversable_directory(&runs).map_err(|_| ProductionHostingError::Storage)?;
    Ok(root)
}

fn prepare_runs_directory(root: &std::path::Path) -> Result<PathBuf, ProductionHostingError> {
    let runs = root.join("runs");
    std::fs::create_dir(&runs)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|_| ProductionHostingError::Storage)?;
    canonical_directory(&runs)?;
    Ok(runs)
}

fn canonical_directory(path: &std::path::Path) -> Result<PathBuf, ProductionHostingError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ProductionHostingError::Storage)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProductionHostingError::Storage);
    }
    std::fs::canonicalize(path).map_err(|_| ProductionHostingError::Storage)
}

#[cfg(unix)]
fn prepare_production_ledger(path: &std::path::Path) -> Result<(), ProductionHostingError> {
    use std::ffi::OsString;
    use std::fs::OpenOptions;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};

    fn secure_file(path: &std::path::Path, create: bool) -> Result<(), ProductionHostingError> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
        if create {
            options.write(true).create(true);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(ProductionHostingError::Ledger),
        };
        let metadata = file
            .metadata()
            .map_err(|_| ProductionHostingError::Ledger)?;
        if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(ProductionHostingError::Ledger);
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| ProductionHostingError::Ledger)
    }

    secure_file(path, true)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar: OsString = path.as_os_str().to_owned();
        sidecar.push(suffix);
        secure_file(std::path::Path::new(&sidecar), false)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_production_ledger(_path: &std::path::Path) -> Result<(), ProductionHostingError> {
    Ok(())
}

#[cfg(unix)]
fn set_traversable_directory(path: &std::path::Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o711))
}

#[cfg(not(unix))]
fn set_traversable_directory(_path: &std::path::Path) -> Result<(), std::io::Error> {
    Ok(())
}

impl Default for ProductionHostingConfig {
    fn default() -> Self {
        Self {
            memory_storage: None,
            operator_diagnostic_output: None,
            workspace_storage: None,
            storage_root: PathBuf::from("/var/lib/zeroshot/native-v2"),
            copilot_executable: PathBuf::from("/usr/local/bin/copilot"),
            codex_executable: PathBuf::from("/usr/local/bin/codex"),
            claude_executable: "/usr/local/bin/claude".to_owned(),
            claude_prefix_arguments: Vec::new(),
            claude_process_environment: ClaudeProcessEnvironment::default(),
            pi_executable: "/usr/local/bin/pi".to_owned(),
            pi_prefix_arguments: Vec::new(),
            pi_process_environment: PiProcessEnvironment::default(),
            executable_search_path: "/usr/local/bin:/usr/bin:/bin".to_owned(),
            git_program: PathBuf::from("/usr/bin/git"),
            gh_program: PathBuf::from("/usr/bin/gh"),
            process_pool: HostedProcessPool::hosted_default(),
        }
    }
}
