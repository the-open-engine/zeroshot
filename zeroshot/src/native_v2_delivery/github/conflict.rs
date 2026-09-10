use std::path::{Component, Path};
use tokio::process::Command;
use tokio::time::timeout;

use super::*;

const MAX_CONFLICT_PATH_OUTPUT_BYTES: usize = 6 * 1_024;

#[derive(Clone, Copy)]
struct ConflictContext<'a> {
    authority: &'a GhCliDeliveryAuthority,
    request: &'a GitHubConflictRequest,
    credential: GitHubCredential<'a>,
}

pub(super) async fn materialize(
    authority: &GhCliDeliveryAuthority,
    request: &GitHubConflictRequest,
    credential: GitHubCredential<'_>,
) -> Result<GitHubConflictOutcome, GitHubAuthorityError> {
    let context = ConflictContext {
        authority,
        request,
        credential,
    };
    require_clean_review_head(context).await?;
    let target_revision = authority
        .target_revision(&request.review, credential)
        .await?;
    fetch_target(context, &target_revision).await?;
    require_commit(context, &target_revision).await?;
    let merge_status = merge_target(context, &target_revision).await?;
    let conflicted_paths = conflicted_paths(context).await?;
    if observation_changed(merge_status, &conflicted_paths) {
        restore_clean_review_head(context).await?;
        return Ok(GitHubConflictOutcome::ObservationChanged);
    }
    require_materialized_state(context, &target_revision, merge_status, &conflicted_paths).await?;
    Ok(GitHubConflictOutcome::Materialized(
        GitHubConflictMaterialization {
            target_revision,
            conflicted_paths,
        },
    ))
}

fn observation_changed(merge_status: i32, conflicted_paths: &[String]) -> bool {
    merge_status == 0 && conflicted_paths.is_empty()
}

async fn require_clean_review_head(
    context: ConflictContext<'_>,
) -> Result<(), GitHubAuthorityError> {
    let head = workspace_head(context).await?;
    let status = bounded_git_output(
        local_git_command(&context.authority.config, &context.request.workspace).args([
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ]),
        context.authority.config.api_deadline,
        1,
    )
    .await?;
    (head.trim() == context.request.review.head_revision && status.is_empty())
        .then_some(())
        .ok_or(GitHubAuthorityError::Rejected)
}

async fn fetch_target(
    context: ConflictContext<'_>,
    target_revision: &str,
) -> Result<(), GitHubAuthorityError> {
    let mut command = authenticated_git_command(
        &context.authority.config,
        &context.request.workspace,
        context.credential,
    );
    command.args([
        "fetch",
        "--no-tags",
        "--quiet",
        "--no-write-fetch-head",
        &format!(
            "https://github.com/{}.git",
            context.request.review.repository
        ),
        target_revision,
    ]);
    bounded_status(command, context.authority.config.push_deadline).await
}

async fn require_commit(
    context: ConflictContext<'_>,
    target_revision: &str,
) -> Result<(), GitHubAuthorityError> {
    let mut command = local_git_command(&context.authority.config, &context.request.workspace);
    command.args(["cat-file", "-e", &format!("{target_revision}^{{commit}}")]);
    bounded_status(command, context.authority.config.api_deadline).await
}

async fn merge_target(
    context: ConflictContext<'_>,
    target_revision: &str,
) -> Result<i32, GitHubAuthorityError> {
    let mut command = local_git_command(&context.authority.config, &context.request.workspace);
    command.args([
        "-c",
        "rerere.enabled=false",
        "-c",
        "user.name=Zeroshot",
        "-c",
        "user.email=delivery@zeroshot.invalid",
        "merge",
        "--no-commit",
        "--no-ff",
        "--no-edit",
        target_revision,
    ]);
    bounded_exit_code(command, context.authority.config.api_deadline).await
}

