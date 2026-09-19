use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::*;
use crate::native_v2_candidate::test_support::{TestGitRepository, commit_all, git, git_output};

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
        git_program: PathBuf::from("git"),
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
    let mismatch = adopt_fetched_head(context, &previous, &updated)
        .await
        .assert_error()
        .to_string();
    assert!(mismatch.contains(&format!("expected HEAD {previous_head}")));
    assert!(mismatch.contains(&format!("actual HEAD {}", repository.base)));

    git(workspace, &["reset", "--hard", &previous_head]);
    let private_filename = workspace.join("dirty-test-token.txt");
    std::fs::write(&private_filename, "keep work").assert_value();
    let dirty = adopt_fetched_head(context, &previous, &updated)
        .await
        .assert_error()
        .to_string();
    assert!(dirty.contains("git status --porcelain=v1"));
    assert!(dirty.contains("?? dirty-[REDACTED].txt"));
    assert!(!dirty.contains("test-token"));
    std::fs::remove_file(private_filename).assert_value();
    adopt_fetched_head(context, &previous, &updated)
        .await
        .assert_value();
    assert_eq!(git_output(workspace, &["rev-parse", "HEAD"]), updated_head);
    adopt_fetched_head(context, &previous, &updated)
        .await
        .assert_value();
}

struct SeparateHistory {
    repository: TestGitRepository,
    external: PathBuf,
    published: GitHubReviewReceipt,
    authority: GhCliDeliveryAuthority,
}

impl SeparateHistory {
    fn new() -> Self {
        let repository = TestGitRepository::candidate();
        std::fs::write(repository.workspace.join("result.txt"), "candidate\n").assert_value();
        commit_all(&repository.workspace, "candidate");
        let mut published = review();
        published.head_revision = git_output(&repository.workspace, &["rev-parse", "HEAD"]);
        git(
            &repository.workspace,
            &["push", "origin", "HEAD:refs/heads/zeroshot/v2-run"],
        );
        let external = repository.root.child("external");
        git(
            repository.root.path(),
            &[
                "clone",
                "--branch",
                "zeroshot/v2-run",
                repository.remote.to_str().assert_value(),
                external.to_str().assert_value(),
            ],
        );
        git(
            &repository.workspace,
            &[
                "config",
                &format!("url.{}.insteadOf", repository.remote.display()),
                "https://github.com/acme/project.git",
            ],
        );
        let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            git_program: "git".into(),
            gh_program: "/usr/bin/false".into(),
            home_directory: repository.root.path().to_owned(),
            api_deadline: Duration::from_secs(5),
            push_deadline: Duration::from_secs(5),
        });
        Self {
            repository,
            external,
            published,
            authority,
        }
    }

    fn advance_remote(&self, name: &str, contents: &str) -> GitHubReviewReceipt {
        std::fs::write(self.external.join(name), contents).assert_value();
        commit_all(&self.external, "remote update");
        git(
            &self.external,
            &["push", "origin", "HEAD:refs/heads/zeroshot/v2-run"],
        );
        let mut observed = self.published.clone();
        observed.head_revision = git_output(&self.external, &["rev-parse", "HEAD"]);
        observed
    }

    fn rewrite_remote(&self) -> GitHubReviewReceipt {
        git(&self.external, &["reset", "--hard", &self.repository.base]);
        std::fs::write(self.external.join("rewritten.txt"), "rewritten\n").assert_value();
        commit_all(&self.external, "human rewrite");
        git(
            &self.external,
            &[
                "push",
                "--force",
                "origin",
                "HEAD:refs/heads/zeroshot/v2-run",
            ],
        );
        let mut observed = self.published.clone();
        observed.head_revision = git_output(&self.external, &["rev-parse", "HEAD"]);
        observed
    }

    async fn reconcile(
        &self,
        observed: &GitHubReviewReceipt,
        authorized: bool,
    ) -> GitHubReconciliationOutcome {
        self.reconcile_with_mode(observed, authorized, false).await
    }

    async fn reconcile_with_mode(
        &self,
        observed: &GitHubReviewReceipt,
        authorized: bool,
        adopting_existing: bool,
    ) -> GitHubReconciliationOutcome {
        super::reconcile(
            &self.authority,
            GitHubHeadReconciliation {
                workspace: &self.repository.workspace,
                published: &self.published,
                observed,
                commit_message: "Preserve repair changes",
                authorized_update: authorized,
                adopting_existing,
            },
            GitHubCredential("test-token"),
        )
        .await
        .assert_value()
    }
}

