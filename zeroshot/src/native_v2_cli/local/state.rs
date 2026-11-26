use std::ffi::OsString;
use std::path::Path;

use openengine_cluster_protocol::{RunId, RunResumeFrom, is_canonical_uuid_v7};
use tokio::process::Command;

use crate::execution::platform::{FileAccess, private_file};
use super::super::support::{CommitPaths, cleanup_temporary, write_and_commit};
use super::{
    LocalRecoveryDocument, NativeV2CliError, RunLedger, SqliteRunLedger, local_error, local_io,
    local_message,
};

pub(super) fn validate_local_run_id(run_id: &RunId) -> Result<(), NativeV2CliError> {
    is_canonical_uuid_v7(run_id)
        .then_some(())
        .ok_or_else(|| local_message("run ID is not a local controller identity"))
}

pub(super) fn local_run_id_from_entry(entry: std::fs::DirEntry) -> Option<RunId> {
    let file_type = entry.file_type().ok()?;
    if !file_type.is_dir() || file_type.is_symlink() {
        return None;
    }
    let name = entry.file_name().to_str()?.to_owned();
    let run_id = RunId::new(name);
    validate_local_run_id(&run_id).ok().map(|()| run_id)
}

#[cfg(unix)]
pub(super) fn validate_local_socket_path(path: &Path) -> Result<(), NativeV2CliError> {
    use std::os::unix::ffi::OsStrExt as _;

    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(local_message(
            "controller socket path cannot contain a null byte",
        ));
    }
    if bytes.len() >= local_socket_path_capacity() {
        return Err(local_message(
            "controller socket path is too long; set ZEROSHOT_STATE_DIR to a shorter absolute directory",
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn validate_local_socket_path(_path: &Path) -> Result<(), NativeV2CliError> {
    Ok(())
}

#[cfg(unix)]
fn local_socket_path_capacity() -> usize {
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_path.len()
}

const RESTIC_ENVIRONMENT: &str = "ZEROSHOT_RESTIC";

pub(super) fn copy_minimal_process_environment(
    command: &mut Command,
) -> Result<(), NativeV2CliError> {
    copy_minimal_environment(command, |name| std::env::var_os(name))
}

fn copy_minimal_environment(
    command: &mut Command,
    source: impl Fn(&str) -> Option<OsString>,
) -> Result<(), NativeV2CliError> {
    for name in [
        "PATH",
        "LANG",
        "LC_ALL",
        "TERM",
        "TMPDIR",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
    ] {
        if let Some(value) = source(name).filter(|value| !value.is_empty()) {
            command.env(name, value);
        }
    }
    // The Restic selector names Zeroshot's own checkpoint tool, not harness context. Resolve a
    // relative value here so the detached controller finds the same executable; an empty value
    // stays empty so the controller rejects it like any other Zeroshot process.
    if let Some(value) = source(RESTIC_ENVIRONMENT) {
        let value = if value.is_empty() {
            value
        } else {
            std::path::absolute(value)
                .map_err(local_io)?
                .into_os_string()
        };
        command.env(RESTIC_ENVIRONMENT, value);
    }
    Ok(())
}

pub(super) fn prepare_private_directory(path: &Path) -> Result<(), NativeV2CliError> {
    crate::execution::platform::private_directory(path).map_err(local_io)
}

pub(super) fn require_existing_ledger(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(std::io::Error::other(
            "run ledger path is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn remove_private_bootstrap(path: &Path) {
    if std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    {
        let _ = std::fs::remove_file(path);
    }
}

impl LocalRecoveryDocument {
    pub(super) fn workspace_checkpoints_enabled(&self) -> bool {
        self.submission.runtime.workspace_checkpoints && !self.workspace_checkpoints_discarded
    }

    pub(super) fn read(path: &Path) -> Result<Self, NativeV2CliError> {
        let bytes = std::fs::read(path).map_err(|error| {
            let reason = if error.kind() == std::io::ErrorKind::NotFound {
                "is missing"
            } else {
                "could not be read"
            };
            local_message(format!(
                "workspace recovery record {} {reason}: {error}",
                path.display()
            ))
        })?;
        openengine_cluster_protocol::bounded_json_from_slice(&bytes).map_err(|_| {
            local_message(format!(
                "workspace recovery record {} is invalid",
                path.display()
            ))
        })
    }

    pub(super) fn write(&self, path: &Path) -> Result<(), NativeV2CliError> {
        let parent = path
            .parent()
            .ok_or_else(|| local_message("workspace recovery path is invalid"))?;
        let temporary = parent.join(format!(".workspace-recovery-{}.tmp", uuid::Uuid::now_v7()));
        let bytes = serde_json::to_vec(self)
            .map_err(|_| local_message("workspace recovery metadata could not be encoded"))?;
        let result = (|| {
            let file = private_file(&temporary, FileAccess::CreateNew).map_err(local_io)?;
            write_and_commit(
                file,
                &bytes,
                CommitPaths {
                    temporary: &temporary,
                    destination: path,
                    parent,
                },
            )
        })();
        cleanup_temporary(result, &temporary)
    }
}

#[cfg(test)]
#[path = "state/tests.rs"]
mod tests;

impl super::LocalCliBackend {
    pub(super) async fn managed_restart_required(
        &self,
        params: &openengine_cluster_protocol::RunResumeParams,
        recovery: &LocalRecoveryDocument,
    ) -> Result<bool, NativeV2CliError> {
        if !matches!(params.from, None | Some(RunResumeFrom::Restart {}))
            || !recovery
                .submission
                .workspace
                .as_ref()
                .is_some_and(|workspace| !workspace.is_in_place())
            || recovery.workspace_checkpoints_enabled()
        {
            return Ok(false);
        }
        let ledger = SqliteRunLedger::open_read_only(self.paths(&params.run_id)?.ledger())
            .map_err(local_error)?;
        let stored = ledger
            .get(&params.run_id)
            .await
            .map_err(local_error)?
            .ok_or_else(|| local_message("predecessor ledger is unavailable"))?;
        // A successor restarts at its original execution seed. Keep the corresponding live
        // files when completed work is retained; original inputs only match an empty seed.
        Ok(
            stored.snapshot.phase != crate::v2_run_ledger::RunPhase::Suspended
                && stored.snapshot.execution_seed.is_empty(),
        )
    }
}
