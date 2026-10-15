//! Canonical local workspace and state boundaries, independent of repository identity.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::{LocalCompositionError, git_line};

pub(super) fn resolve_workspace(
    directory: &Path,
    git_program: &Path,
) -> Result<PathBuf, LocalCompositionError> {
    let directory =
        std::fs::canonicalize(directory).map_err(|_| LocalCompositionError::Workspace)?;
    if !directory.is_dir() {
        return Err(LocalCompositionError::Workspace);
    }
    match git_line(git_program, &directory, &["rev-parse", "--show-toplevel"]) {
        Ok(root) => std::fs::canonicalize(root).map_err(|_| LocalCompositionError::Workspace),
        Err(_) => Ok(directory),
    }
}

pub(crate) fn validate_workspace_state(
    workspace: &Path,
    state: &Path,
) -> Result<(), LocalCompositionError> {
    let workspace =
        std::fs::canonicalize(workspace).map_err(|_| LocalCompositionError::Workspace)?;
    let state = canonical_future_path(state).map_err(|_| LocalCompositionError::Storage)?;
    if workspace.starts_with(&state) || state.starts_with(&workspace) {
        return Err(LocalCompositionError::WorkspaceStateOverlap { workspace, state });
    }
    Ok(())
}

/// Resolve existing symlinks before processing later `..` components. Missing state directories
/// remain missing; preflight must not create storage inside the workspace it is checking.
fn canonical_future_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(_) => {
                resolved.push(component.as_os_str());
                resolved = resolve_existing_component(resolved)?;
            }
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
        }
    }
    Ok(resolved)
}

fn resolve_existing_component(path: PathBuf) -> io::Result<PathBuf> {
    match std::fs::canonicalize(&path) {
        Ok(existing) => Ok(existing),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(&path).is_ok() {
                Err(error)
            } else {
                Ok(path)
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "workspace/tests.rs"]
mod tests;