#[tokio::test]
async fn real_reconciliation_fetches_remote_objects_before_needs_work() {
    let history = SeparateHistory::new();
    let observed = history.advance_remote("remote.txt", "remote work\n");
    let before = std::process::Command::new("git")
        .arg("-C")
        .arg(&history.repository.workspace)
        .args(["cat-file", "-e", &observed.head_revision])
        .status()
        .assert_value();
    assert!(
        !before.success(),
        "the remote commit must not already exist locally"
    );
    assert!(matches!(
        history.reconcile(&observed, false).await,
        GitHubReconciliationOutcome::NeedsWork(_)
    ));
    assert_eq!(
        git_output(&history.repository.workspace, &["rev-parse", "HEAD"]),
        observed.head_revision
    );
    assert_eq!(
        std::fs::read_to_string(history.repository.workspace.join("remote.txt")).assert_value(),
        "remote work\n"
    );
}

#[tokio::test]
async fn real_reconciliation_preserves_dirty_repairs_and_local_commits() {
    for committed in [false, true] {
        let history = SeparateHistory::new();
        let observed = history.advance_remote("remote.txt", "remote work\n");
        std::fs::write(
            history.repository.workspace.join("repair.txt"),
            "repair work\n",
        )
        .assert_value();
        if committed {
            commit_all(&history.repository.workspace, "agent repair");
        }
        let before = git_output(&history.repository.workspace, &["rev-parse", "HEAD"]);
        assert!(matches!(
            history.reconcile(&observed, true).await,
            GitHubReconciliationOutcome::NeedsWork(_)
        ));
        git(
            &history.repository.workspace,
            &["merge-base", "--is-ancestor", &before, "HEAD"],
        );
        git(
            &history.repository.workspace,
            &[
                "merge-base",
                "--is-ancestor",
                &observed.head_revision,
                "HEAD",
            ],
        );
        assert_eq!(
            std::fs::read_to_string(history.repository.workspace.join("repair.txt")).assert_value(),
            "repair work\n"
        );
    }
}

#[tokio::test]
async fn real_reconciliation_materializes_conflict_without_discarding_repairs() {
    let history = SeparateHistory::new();
    let observed = history.advance_remote("result.txt", "remote replacement\n");
    std::fs::write(
        history.repository.workspace.join("result.txt"),
        "local repair\n",
    )
    .assert_value();
    let outcome = history.reconcile(&observed, false).await;
    let GitHubReconciliationOutcome::NeedsWork(diagnostic) = outcome else {
        panic!("expected conflict");
    };
    assert!(diagnostic.contains("CONFLICT"));
    assert!(
        !git_output(
            &history.repository.workspace,
            &["diff", "--name-only", "--diff-filter=U"]
        )
        .is_empty()
    );
    assert_eq!(
        git_output(&history.repository.workspace, &["rev-parse", "MERGE_HEAD"]),
        observed.head_revision
    );
    assert_eq!(
        git_output(&history.repository.workspace, &["show", "HEAD:result.txt"]),
        "local repair"
    );
}

#[tokio::test]
async fn real_reconciliation_refuses_rewritten_remote_without_changing_local_work() {
    let history = SeparateHistory::new();
    let observed = history.rewrite_remote();
    assert_refusal_preserves_repairs(&history, &observed, false).await;
}

#[tokio::test]
async fn adopted_reconciliation_refuses_divergent_remote_without_changing_local_work() {
    let history = SeparateHistory::new();
    let observed = history.rewrite_remote();
    assert_refusal_preserves_repairs(&history, &observed, true).await;
}

#[tokio::test]
async fn real_reconciliation_preserves_the_authorized_clean_fast_forward_path() {
    let history = SeparateHistory::new();
    let observed = history.advance_remote("remote.txt", "base update\n");
    assert_eq!(
        history.reconcile(&observed, true).await,
        GitHubReconciliationOutcome::Adopted
    );
    assert_eq!(
        history.reconcile(&observed, true).await,
        GitHubReconciliationOutcome::Unchanged
    );
}

#[tokio::test]
async fn real_reconciliation_refuses_lost_local_anchor_even_when_remote_is_unchanged() {
    let history = SeparateHistory::new();
    git(
        &history.repository.workspace,
        &["reset", "--hard", &history.repository.base],
    );
    assert_refusal_preserves_repairs(&history, &history.published, false).await;
}

async fn assert_refusal_preserves_repairs(
    history: &SeparateHistory,
    observed: &GitHubReviewReceipt,
    adopting_existing: bool,
) {
    std::fs::write(
        history.repository.workspace.join("repair.txt"),
        "preserve\n",
    )
    .assert_value();
    let before = git_output(&history.repository.workspace, &["rev-parse", "HEAD"]);
    assert!(matches!(
        history
            .reconcile_with_mode(observed, false, adopting_existing)
            .await,
        GitHubReconciliationOutcome::Refused(_)
    ));
    assert_eq!(
        git_output(&history.repository.workspace, &["rev-parse", "HEAD"]),
        before
    );
    assert_eq!(
        std::fs::read_to_string(history.repository.workspace.join("repair.txt")).assert_value(),
        "preserve\n"
    );
}
