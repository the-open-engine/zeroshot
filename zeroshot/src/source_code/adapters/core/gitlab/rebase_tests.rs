//! GitLab `need_rebase` integration of a clean published review head through real Git.
use std::fs;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::native_v2_candidate::test_support::{
    TestGitRepository, commit_all, git, git_output, path_text,
};
use crate::native_v2_delivery::transport::GitTransportConfig;

/// Merge methods for which GitLab reports `need_rebase` until the source branch contains the
/// target: fast-forward and semi-linear (`rebase_merge`).
const REBASE_REQUIRED_METHODS: [&str; 2] = ["ff", "rebase_merge"];
const NO_MUTATION: &str = "no deliverable mutation";

struct RebaseFixture {
    gitlab: Fixture,
    repository: TestGitRepository,
    request: SourceConflictRequest,
    target_revision: String,
}

impl RebaseFixture {
    /// The run's change is committed and published as the MR head, so the workspace is clean and
    /// its HEAD equals the MR `sha`, as it is while delivery waits for forge merge policy.
    async fn new(merge_method: &str, teammate_commits_to_main: bool) -> Self {
        let repository = TestGitRepository::delivery();
        commit_all(&repository.workspace, "Fix the behavior");
        let head_revision = git_output(&repository.workspace, &["rev-parse", "HEAD"]);
        let target_revision = if teammate_commits_to_main {
            let teammate = repository.root.child("teammate");
            git(
                repository.root.path(),
                &["clone", path_text(&repository.remote), path_text(&teammate)],
            );
            fs::write(teammate.join("NOTES.md"), "unrelated notes\n").assert_value();
            commit_all(&teammate, "Add notes");
            git(&teammate, &["push", "origin", "main"]);
            git_output(&teammate, &["rev-parse", "HEAD"])
        } else {
            repository.base.clone()
        };
        let mut gitlab = Fixture::new().await;
        {
            let mut state = gitlab.state.lock().unwrap();
            state.project["merge_method"] = json!(merge_method);
            let mut review = merge_request();
            review["sha"] = json!(head_revision);
            review["head_pipeline"]["sha"] = json!(head_revision);
            review["detailed_merge_status"] = json!("need_rebase");
            review["has_conflicts"] = json!(false);
            state.review = Some(review);
        }
        gitlab.response(
            "GET",
            "/repository/branches/main",
            (
                StatusCode::OK,
                json!({"name":"main","commit":{"id":target_revision}}),
            ),
        );
        gitlab.authority.transport = Some(GitTransport::new(
            GitTransportConfig {
                git_program: "git".into(),
                workspace: repository.workspace.clone(),
                home_directory: repository.root.path().to_owned(),
                private_directory: repository.root.path().to_owned(),
                identity: None,
                local_environment: None,
                deadline: Duration::from_secs(20),
            },
            repository.remote.as_os_str(),
        ));
        let mut review = receipt();
        review.head_revision = head_revision;
        let request = SourceConflictRequest {
            workspace: repository.workspace.clone(),
            review,
        };
        Self {
            gitlab,
            repository,
            request,
            target_revision,
        }
    }

    async fn materialize(&self) -> Result<SourceConflictOutcome, SourceDeliveryError> {
        tokio::time::timeout(
            Duration::from_secs(60),
            self.gitlab
                .authority
                .materialize_merge_conflict(&self.request, credential()),
        )
        .await
        .assert_value()
    }

    fn contains(&self, ancestor: &str) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repository.workspace)
            .args(["merge-base", "--is-ancestor", ancestor, "HEAD"])
            .status()
            .assert_value()
            .success()
    }

    fn workspace_status(&self) -> String {
        git_output(
            &self.repository.workspace,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
    }
}

/// The API value as a whole word (so revision hex cannot match `ff`), or GitLab's display name.
fn names_merge_method(text: &str, method: &str) -> bool {
    let display = if method == "ff" {
        "fast-forward"
    } else {
        "semi-linear"
    };
    text.to_lowercase().contains(display)
        || text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|word| word == method)
}

#[tokio::test]
async fn need_rebase_without_conflicts_integrates_the_advanced_target_into_the_review_head() {
    for method in REBASE_REQUIRED_METHODS {
        let fixture = RebaseFixture::new(method, true).await;
        let head = fixture.request.review.head_revision.clone();
        let target = fixture.target_revision.clone();
        let error = fixture.materialize().await.expect_err(
            "a clean integration still changes the candidate and goes back to the worker",
        );
        let text = error.to_string();
        assert!(!text.contains(NO_MUTATION), "{method}: {text}");
        assert!(
            matches!(error, SourceDeliveryError::Repairable(_)),
            "{method}: {error:?}"
        );
        assert!(text.contains(&target), "{method}: {text}");
        assert!(fixture.contains(&head), "{method}: review head was lost");
        assert!(
            fixture.contains(&target),
            "{method}: target {target} was not integrated"
        );
        assert!(fixture.repository.workspace.join("NOTES.md").is_file());
        assert!(fixture.repository.workspace.join("result.txt").is_file());
        assert_eq!(fixture.workspace_status(), "", "{method}");
        assert_eq!(fixture.gitlab.writes("/merge"), 0, "{method}");
    }
}

#[tokio::test]
async fn need_rebase_refusal_names_the_merge_method_and_captured_target() {
    for method in REBASE_REQUIRED_METHODS {
        // GitLab still reports need_rebase although the clean candidate contains the target.
        let fixture = RebaseFixture::new(method, false).await;
        let head = fixture.request.review.head_revision.clone();
        let target = fixture.target_revision.clone();
        let error = fixture
            .materialize()
            .await
            .expect_err("integration is impossible");
        let text = error.to_string();
        assert!(
            !matches!(
                error,
                SourceDeliveryError::Repairable(_) | SourceDeliveryError::Command(_)
            ) && !error.retryable_operation(),
            "{method}: must not be routed to repair or retried: {error:?}"
        );
        assert!(!text.contains(NO_MUTATION), "{method}: {text}");
        assert!(text.contains(&target), "{method}: {text}");
        assert!(
            names_merge_method(&text, method),
            "{method}: merge method is not named: {text}"
        );
        assert_eq!(
            git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]),
            head,
            "{method}"
        );
        assert_eq!(fixture.workspace_status(), "", "{method}");
        assert_eq!(fixture.gitlab.writes("/merge"), 0, "{method}");
    }
}
