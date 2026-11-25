use std::io;
use std::path::{Path, PathBuf};

/// Resolve only existing regular files and directories confined to the recorded working root.
/// Call again when using a live reference; an earlier validation never authorizes a later open.
pub fn workspace_path(root: &Path, path: &str) -> io::Result<PathBuf> {
    resolve_workspace_path(&root.canonicalize()?, path)
}

/// A live reference keeps the canonical root recorded at admission, even during redirection.
pub(super) fn local_workspace_path(root: &Path, path: &str) -> io::Result<PathBuf> {
    if root.canonicalize()? != root {
        return Err(super::denied());
    }
    resolve_workspace_path(root, path)
}

fn resolve_workspace_path(root: &Path, path: &str) -> io::Result<PathBuf> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err(super::invalid("workspace filepath must be nonempty"));
    }
    let requested = root.join(path);
    let resolved = requested.canonicalize()?;
    if !resolved.starts_with(root) {
        return Err(super::denied());
    }
    let metadata = resolved.metadata()?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(super::invalid(
            "workspace filepath must name a file or folder",
        ));
    }
    Ok(resolved)
}
