use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::native_v2_candidate::test_support::{TestGitRepository, git, git_output};

const OLD_HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NEW_HEAD: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn review() -> GitHubReviewReceipt {
    GitHubReviewReceipt {
        review_id: "17".to_owned(),
        repository: "acme/project".to_owned(),
        target_branch: "main".to_owned(),
        head_branch: "zeroshot/v2-run".to_owned(),
        head_revision: OLD_HEAD.to_owned(),
    }
}

fn response(overrides: impl FnOnce(&mut Value)) -> Value {
    let mut value = json!({
        "data": {
            "updatePullRequestBranch": {
                "pullRequest": {
                    "id": "PR_node_17",
                    "number": 17,
                    "repository": {"nameWithOwner": "acme/project"},
                    "baseRefName": "main",
                    "headRefName": "zeroshot/v2-run",
                    "headRefOid": NEW_HEAD
                }
            }
        }
    });
    overrides(&mut value);
    value
}

#[test]
fn cas_update_arguments_pin_the_expected_head() {
    let arguments = update_arguments("PR_node_17", OLD_HEAD);
    let query = arguments
        .iter()
        .find(|argument| argument.starts_with("query="))
        .assert_value();
    assert!(query.contains("updatePullRequestBranch"));
    assert!(query.contains("updateMethod: MERGE"));
    assert!(
        arguments
            .iter()
            .any(|value| value == "pullRequestId=PR_node_17")
    );
    assert!(
        arguments
            .iter()
            .any(|value| value == &format!("expectedHeadOid={OLD_HEAD}"))
    );
}

#[test]
fn mutation_response_authorizes_only_one_exact_head_transition() {
    let updated = updated_receipt(response(|_| {}), &review(), "PR_node_17").assert_value();
    assert_eq!(updated.head_revision, NEW_HEAD);

    for pointer in [
        "/data/updatePullRequestBranch/pullRequest/id",
        "/data/updatePullRequestBranch/pullRequest/repository/nameWithOwner",
        "/data/updatePullRequestBranch/pullRequest/baseRefName",
        "/data/updatePullRequestBranch/pullRequest/headRefName",
    ] {
        let invalid = response(|value| {
            *value.pointer_mut(pointer).assert_value() = json!("changed");
        });
        assert!(updated_receipt(invalid, &review(), "PR_node_17").is_err());
    }
}

#[test]
fn mutation_response_rejects_missing_invalid_or_unchanged_heads() {
    for head in [OLD_HEAD, "not-a-revision"] {
        let invalid = response(|value| {
            *value
                .pointer_mut("/data/updatePullRequestBranch/pullRequest/headRefOid")
                .assert_value() = json!(head);
        });
        assert!(updated_receipt(invalid, &review(), "PR_node_17").is_err());
    }
    assert!(updated_receipt(json!({"data": {}}), &review(), "PR_node_17").is_err());
}

#[tokio::test]
async fn fetched_head_adoption_accepts_only_the_authorized_transition_and_its_retry() {
    let repository = TestGitRepository::candidate();
    let workspace = &repository.workspace;
    git(
        workspace,
        &[
            "-c",
            "user.name=GitHub",
            "-c",
            "user.email=noreply@github.com",
            "commit",
            "--allow-empty",
            "--message",
            "previous",
        ],
    );
    let previous_head = git_output(workspace, &["rev-parse", "HEAD"]);
    git(
        workspace,
        &[
            "-c",
            "user.name=GitHub",
            "-c",
            "user.email=noreply@github.com",
            "commit",
            "--allow-empty",
            "--message",
            "updated",
        ],
    );
    let updated_head = git_output(workspace, &["rev-parse", "HEAD"]);
    let mut previous = review();
    previous.head_revision = previous_head.clone();
    let mut updated = previous.clone();
    updated.head_revision = updated_head.clone();
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        git_program: PathBuf::from("/usr/bin/git"),
        gh_program: PathBuf::from("/usr/bin/false"),
        home_directory: repository.root.path().to_path_buf(),
        api_deadline: Duration::from_secs(5),
        push_deadline: Duration::from_secs(5),
    });
    let context = HeadUpdateContext {
        authority: &authority,
        workspace,
        credential: GitHubCredential("test-token"),
    };

    git(workspace, &["reset", "--hard", &repository.base]);
    assert_eq!(
        adopt_fetched_head(context, &previous, &updated).await,
        Err(GitHubAuthorityError::Rejected)
    );

    git(workspace, &["reset", "--hard", &previous_head]);
    adopt_fetched_head(context, &previous, &updated)
        .await
        .assert_value();
    assert_eq!(git_output(workspace, &["rev-parse", "HEAD"]), updated_head);
    adopt_fetched_head(context, &previous, &updated)
        .await
        .assert_value();
}
