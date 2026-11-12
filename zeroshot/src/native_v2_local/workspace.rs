//! Canonical local workspace and state boundaries, independent of repository identity.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::{LocalCompositionError, git_line};

pub(crate) fn resolve_workspace(
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

/// Resolve the source commit and review target before creating any worktree.
pub(crate) async fn prepare_branch_source(
    prepared: &mut super::PreparedLocalRun,
    worktree: Option<crate::native_v2_cli::LocalWorktreeBase>,
    git_program: &Path,
    state_root: &Path,
) -> Result<(), LocalCompositionError> {
    use crate::native_v2_cli::LocalWorktreeBase;
    use crate::native_v2_delivery::transport::source_credential;
    use crate::source_code_provider::SourceCapability;
    let capabilities =
        crate::native_v2_admission::delivery_capabilities(&prepared.submission.graph.root);
    if capabilities.is_empty() && worktree.is_none() {
        return Ok(());
    }
    let branch = super::source::attached_branch(&prepared.workspace, git_program)?;
    prepared.local_branch = Some(branch.clone());
    let upstream = super::source::branch_upstream(&prepared.workspace, git_program, &branch);
    if worktree.is_some() {
        let revision = openengine_cluster_protocol::SourceRevisionId::new(git_line(
            git_program,
            &prepared.workspace,
            &["rev-parse", "--verify", "HEAD^{commit}"],
        )?)
        .map_err(|_| LocalCompositionError::ResolvedSource)?;
        prepared.workspace_revision = Some(revision.clone());
        if let Some(source) = &mut prepared.submission.source {
            source.revision = revision;
        }
    }
    if worktree == Some(LocalWorktreeBase::Remote) {
        let (_, upstream_branch) = upstream
            .as_ref()
            .ok_or(LocalCompositionError::MissingWorktreeUpstream)?;
        let (transport, token, credentials) =
            super::source::local_transport(prepared, git_program, state_root).await?;
        let credential = source_credential(token.as_deref().unwrap_or(""), credentials.as_ref());
        let revision = transport
            .read_ref(upstream_branch, credential)
            .await
            .map_err(|error| LocalCompositionError::Worktree(error.to_string()))?
            .ok_or_else(|| LocalCompositionError::MissingRemoteBase {
                branch: upstream_branch.clone(),
            })?;
        transport
            .fetch_exact(&revision, credential)
            .await
            .map_err(|error| LocalCompositionError::Worktree(error.to_string()))?;
        prepared
            .submission
            .source
            .as_mut()
            .ok_or(LocalCompositionError::DeliverySourceRequired)?
            .revision = openengine_cluster_protocol::SourceRevisionId::new(revision)
            .map_err(|_| LocalCompositionError::ResolvedSource)?;
        prepared.workspace_revision = prepared
            .submission
            .source
            .as_ref()
            .map(|source| source.revision.clone());
    }
    if worktree.is_none() && capabilities.contains(&SourceCapability::PullRequest) {
        let target = match upstream {
            Some((_, target)) if target != branch => target,
            _ => {
                let (transport, token, credentials) =
                    super::source::local_transport(prepared, git_program, state_root).await?;
                transport
                    .default_branch(source_credential(
                        token.as_deref().unwrap_or(""),
                        credentials.as_ref(),
                    ))
                    .await
                    .map_err(|_| LocalCompositionError::RemoteBaseCheck {
                        branch: "remote default".to_owned(),
                    })?
            }
        };
        if branch == target {
            return Err(LocalCompositionError::DeliveryOnTarget { branch });
        }
        prepared
            .submission
            .source
            .as_mut()
            .ok_or(LocalCompositionError::DeliverySourceRequired)?
            .branch = openengine_cluster_protocol::SourceBranchId::new(target)
            .map_err(|_| LocalCompositionError::ResolvedSource)?;
    }
    Ok(())
}

/// Add a committed-files-only worktree. It remains user-owned after every outcome.
pub(crate) async fn create_run_worktree(
    prepared: &mut super::PreparedLocalRun,
    git_program: &Path,
    state_root: &Path,
) -> Result<(), LocalCompositionError> {
    let revision = prepared
        .workspace_revision
        .as_ref()
        .ok_or(LocalCompositionError::ResolvedSource)?;
    let name = state_root
        .file_name()
        .ok_or(LocalCompositionError::Storage)?
        .to_string_lossy();
    let root = state_root.with_file_name(format!("{name}-worktrees"));
    validate_workspace_state(&prepared.workspace, &root)?;
    super::prepare_private_directory(&root)?;
    let workspace = root.join(prepared.run_id.as_str());
    // Run IDs also name storage directories and must be one bounded path component.
    if !matches!(
        Path::new(prepared.run_id.as_str())
            .components()
            .collect::<Vec<_>>()
            .as_slice(),
        [Component::Normal(_)]
    ) || workspace.parent() != Some(root.as_path())
    {
        return Err(LocalCompositionError::RunIdentity);
    }
    let branch = crate::native_v2_delivery::delivery_head_branch(&prepared.delivery_run_id);
    let mut command = crate::native_v2_delivery::command::git_command(
        git_program,
        &prepared.workspace,
        crate::native_v2_delivery::command::GitCommandContext {
            identity: None,
            local_environment: Some(&prepared.native_environment),
            commit_identity: None,
        },
    );
    command
        .args(["worktree", "add", "--no-track", "-b", &branch])
        .arg(&workspace)
        .arg(revision.as_str());
    crate::native_v2_delivery::command::capture(&mut command, std::time::Duration::from_secs(600))
        .await
        .and_then(|output| output.require_success())
        .map_err(|error| LocalCompositionError::Worktree(error.to_string()))?;
    prepared.invoking_workspace = Some(prepared.workspace.clone());
    prepared.workspace =
        std::fs::canonicalize(workspace).map_err(|_| LocalCompositionError::Workspace)?;
    prepared.local_branch = Some(branch);
    validate_workspace_state(&prepared.workspace, state_root)?;
    Ok(())
}

/// Reattach the saved run branch after checkpoint restore, without moving its shared tip.
pub(super) fn attach_run_branch(
    workspace: &Path,
    git_program: &Path,
    branch: &str,
) -> Result<(), LocalCompositionError> {
    if !crate::source_code_provider::valid_git_branch(branch) {
        return Err(LocalCompositionError::ResolvedSource);
    }
    let reference = format!("refs/heads/{branch}");
    git_line(
        git_program,
        workspace,
        &["show-ref", "--verify", &reference],
    )?;
    if let Some(owner) = branch_owner_elsewhere(workspace, git_program, branch)? {
        return Err(LocalCompositionError::Worktree(format!(
            "run branch `{branch}` is checked out in {}; release it there before resuming this checkout",
            owner.display(),
        )));
    }
    if let Ok(current) = super::source::attached_branch(workspace, git_program) {
        if current != branch {
            return Err(LocalCompositionError::Worktree(format!(
                "run branch is `{branch}`; return with `git switch {branch}` before resuming"
            )));
        }
        return Ok(());
    }
    // symbolic-ref changes HEAD only. The restored files/index remain the checkpoint's candidate.
    super::git_output(
        git_program,
        workspace,
        &["symbolic-ref", "HEAD", &reference],
    )?;
    Ok(())
}

/// Git's NUL-delimited listing preserves checkout paths containing spaces or newlines.
/// Missing but registered checkouts still own their branches until explicitly removed/pruned.
pub(super) fn branch_owner_elsewhere(
    workspace: &Path,
    git_program: &Path,
    branch: &str,
) -> Result<Option<PathBuf>, LocalCompositionError> {
    let current = std::fs::canonicalize(workspace).map_err(|_| LocalCompositionError::Workspace)?;
    let listing = super::git_output(
        git_program,
        workspace,
        &["worktree", "list", "--porcelain", "-z"],
    )?;
    let reference = format!("branch refs/heads/{branch}");
    for record in listing.split("\0\0").filter(|record| !record.is_empty()) {
        let mut fields = record.split('\0');
        let path = fields
            .next()
            .and_then(|field| field.strip_prefix("worktree "))
            .ok_or(LocalCompositionError::ResolvedSource)?;
        let owner = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
        if fields.any(|field| field == reference) && owner != current {
            return Ok(Some(owner));
        }
    }
    Ok(None)
}
