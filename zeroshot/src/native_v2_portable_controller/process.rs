use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use openengine_cluster_client::NdjsonTransport;
use openengine_cluster_protocol::{RunConnectionValues, RunId, RunSubmission};
use openengine_cluster_server::admission::CancellationSignal;
use openengine_cluster_server::identity::{
    BindingAttributes, ConnectionBinding, ConnectionIdentity, ConnectionIdentityConfig,
    PrincipalId, StaticConnectionIdentityResolver, SystemConnectionTime, TenantId,
};
use serde::{Deserialize, Serialize};

use crate::execution::process::write_new_file;
use crate::native_v2_admission::DeliveryPolicy;
use crate::native_v2_supervisor::RunEnvironment;

use super::controller::PortableRunController;
use super::engine::PortableRuntime;
use super::{
    PortableControllerBootstrap, PortableControllerError, PortableControllerPaths,
    PortableControllerReady,
};

const BOOTSTRAP_MAX_BYTES: u64 = 4 * 1024 * 1024;
const READY_MAX_BYTES: u64 = 16 * 1024;
const READY_KIND: &str = "zeroshot.portable-controller-ready/v1";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PortableBootstrapDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<crate::native_v2_supervisor::checkpoints::CheckpointRestore>,
    run_id: RunId,
    delivery_run_id: RunId,
    adopt_existing_delivery: bool,
    submission: RunSubmission,
    connections: RunConnectionValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github_token: Option<String>,
    workspace: PathBuf,
    workspace_lease: PathBuf,
    storage: PathBuf,
    delivery_policy: DeliveryPolicy,
}

impl PortableBootstrapDocument {
    fn validate(self) -> Result<PortableControllerBootstrap, PortableControllerError> {
        require_absolute(&self.workspace)?;
        require_absolute(&self.workspace_lease)?;
        require_absolute(&self.storage)?;
        let environment = RunEnvironment::exact(&self.submission.runtime, self.connections)?;
        Ok(PortableControllerBootstrap {
            checkpoint: self.checkpoint,
            run_id: self.run_id,
            delivery_run_id: self.delivery_run_id,
            adopt_existing_delivery: self.adopt_existing_delivery,
            submission: self.submission,
            environment,
            github_token: self.github_token,
            workspace: self.workspace,
            workspace_lease: self.workspace_lease,
            storage: self.storage,
            delivery_policy: self.delivery_policy,
        })
    }
}

pub type PortableControllerTransport = NdjsonTransport<
    tokio::io::ReadHalf<super::transport::Client>,
    tokio::io::WriteHalf<super::transport::Client>,
>;

pub async fn connect_transport(
    paths: &PortableControllerPaths,
) -> Result<Arc<PortableControllerTransport>, PortableControllerError> {
    let ready = read_ready(paths)?;
    let stream = super::transport::connect(&ready.socket)
        .await
        .map_err(PortableControllerError::Io)?;
    let (reader, writer) = tokio::io::split(stream);
    Ok(Arc::new(NdjsonTransport::new(reader, writer)))
}

pub fn read_ready(
    paths: &PortableControllerPaths,
) -> Result<PortableControllerReady, PortableControllerError> {
    let bytes = read_bounded_regular_file(&paths.ready(), READY_MAX_BYTES)?;
    let ready: PortableControllerReady =
        serde_json::from_slice(&bytes).map_err(|_| PortableControllerError::Readiness)?;
    if ready.kind != READY_KIND || ready.socket != paths.socket() {
        return Err(PortableControllerError::Readiness);
    }
    Ok(ready)
}

