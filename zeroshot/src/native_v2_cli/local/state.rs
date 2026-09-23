use std::path::Path;

use openengine_cluster_protocol::{RunId, is_canonical_uuid_v7};
use tokio::process::Command;

use super::{NativeV2CliError, local_io, local_message};

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

pub(super) fn copy_minimal_process_environment(
    command: &mut Command,
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
        if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            command.env(name, value);
        }
    }
    Ok(())
}

pub(super) fn prepare_private_directory(path: &Path) -> Result<(), NativeV2CliError> {
    crate::execution::platform::private_directory(path).map_err(local_io)
}

pub(super) fn require_existing_ledger(path: &Path) -> Result<bool, NativeV2CliError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(local_message("run ledger path is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(local_io(error)),
    }
}

pub(super) fn remove_private_bootstrap(path: &Path) {
    if std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
#[path = "state/tests.rs"]
mod tests;
