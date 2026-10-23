use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::{Result, invalid};
use crate::active_memory::MAX_STORE_BYTES;

#[cfg(test)]
#[path = "files/tests.rs"]
mod tests;

pub(super) fn create_directory(path: &Path) -> Result<()> {
    reject_links(path)?;
    crate::execution::platform::create_private_directory(path)?;
    private_directory(path)
}

pub(super) fn reject_links(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(invalid("initialization paths cannot traverse symlinks"));
        }
    }
    Ok(())
}

pub(super) fn private_directory(path: &Path) -> Result<()> {
    reject_links(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(invalid("session must be a private directory"));
    }
    #[cfg(windows)]
    {
        let directory = crate::execution::platform::open_directory(path)?;
        crate::execution::platform::validate_private_file(&directory)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid(
                "session directory must be owned by this user and mode 0700",
            ));
        }
    }
    Ok(())
}

pub(super) fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_STORE_BYTES {
        return Err(invalid("initialization document exceeds byte budget"));
    }
    Ok(bytes)
}

pub(super) fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = bounded(value)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing session directory"))?;
    private_directory(parent)?;
    let temporary = parent.join(format!(".memory-{}.tmp", uuid::Uuid::now_v7()));
    let file = crate::execution::platform::private_file(
        &temporary,
        crate::execution::platform::FileAccess::CreateNew,
    )?;
    let temporary = tempfile::TempPath::try_from_path(temporary)?;
    let mut staged = tempfile::NamedTempFile::from_parts(file, temporary);
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    let temporary = staged.into_temp_path();
    crate::execution::platform::commit_file(&temporary, path, parent)?;
    Ok(())
}

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    reject_links(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES as u64 {
        return Err(invalid("expected a bounded regular initialization file"));
    }
    let mut file = crate::execution::platform::open_readonly_file(path)?;
    #[cfg(windows)]
    validate_input_file(path, &file)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_STORE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STORE_BYTES {
        return Err(invalid("initialization file exceeds byte budget"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(windows)]
fn validate_input_file(path: &Path, file: &File) -> Result<()> {
    if crate::execution::platform::validate_private_file(file).is_ok() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing session directory"))?;
    private_directory(parent)?;
    crate::execution::platform::private_inherited_file(path)?;
    crate::execution::platform::validate_private_file(file)?;
    Ok(())
}

pub(super) fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(bounded(value)?)))
}

/// The source authority is tracked plus nonignored untracked files; generated/ignored artifacts
/// cannot be used as maintenance citations. No branch switch, fetch, or index write is performed.
pub(super) async fn source_files(workspace: &Path) -> Result<BTreeSet<String>> {
    let mut command = tokio::process::Command::new("git");
    command
        .current_dir(workspace)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    crate::native_v2_delivery::transport::remove_repository_environment(command.as_std_mut());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("git source inventory unavailable"))?;
    let result =
        tokio::time::timeout(Duration::from_secs(30), read_git_paths(&mut child, stdout)).await;
    match result {
        Ok(Ok(paths)) => Ok(paths),
        other => {
            let _ = child.kill().await;
            other.map_err(|_| invalid("Git source inventory timed out"))?
        }
    }
}

async fn read_git_paths(
    child: &mut tokio::process::Child,
    stdout: tokio::process::ChildStdout,
) -> Result<BTreeSet<String>> {
    let mut bytes = Vec::new();
    stdout
        .take(MAX_STORE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > MAX_STORE_BYTES {
        return Err(invalid("Git source inventory exceeds byte budget"));
    }
    if !child.wait().await?.success() {
        return Err(invalid("memory initialization requires a Git checkout"));
    }
    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            String::from_utf8(path.to_vec()).map_err(|_| invalid("source paths must be UTF-8"))
        })
        .collect()
}

pub(super) async fn source_digest(workspace: &Path) -> Result<String> {
    let paths = source_files(workspace).await?;
    let workspace = workspace.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut digest = Sha256::new();
        for path in paths {
            hash_path(&mut digest, &workspace, &path)?;
        }
        Ok(format!("{:x}", digest.finalize()))
    })
    .await
    .map_err(|_| invalid("source fingerprint task failed"))?
}

fn hash_path(digest: &mut Sha256, workspace: &Path, path: &str) -> Result<()> {
    digest.update((path.len() as u64).to_le_bytes());
    digest.update(path.as_bytes());
    let absolute = workspace.join(path);
    let metadata = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            digest.update(b"missing");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        digest.update(b"symlink");
        digest.update(fs::read_link(absolute)?.to_string_lossy().as_bytes());
    } else if metadata.is_file() {
        hash_regular(digest, &absolute, &metadata)?;
    } else {
        return Err(invalid(
            "submodules and non-file source entries require separate repository initialization",
        ));
    }
    Ok(())
}

fn hash_regular(digest: &mut Sha256, path: &Path, metadata: &fs::Metadata) -> Result<()> {
    reject_links(path)?;
    let mut file = File::open(path)?;
    digest.update(b"file");
    digest.update(metadata.len().to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        digest.update((metadata.permissions().mode() & 0o111).to_le_bytes());
    }
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(())
}

pub(super) fn optional<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::symlink_metadata(path) {
        Ok(_) => read(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn sources_digest(workspaces: &BTreeSet<PathBuf>) -> Result<String> {
    let mut sources = Vec::new();
    for workspace in workspaces {
        sources.push((workspace, source_digest(workspace).await?));
    }
    digest(&sources)
}