pub async fn wait_ready(
    paths: &PortableControllerPaths,
    run_id: &RunId,
    deadline: Duration,
) -> Result<PortableControllerReady, PortableControllerError> {
    tokio::time::timeout(deadline, async {
        loop {
            if let Ok(ready) = read_ready(paths) {
                if &ready.run_id == run_id {
                    return ready;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| PortableControllerError::Readiness)
}

/// Runs the private one-run controller child used by the shipped executable's re-exec path.
/// The bootstrap is consumed before any durable controller effect and is never retained.
pub async fn run_controller_process(bootstrap_path: &Path) -> Result<(), PortableControllerError> {
    let bootstrap = load_bootstrap_file(bootstrap_path)?;
    let workspace = bootstrap.workspace.clone();
    let storage = bootstrap.storage.clone();
    let delivery_run_id = bootstrap.delivery_run_id.clone();
    let adopt_existing_delivery = bootstrap.adopt_existing_delivery;
    let github_token = bootstrap.github_token.clone();
    let controller = Arc::new(
        PortableRunController::start(bootstrap, move |admitted| {
            crate::native_v2_local::build_local_process_candidate(
                crate::native_v2_local::LocalProcessCandidateRequest {
                    admitted,
                    delivery_run_id: delivery_run_id.clone(),
                    adopt_existing_delivery,
                    workspace: &workspace,
                    storage: &storage,
                    github_token,
                },
            )
            .map(PortableRuntime::new)
        })
        .await?,
    );
    controller.bind().await?.serve_until_terminal().await
}

pub fn load_bootstrap_file(
    path: &Path,
) -> Result<PortableControllerBootstrap, PortableControllerError> {
    let file = validate_private_bootstrap(path)?;
    let bytes = read_bounded_file(file, BOOTSTRAP_MAX_BYTES)?;
    let parsed = serde_json::from_slice::<PortableBootstrapDocument>(&bytes)
        .map_err(|_| PortableControllerError::Bootstrap)
        .and_then(PortableBootstrapDocument::validate);
    std::fs::remove_file(path).map_err(|_| PortableControllerError::BootstrapCleanup)?;
    parsed
}

pub fn write_bootstrap_file(
    path: &Path,
    bootstrap: &PortableControllerBootstrap,
) -> Result<(), PortableControllerError> {
    let bytes = encode_bootstrap(bootstrap)?;
    prepare_bootstrap_parent(path)?;
    write_private_new_file(path, &bytes)
}

fn encode_bootstrap(
    bootstrap: &PortableControllerBootstrap,
) -> Result<Vec<u8>, PortableControllerError> {
    require_absolute(&bootstrap.workspace)?;
    require_absolute(&bootstrap.workspace_lease)?;
    require_absolute(&bootstrap.storage)?;
    let environment = bootstrap
        .environment
        .for_runtime(&bootstrap.submission.runtime)?;
    let document = PortableBootstrapDocument {
        checkpoint: bootstrap.checkpoint.clone(),
        run_id: bootstrap.run_id.clone(),
        delivery_run_id: bootstrap.delivery_run_id.clone(),
        adopt_existing_delivery: bootstrap.adopt_existing_delivery,
        submission: bootstrap.submission.clone(),
        connections: environment.bootstrap_values(),
        github_token: bootstrap.github_token.clone(),
        workspace: bootstrap.workspace.clone(),
        workspace_lease: bootstrap.workspace_lease.clone(),
        storage: bootstrap.storage.clone(),
        delivery_policy: bootstrap.delivery_policy,
    };
    let bytes = serde_json::to_vec(&document).map_err(|_| PortableControllerError::Bootstrap)?;
    if bytes.len() as u64 > BOOTSTRAP_MAX_BYTES {
        return Err(PortableControllerError::Bootstrap);
    }
    Ok(bytes)
}

fn prepare_bootstrap_parent(path: &Path) -> Result<(), PortableControllerError> {
    require_absolute(path)?;
    let parent = path.parent().ok_or(PortableControllerError::Path)?;
    crate::execution::platform::private_directory(parent).map_err(PortableControllerError::Io)?;
    let metadata = std::fs::symlink_metadata(parent).map_err(PortableControllerError::Io)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(PortableControllerError::Bootstrap);
    }
    Ok(())
}

pub struct PortableControllerServer {
    controller: Arc<PortableRunController>,
    listener: super::transport::Listener,
}

impl PortableControllerServer {
    pub(super) async fn bind(
        controller: Arc<PortableRunController>,
    ) -> Result<Self, PortableControllerError> {
        let socket = controller.paths().socket();
        let listener =
            super::transport::Listener::bind(&socket).map_err(PortableControllerError::Io)?;
        write_ready(&controller)?;
        Ok(Self {
            controller,
            listener,
        })
    }

    pub async fn serve(self) -> io::Result<()> {
        loop {
            self.accept().await?;
        }
    }

    /// Serves one active local run until its durable terminal result exists. If a very short run
    /// finishes before the submitting CLI reaches the socket, one connection is still accepted so
    /// readiness cannot race normal startup. Later observation reopens the durable ledger.
    async fn serve_until_terminal(self) -> Result<(), PortableControllerError> {
        let mut accepted = false;
        let mut terminal = false;
        loop {
            if accepted && terminal {
                return Ok(());
            }
            tokio::select! {
                result = self.accept() => {
                    result.map_err(PortableControllerError::Io)?;
                    accepted = true;
                }
                result = self.controller.wait_terminal(), if !terminal => {
                    result?;
                    terminal = true;
                }
            }
        }
    }

    async fn accept(&self) -> io::Result<()> {
        let stream = self.listener.accept().await?;
        let controller = self.controller.clone();
        tokio::spawn(async move {
            let (reader, writer) = tokio::io::split(stream);
            let binding = local_binding(controller);
            let _ = openengine_cluster_server::stdio::serve_ndjson(
                binding,
                openengine_cluster_server::stdio::NdjsonIo::new(reader, writer, tokio::io::sink()),
            )
            .await;
        });
        Ok(())
    }
}

fn local_binding(
    controller: Arc<PortableRunController>,
) -> ConnectionBinding<PortableRunController, StaticConnectionIdentityResolver, SystemConnectionTime>
{
    let identity = ConnectionIdentity::new(ConnectionIdentityConfig {
        principal: PrincipalId::new("local-controller"),
        tenant: TenantId::new("local-run"),
        issued_at_ms: None,
        expires_at_ms: u64::MAX,
        binding_attributes: BindingAttributes::default(),
    });
    ConnectionBinding::new(
        controller,
        StaticConnectionIdentityResolver::new(identity),
        SystemConnectionTime,
        CancellationSignal::default(),
    )
}

pub(super) fn require_absolute(path: &Path) -> Result<(), PortableControllerError> {
    (path.is_absolute() && !path.as_os_str().is_empty())
        .then_some(())
        .ok_or(PortableControllerError::Path)
}

pub(super) fn validate_ledger_path(path: &Path) -> Result<(), PortableControllerError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(PortableControllerError::LedgerPath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PortableControllerError::LedgerPath),
    }
}

pub(super) fn validate_existing_storage(path: &Path) -> Result<(), PortableControllerError> {
    validate_existing_path(path, std::fs::Metadata::is_dir)
}

pub(super) fn validate_existing_ledger_path(path: &Path) -> Result<(), PortableControllerError> {
    validate_existing_path(path, std::fs::Metadata::is_file)
}

fn validate_existing_path(
    path: &Path,
    expected: fn(&std::fs::Metadata) -> bool,
) -> Result<(), PortableControllerError> {
    let metadata = std::fs::symlink_metadata(path).map_err(PortableControllerError::Io)?;
    if expected(&metadata) && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(PortableControllerError::LedgerPath)
    }
}

