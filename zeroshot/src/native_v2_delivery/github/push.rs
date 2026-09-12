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
        .arg(format!("HEAD:refs/heads/{}", request.head_branch));
    match capture(&mut command, authority.config.push_deadline)
        .await
        .and_then(GitCommandFailure::require_success)
    {
        Ok(_) => Ok(()),
        Err(failure) => {
            authority.record_push_failure(&failure);
            // A transport failure may follow an accepted push. Observe the exact remote ref before repair.
            if authority
                .confirm_pushed_head(request, credential)
                .await
                .is_ok()
            {
                return Ok(());
            }
            Err(failure.into())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use crate::native_v2_target_authority::MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES;
    const REDACTED: &str = "[REDACTED]";

    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::sync::Arc;
    #[cfg(unix)]
    use crate::native_v2_delivery::git_auth::encode_basic_credential;

    #[cfg(unix)]
    use openengine_cluster_protocol::RunId;
    #[cfg(unix)]
    use openengine_cluster_testkit::assertions::AssertValue;

    #[cfg(unix)]
    use crate::native_v2_candidate::test_support::TestGitRepository;
    #[cfg(unix)]
    use crate::native_v2_delivery::GhCliAuthorityConfig;
    #[cfg(unix)]
    use crate::native_v2_target_authority::OperatorDiagnosticStore;

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_push_records_bounded_redacted_stdout_and_stderr() {
        let repository = TestGitRepository::delivery();
        let git_program = executable(
            repository.root.path(),
            "failing-git",
            r#"#!/bin/sh
/usr/bin/printf 'stdout-safe-ref=refs/heads/rejected\n'
/usr/bin/printf 'stdout-token=%s\n' "$GH_TOKEN"
/usr/bin/printf 'stdout-auth=%s\n' "$GIT_CONFIG_VALUE_1"
for argument in "$@"; do /usr/bin/printf 'arg=%s\n' "$argument"; done
/usr/bin/head -c 131072 /dev/zero | /usr/bin/tr '\000' x
/usr/bin/printf 'stderr-safe=remote rejected update\n' >&2
/usr/bin/printf 'stderr-token=%s\n' "$GH_TOKEN" >&2
/usr/bin/printf 'stderr-auth=%s\n' "$GIT_CONFIG_VALUE_1" >&2
/usr/bin/head -c 131072 /dev/zero | /usr/bin/tr '\000' y >&2
exit 17
"#,
        );
        let store = Arc::new(OperatorDiagnosticStore::default());
        let run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
        let authority =
            diagnostic_authority(&repository, git_program, run_id.clone(), store.clone());
        let request = push_request(&repository);
        let token = "raw-github-token";
        let basic = encode_basic_credential(token);

        assert!(matches!(
            push_branch(&authority, &request, GitHubCredential(token)).await,
            Err(GitHubAuthorityError::Command(_))
        ));

        let snapshot = store.snapshot(&run_id);
        assert_eq!(snapshot.diagnostics.len(), 1);
        let diagnostic = &snapshot.diagnostics[0];
        assert_eq!(diagnostic.code, "git_push_failed");
        assert_eq!(diagnostic.operation, "delivery.git_push");
        assert_eq!(diagnostic.exit_status, Some(17));
        assert!(diagnostic.stdout_truncated);
        assert!(diagnostic.stderr_truncated);
        assert!(diagnostic.stdout.len() <= MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES);
        assert!(diagnostic.stderr.len() <= MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES);
        assert!(
            diagnostic
                .stdout
                .contains("stdout-safe-ref=refs/heads/rejected")
        );
        assert!(
            diagnostic
                .stderr
                .contains("stderr-safe=remote rejected update")
        );
        assert!(
            diagnostic
                .stdout
                .contains("arg=https://github.com/acme/project.git")
        );
        assert!(diagnostic.stdout.contains(REDACTED));
        assert!(diagnostic.stderr.contains(REDACTED));
        assert!(!diagnostic.stdout.contains(token));
        assert!(!diagnostic.stderr.contains(token));
        assert!(!diagnostic.stdout.contains(&basic));
        assert!(!diagnostic.stderr.contains(&basic));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_push_does_not_record_a_diagnostic() {
        let repository = TestGitRepository::delivery();
        let store = Arc::new(OperatorDiagnosticStore::default());
        let run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
        let authority = diagnostic_authority(
            &repository,
            "/usr/bin/true".into(),
            run_id.clone(),
            store.clone(),
        );

        push_branch(
            &authority,
            &push_request(&repository),
            GitHubCredential("raw-github-token"),
        )
        .await
        .assert_value();

        assert!(store.snapshot(&run_id).diagnostics.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ambiguous_push_requires_an_exact_remote_head_observation() {
        let repository = TestGitRepository::delivery();
        let request = push_request(&repository);
        for revision in [
            &request.head_revision,
            &"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        ] {
            let body = serde_json::json!({
                "ref": format!("refs/heads/{}", request.head_branch),
                "object": { "type": "commit", "sha": revision }
            });
            let program = executable(
                repository.root.path(),
                "observed-head",
                &format!("#!/bin/sh\nprintf '%s' '{}'\n", body),
            );
            let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
                git_program: "/usr/bin/false".into(),
                gh_program: program,
                home_directory: repository.root.path().to_owned(),
                api_deadline: Duration::from_secs(1),
                push_deadline: Duration::from_secs(1),
            });
            assert_eq!(
                push_branch(&authority, &request, GitHubCredential("test-token"))
                    .await
                    .is_ok(),
                revision == &request.head_revision
            );
        }
    }

    #[cfg(unix)]
    fn push_request(repository: &TestGitRepository) -> GitHubPushRequest {
        let review = super::super::test_review_request();
        GitHubPushRequest {
            workspace: repository.workspace.clone(),
            target: review.target,
            head_branch: review.head_branch,
            head_revision: review.head_revision,
        }
    }

    #[cfg(unix)]
    fn diagnostic_authority(
        repository: &TestGitRepository,
        git_program: std::path::PathBuf,
        run_id: RunId,
        store: Arc<OperatorDiagnosticStore>,
    ) -> GhCliDeliveryAuthority {
        GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            git_program,
            gh_program: "/usr/bin/false".into(),
            home_directory: repository.root.path().to_owned(),
            api_deadline: Duration::from_secs(10),
            push_deadline: Duration::from_secs(10),
        })
        .with_operator_diagnostics(run_id, store)
    }

    #[cfg(unix)]
    fn executable(directory: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = directory.join(name);
        fs::write(&path, contents).assert_value();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).assert_value();
        path
    }
}
