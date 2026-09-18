use std::path::Path;

use openengine_cluster_protocol::{RunId, is_canonical_uuid_v7};
use tokio::process::Command;

use super::{NativeV2CliError, local_io, local_message};
use crate::native_v2_capsule::provider_process::{
    CLAUDE_LOCAL_ENVIRONMENT, CODEX_LOCAL_ENVIRONMENT, local_environment,
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

pub(super) fn copy_minimal_process_environment(
    command: &mut Command,
) -> Result<(), NativeV2CliError> {
    command.envs(local_environment(CODEX_LOCAL_ENVIRONMENT));
    command.envs(local_environment(CLAUDE_LOCAL_ENVIRONMENT));
    for name in [
        "PATH",
        "LANG",
        "LC_ALL",
        "TERM",
        "TMPDIR",
        "HOME",
        "CODEX_HOME",
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
            let value = if name == "CODEX_HOME" {
                std::path::absolute(value)
                    .map_err(local_io)?
                    .into_os_string()
            } else {
                value
            };
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
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn rejects_socket_paths_that_cannot_fit_the_platform_address() {
        let capacity = local_socket_path_capacity();
        assert!(validate_local_socket_path(Path::new(&"x".repeat(capacity - 1))).is_ok());
        assert!(
            validate_local_socket_path(Path::new(&"x".repeat(capacity)))
                .is_err_and(|error| error.to_string().contains("ZEROSHOT_STATE_DIR"))
        );
    }

    #[test]
    fn local_controller_inherits_current_user_cli_home_paths() {
        let mut command = Command::new("true");
        command.env_clear();
        copy_minimal_process_environment(&mut command).expect("copy local environment");
        let environment = command
            .as_std()
            .get_envs()
            .filter_map(|(name, value)| value.map(|value| (name.to_owned(), value.to_owned())))
            .collect::<std::collections::BTreeMap<_, _>>();

        for name in ["HOME", "CODEX_HOME"] {
            let expected = std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    if name == "CODEX_HOME" {
                        std::path::absolute(value)
                            .expect("absolute config home")
                            .into_os_string()
                    } else {
                        value
                    }
                });
            assert_eq!(
                environment.get(std::ffi::OsStr::new(name)),
                expected.as_ref()
            );
        }
    }
}