pub(super) fn clear_stale_endpoint(
    paths: &PortableControllerPaths,
) -> Result<(), PortableControllerError> {
    remove_existing_socket(&paths.socket())?;
    remove_existing_regular_file(&paths.ready())
}

fn remove_existing_socket(path: &Path) -> Result<(), PortableControllerError> {
    super::transport::remove_endpoint(path).map_err(|_| PortableControllerError::EndpointPath)
}

fn remove_existing_regular_file(path: &Path) -> Result<(), PortableControllerError> {
    remove_existing_endpoint(path, std::fs::FileType::is_file)
}

fn remove_existing_endpoint(
    path: &Path,
    expected: impl FnOnce(&std::fs::FileType) -> bool,
) -> Result<(), PortableControllerError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if expected(&metadata.file_type()) => {
            std::fs::remove_file(path).map_err(PortableControllerError::Io)
        }
        Ok(_) => Err(PortableControllerError::EndpointPath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PortableControllerError::Io(error)),
    }
}

fn read_bounded_regular_file(
    path: &Path,
    maximum: u64,
) -> Result<Vec<u8>, PortableControllerError> {
    let file = crate::execution::platform::private_file(
        path,
        crate::execution::platform::FileAccess::Read,
    )
    .map_err(PortableControllerError::Io)?;
    read_bounded_file(file, maximum)
}

fn read_bounded_file(
    file: std::fs::File,
    maximum: u64,
) -> Result<Vec<u8>, PortableControllerError> {
    let metadata = file.metadata().map_err(PortableControllerError::Io)?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(PortableControllerError::Bootstrap);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(PortableControllerError::Io)?;
    if bytes.len() as u64 > maximum {
        return Err(PortableControllerError::Bootstrap);
    }
    Ok(bytes)
}

fn validate_private_bootstrap(path: &Path) -> Result<std::fs::File, PortableControllerError> {
    require_absolute(path)?;
    crate::execution::platform::private_file(path, crate::execution::platform::FileAccess::Read)
        .map_err(|_| PortableControllerError::BootstrapPermissions)
}

fn write_private_new_file(path: &Path, bytes: &[u8]) -> Result<(), PortableControllerError> {
    write_new_file(path, bytes, 0o600).map_err(PortableControllerError::Io)
}

fn write_ready(controller: &PortableRunController) -> Result<(), PortableControllerError> {
    let ready = PortableControllerReady {
        kind: READY_KIND.to_owned(),
        run_id: controller.run_id().clone(),
        socket: controller.paths().socket(),
        pid: std::process::id(),
    };
    let bytes = serde_json::to_vec(&ready).map_err(|_| PortableControllerError::Readiness)?;
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|_| PortableControllerError::Readiness)?;
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let temporary = controller
        .paths()
        .storage()
        .join(format!(".controller.ready-{suffix}.tmp"));
    let result = (|| {
        let mut file = crate::execution::platform::private_file(
            &temporary,
            crate::execution::platform::FileAccess::CreateNew,
        )
        .map_err(PortableControllerError::Io)?;
        file.write_all(&bytes)
            .map_err(PortableControllerError::Io)?;
        file.sync_all().map_err(PortableControllerError::Io)?;
        drop(file);
        crate::execution::platform::commit_file(
            &temporary,
            &controller.paths().ready(),
            controller.paths().storage(),
        )
        .map_err(PortableControllerError::Io)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
