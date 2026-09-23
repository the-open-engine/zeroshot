use crate::native_v2_delivery::command::{capture, GitCommandFailure};
use crate::native_v2_target_authority::NewOperatorDiagnostic;

use super::{
    GhCliDeliveryAuthority, GitHubAuthorityError, GitHubCredential, GitHubPushRequest,
    authenticated_git_command,
};

pub(super) async fn push_branch(
    authority: &GhCliDeliveryAuthority,
    request: &GitHubPushRequest,
    credential: GitHubCredential<'_>,
) -> Result<(), GitHubAuthorityError> {
    let mut command = authenticated_git_command(&authority.config, &request.workspace, credential);
    command
        .arg("push")
        .arg("--porcelain")
        .arg("--no-verify")
        .arg(format!(
            "https://github.com/{}.git",
            request.target.repository
        ))
        .arg(format!(
            "{}:refs/heads/{}",
            request.head_revision, request.head_branch
        ));
    match capture(&mut command, authority.config.push_deadline)
        .await
        .and_then(GitCommandFailure::require_success)
    {
        Ok(_) => authority.confirm_pushed_head(request, credential).await,
        Err(failure) => {
            authority.record_push_failure(&failure);
            // A transport failure may follow an accepted push. Observe the exact remote ref before repair.
            match authority.confirm_pushed_head(request, credential).await {
                Ok(()) => Ok(()),
                Err(error) if error.authentication_failed() || error.retryable_operation() => {
                    Err(error.with_context(failure))
                }
                Err(error) => Err(failure
                    .with_context(format!("push confirmation failed: {error}"))
                    .into()),
            }
        }
    }
}

impl GhCliDeliveryAuthority {
    fn record_push_failure(&self, diagnostic: &GitCommandFailure) {
        let Some(reporter) = &self.operator_diagnostics else {
            return;
        };
        reporter.store.record(NewOperatorDiagnostic {
            run_id: reporter.run_id.clone(),
            code: "git_push_failed",
            operation: "delivery.git_push",
            exit_status: diagnostic.exit_status,
            stdout: diagnostic.stdout.clone(),
            stderr: diagnostic.stderr.clone(),
            stdout_truncated: diagnostic.stdout_truncated,
            stderr_truncated: diagnostic.stderr_truncated,
        });
    }
}

#[cfg(all(test, unix))]
#[path = "push/tests.rs"]
mod tests;