async fn conflicted_paths(
    context: ConflictContext<'_>,
) -> Result<Vec<String>, GitHubAuthorityError> {
    let output = bounded_git_output(
        local_git_command(&context.authority.config, &context.request.workspace).args([
            "diff",
            "--name-only",
            "--diff-filter=U",
            "-z",
        ]),
        context.authority.config.api_deadline,
        MAX_CONFLICT_PATH_OUTPUT_BYTES,
    )
    .await?;
    if !output.is_empty() && !output.ends_with('\0') {
        return Err(GitHubAuthorityError::Rejected);
    }
    output
        .split_terminator('\0')
        .map(|path| {
            let valid = !path.is_empty()
                && !Path::new(path).is_absolute()
                && Path::new(path)
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)));
            valid
                .then(|| path.to_owned())
                .ok_or(GitHubAuthorityError::Rejected)
        })
        .collect()
}

async fn require_materialized_state(
    context: ConflictContext<'_>,
    target_revision: &str,
    merge_status: i32,
    conflicted_paths: &[String],
) -> Result<(), GitHubAuthorityError> {
    require_merge_result(merge_status, conflicted_paths)?;
    require_unchanged_head(context).await?;
    require_merge_head(context, target_revision).await
}

fn require_merge_result(
    merge_status: i32,
    conflicted_paths: &[String],
) -> Result<(), GitHubAuthorityError> {
    if merge_status != 1 || conflicted_paths.is_empty() {
        return Err(GitHubAuthorityError::Rejected);
    }
    Ok(())
}

async fn require_unchanged_head(context: ConflictContext<'_>) -> Result<(), GitHubAuthorityError> {
    let head = workspace_head(context).await?;
    if head.trim() != context.request.review.head_revision {
        return Err(GitHubAuthorityError::Rejected);
    }
    Ok(())
}

async fn restore_clean_review_head(
    context: ConflictContext<'_>,
) -> Result<(), GitHubAuthorityError> {
    if merge_in_progress(context).await? {
        let mut command = local_git_command(&context.authority.config, &context.request.workspace);
        command.args(["merge", "--abort"]);
        bounded_status(command, context.authority.config.api_deadline).await?;
    }
    require_clean_review_head(context).await
}

async fn merge_in_progress(context: ConflictContext<'_>) -> Result<bool, GitHubAuthorityError> {
    let mut command = local_git_command(&context.authority.config, &context.request.workspace);
    command.args(["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
    match bounded_exit_code(command, context.authority.config.api_deadline).await? {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(GitHubAuthorityError::Rejected),
    }
}

async fn require_merge_head(
    context: ConflictContext<'_>,
    target_revision: &str,
) -> Result<(), GitHubAuthorityError> {
    let merge_head = bounded_git_output(
        local_git_command(&context.authority.config, &context.request.workspace).args([
            "rev-parse",
            "-q",
            "--verify",
            "MERGE_HEAD",
        ]),
        context.authority.config.api_deadline,
        128,
    )
    .await;
    merge_head
        .ok()
        .filter(|merge_head| merge_head.trim() == target_revision)
        .map(|_| ())
        .ok_or(GitHubAuthorityError::Rejected)
}

async fn workspace_head(context: ConflictContext<'_>) -> Result<String, GitHubAuthorityError> {
    bounded_git_output(
        local_git_command(&context.authority.config, &context.request.workspace)
            .args(["rev-parse", "HEAD"]),
        context.authority.config.api_deadline,
        128,
    )
    .await
}

async fn bounded_exit_code(
    mut command: Command,
    deadline: Duration,
) -> Result<i32, GitHubAuthorityError> {
    timeout(deadline, command.status())
        .await
        .map_err(|_| GitHubAuthorityError::Unavailable)?
        .map_err(|_| GitHubAuthorityError::Unavailable)?
        .code()
        .ok_or(GitHubAuthorityError::Rejected)
}

#[cfg(test)]
#[path = "conflict/tests.rs"]
mod tests;
