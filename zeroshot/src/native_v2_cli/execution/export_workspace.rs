use std::io::Write;
use std::path::Path;

use futures_util::StreamExt;
use tempfile::{NamedTempFile, TempPath};

use super::{CliOutcome, DetachSignal, NativeV2CliBackend, NativeV2CliError};
use crate::execution::platform::{FileAccess, private_file};
use crate::native_v2_cli::RunExportWorkspaceCommand;

pub(super) async fn execute<B: NativeV2CliBackend, S: DetachSignal, W: Write>(
    command: RunExportWorkspaceCommand,
    backend: &B,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError> {
    // Validate the JSON receipt before any download or publication (native paths may not be UTF-8).
    let receipt = serde_json::json!({"output": serde_json::to_value(&command.output)?});
    let staged = tokio::select! {
        biased;
        () = signal.wait() => return Err(NativeV2CliError::Target("workspace export cancelled".to_owned())),
        result = download(&command, backend) => result?,
    };
    // No network work remains: publish a complete private file without replacing any directory entry.
    staged
        .persist_noclobber(&command.output)
        .map_err(|error| error.error)?;
    super::write_json(output, &receipt)?;
    Ok(CliOutcome::Completed)
}

async fn download<B: NativeV2CliBackend>(
    command: &RunExportWorkspaceCommand,
    backend: &B,
) -> Result<NamedTempFile, NativeV2CliError> {
    let mut staged = stage(&command.output)?;
    let mut stream = backend
        .run_export_workspace(&command.target, command.params.clone())
        .await?;
    while let Some(chunk) = stream.next().await {
        staged.write_all(&chunk?)?;
    }
    staged.as_file().sync_all()?;
    Ok(staged)
}

fn stage(destination: &Path) -> Result<NamedTempFile, NativeV2CliError> {
    match std::fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "workspace export destination already exists",
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let path = parent.join(format!(".zeroshot-export-{}.tmp", uuid::Uuid::now_v7()));
    // This helper creates mode 0600 on Unix and an explicit private ACL at creation on Windows.
    let file = private_file(&path, FileAccess::CreateNew)?;
    let temporary = TempPath::try_from_path(path)?;
    Ok(NamedTempFile::from_parts(file, temporary))
}

#[cfg(test)]
#[path = "export_workspace_tests.rs"]
mod tests;
