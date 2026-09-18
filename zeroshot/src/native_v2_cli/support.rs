use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use super::NativeV2CliError;

pub const VERSION: &str = concat!("zeroshot ", env!("CARGO_PKG_VERSION"), "\n");

pub(super) fn nonempty_environment(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

pub(super) fn absolute_user_path(
    path: impl Into<PathBuf>,
    invalid_message: &'static str,
) -> Result<PathBuf, NativeV2CliError> {
    let path = path.into();
    if path.is_absolute() && !path.as_os_str().is_empty() {
        Ok(path)
    } else {
        Err(NativeV2CliError::Local(invalid_message.to_owned()))
    }
}

pub(super) fn cleanup_temporary<T>(
    result: Result<T, NativeV2CliError>,
    path: &std::path::Path,
) -> Result<T, NativeV2CliError> {
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

pub(super) struct CommitPaths<'a> {
    pub(super) temporary: &'a Path,
    pub(super) destination: &'a Path,
    pub(super) parent: &'a Path,
}

pub(super) fn write_and_commit(
    file: File,
    contents: &[u8],
    paths: CommitPaths<'_>,
) -> Result<(), NativeV2CliError> {
    let mut writer = BufWriter::new(file);
    writer.write_all(contents).map_err(local_io)?;
    writer.flush().map_err(local_io)?;
    writer.get_ref().sync_all().map_err(local_io)?;
    drop(writer);
    crate::execution::platform::commit_file(paths.temporary, paths.destination, paths.parent)
        .map_err(local_io)
}

fn local_io(error: std::io::Error) -> NativeV2CliError {
    NativeV2CliError::Local(error.to_string())
}

pub(crate) fn default_local_state_root() -> Result<PathBuf, NativeV2CliError> {
    if let Some(path) = nonempty_environment("ZEROSHOT_STATE_DIR") {
        return absolute_user_path(path, "controller state path must be absolute");
    }
    #[cfg(windows)]
    {
        let root = nonempty_environment("LOCALAPPDATA")
            .ok_or_else(|| NativeV2CliError::Local("LOCALAPPDATA is unavailable".into()))?;
        absolute_user_path(
            PathBuf::from(root).join("zeroshot").join("state"),
            "controller state path must be absolute",
        )
    }
    #[cfg(unix)]
    default_unix_state_root()
}

#[cfg(unix)]
fn default_unix_state_root() -> Result<PathBuf, NativeV2CliError> {
    if let Some(path) = nonempty_environment("XDG_STATE_HOME") {
        return absolute_user_path(
            PathBuf::from(path).join("zeroshot"),
            "controller state path must be absolute",
        );
    }
    let home = nonempty_environment("HOME")
        .ok_or_else(|| NativeV2CliError::Local("HOME and XDG_STATE_HOME are unavailable".into()))?;
    absolute_user_path(
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("zeroshot"),
        "controller state path must be absolute",
    )
}
