use std::{fs, path::PathBuf, time::Duration};

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::native_v2_candidate::test_support::{
    TestGitRepository, commit_all, git, git_output, path_text,
};
use crate::native_v2_delivery::DeliveryTarget;

struct TargetFixture {
    repository: TestGitRepository,
    external: PathBuf,
    authority: GhCliDeliveryAuthority,
    target_revision: String,
    git_program: PathBuf,
}

impl TargetFixture {
    fn new() -> Self {
        let mut repository = TestGitRepository::candidate();
        fs::create_dir_all(repository.workspace.join(".github/workflows")).assert_value();
        fs::write(
            repository.workspace.join(".github/workflows/ci.yml"),
            "old workflow\n",
        )
        .assert_value();
        commit_all(&repository.workspace, "original workflow");
        git(&repository.workspace, &["push", "origin", "main"]);
        repository.base = git_output(&repository.workspace, &["rev-parse", "HEAD"]);
        let external = repository.root.child("external");
        git(
            repository.root.path(),
            &["clone", path_text(&repository.remote), path_text(&external)],
        );
        fs::write(repository.workspace.join("result.txt"), "feature work\n").assert_value();
        let git_program = git_wrapper(&repository);
        let gh_program = repository.root.child("gh-target-ref");
        let mut config = GhCliAuthorityConfig::hosted(repository.root.path().to_owned());
        config.git_program = git_program.clone();
        config.gh_program = gh_program;
        config.api_deadline = Duration::from_secs(10);
        config.push_deadline = config.api_deadline;
        let authority = GhCliDeliveryAuthority::new(config);
        let mut fixture = Self {
            repository,
            external,
            authority,
            target_revision: String::new(),
            git_program,
        };
        fs::write(
            fixture.repository.root.child("queue-policy.json"),
            r#"{"data":{"repository":{"nameWithOwner":"acme/project","mergeQueue":null}}}"#,
        )
        .assert_value();
        fixture.advance_target(".github/workflows/ci.yml", "new workflow\n");
        fixture
    }

    fn advance_target(&mut self, path: &str, contents: &str) {
        fs::write(self.external.join(path), contents).assert_value();
        commit_all(&self.external, "upstream change");
        git(&self.external, &["push", "origin", "HEAD:main"]);
        self.capture_target();
    }

    fn capture_target(&mut self) {
        self.target_revision = git_output(&self.external, &["rev-parse", "HEAD"]);
        let reference = serde_json::json!({
            "ref": "refs/heads/main",
            "object": { "sha": self.target_revision, "type": "commit" },
        });
        self.repository.root.write_executable(
            "gh-target-ref",
            &format!("#!/bin/sh\ncase \"$*\" in\n*mergeQueue*) /usr/bin/cat '{}' ;;\n*) /usr/bin/printf '%s\\n' '{reference}' ;;\nesac\n", self.repository.root.child("queue-policy.json").display()),
        );
    }

    async fn reconcile(&self) -> (String, SourceReconciliationOutcome) {
        let SourceTargetIntegration::Integrated {
            target_revision,
            outcome,
        } = self.try_reconcile().await.assert_value()
        else {
            panic!("ordinary target must be integrated");
        };
        (target_revision, outcome)
    }

    async fn try_reconcile(&self) -> Result<SourceTargetIntegration, SourceDeliveryError> {
        let target =
            DeliveryTarget::new("acme/project", "main", &self.repository.base).assert_value();
        self.authority
            .reconcile_delivery_target(
                SourceTargetReconciliation {
                    workspace: &self.repository.workspace,
                    target: &target,
                    commit_message: "feat: preserve candidate",
                },
                SourceDeliveryCredential::token("test-token"),
            )
            .await
    }
}

#[tokio::test]
async fn exact_target_integration_preserves_upstream_workflow_and_dirty_candidate() {
    let mut fixture = TargetFixture::new();
    let captured = fixture.target_revision.clone();
    // The branch can advance after observation; fetch and merge must still use the captured SHA.
    fs::write(fixture.external.join("later.txt"), "later upstream work\n").assert_value();
    commit_all(&fixture.external, "later upstream change");
    git(&fixture.external, &["push", "origin", "main"]);
    let source = fixture.repository.base.clone();
    let outcome = fixture.reconcile().await;
    assert_eq!(outcome.0, captured);
    assert!(matches!(
        outcome.1,
        SourceReconciliationOutcome::NeedsWork(_)
    ));
    assert_eq!(
        fs::read_to_string(
            fixture
                .repository
                .workspace
                .join(".github/workflows/ci.yml")
        )
        .assert_value(),
        "new workflow\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.repository.workspace.join("result.txt")).assert_value(),
        "feature work\n"
    );
    assert!(!fixture.repository.workspace.join("later.txt").exists());
    assert_eq!(fixture.repository.base, source);
    git(
        &fixture.repository.workspace,
        &["merge-base", "--is-ancestor", &captured, "HEAD"],
    );
    git(
        &fixture.repository.workspace,
        &["merge-base", "--is-ancestor", &source, "HEAD"],
    );
    assert_transport_is_scoped(&fixture);
    // Keep a separate explicit subsequent observation to prove the fixture's branch did advance.
    fixture.capture_target();
    assert_ne!(fixture.target_revision, captured);
}

#[tokio::test]
async fn target_integration_completes_despite_inherited_merge_preferences() {
    for (key, value) in [
        ("branch.main.mergeOptions", "--squash --no-commit"),
        ("branch.main.mergeOptions", "--ff-only"),
        ("merge.ff", "only"),
    ] {
        let fixture = TargetFixture::new();
        let workspace = &fixture.repository.workspace;
        git(workspace, &["config", key, value]);

        let integration = fixture.reconcile().await;

        assert!(matches!(
            integration.1,
            SourceReconciliationOutcome::NeedsWork(_)
        ));
        for revision in [&fixture.repository.base, &fixture.target_revision] {
            git(
                workspace,
                &["merge-base", "--is-ancestor", revision, "HEAD"],
            );
        }
        assert_eq!(git_output(workspace, &["status", "--porcelain=v1"]), "");
        assert_eq!(
            fs::read_to_string(workspace.join(".github/workflows/ci.yml")).assert_value(),
            "new workflow\n"
        );
        assert_eq!(
            fs::read_to_string(workspace.join("result.txt")).assert_value(),
            "feature work\n"
        );
        assert_eq!(git_output(workspace, &["config", key]), value);
        assert_eq!(
            fixture.reconcile().await.1,
            SourceReconciliationOutcome::Unchanged,
            "completed integration must progress beyond the review loop"
        );
    }
}

#[tokio::test]
async fn configured_signing_failure_preserves_policy_and_work_for_repair() {
    for committed_candidate in [false, true] {
        let fixture = TargetFixture::new();
        let workspace = &fixture.repository.workspace;
        if committed_candidate {
            commit_all(workspace, "candidate before signing policy");
        }
        let candidate = git_output(workspace, &["rev-parse", "HEAD"]);
        let signer = fixture.repository.root.write_executable(
            "unavailable-signer",
            "#!/bin/sh\n/usr/bin/cat >/dev/null\n\
             /usr/bin/printf 'fixture signer unavailable\\n' >&2\nexit 1\n",
        );
        for (key, value) in [
            ("commit.gpgSign", "true"),
            ("gpg.format", "openpgp"),
            ("gpg.program", path_text(&signer)),
        ] {
            git(workspace, &["config", "--local", key, value]);
        }

        let error = fixture
            .try_reconcile()
            .await
            .expect_err("authored signing policy must not be silently bypassed");

        assert!(matches!(&error, SourceDeliveryError::Command(_)));
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains("fixture signer unavailable"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("exitStatus: Some("), "{diagnostic}");
        assert!(!diagnostic.contains("test-token"));
        assert_eq!(git_output(workspace, &["rev-parse", "HEAD"]), candidate);
        assert_eq!(
            fs::read_to_string(workspace.join("result.txt")).assert_value(),
            "feature work\n"
        );
        assert_eq!(
            fs::read_to_string(workspace.join(".github/workflows/ci.yml")).assert_value(),
            if committed_candidate {
                "new workflow\n"
            } else {
                "old workflow\n"
            }
        );
        for (key, value) in [
            ("commit.gpgSign", "true"),
            ("gpg.format", "openpgp"),
            ("gpg.program", path_text(&signer)),
        ] {
            assert_eq!(git_output(workspace, &["config", "--local", key]), value);
        }
        assert_eq!(
            workspace.join(".git/MERGE_HEAD").exists(),
            committed_candidate
        );
        if committed_candidate {
            assert_eq!(
                git_output(workspace, &["rev-parse", "MERGE_HEAD"]),
                fixture.target_revision
            );
            assert_eq!(git_output(workspace, &["ls-files", "--unmerged"]), "");
        }
        assert_transport_is_scoped(&fixture);
    }
}

#[tokio::test]
async fn successful_merge_exit_must_prove_both_ancestries_and_completion() {
    for failure in ["no_merge", "discard_candidate", "leave_merge_pending"] {
        let fixture = TargetFixture::new();
        let command = match failure {
            "no_merge" => "exit 0".to_owned(),
            "discard_candidate" => format!(
                "exec /usr/bin/git -C '{}' reset --hard '{}'",
                path_text(&fixture.repository.workspace),
                fixture.target_revision,
            ),
            "leave_merge_pending" => "exec /usr/bin/git \"${arguments[@]}\" --no-commit".to_owned(),
            _ => unreachable!(),
        };
        let wrapper = fs::read_to_string(&fixture.git_program).assert_value();
        let wrapper = wrapper.replace(
            "exec /usr/bin/git \"${arguments[@]}\"",
            &format!(
                r#"for argument in "${{arguments[@]}}"; do
  if [[ "$argument" == 'merge' ]]; then
    {command}
  fi
done
exec /usr/bin/git "${{arguments[@]}}""#
            ),
        );
        openengine_cluster_testkit::fixture::write_executable(&fixture.git_program, wrapper, 0o700)
            .assert_value();

        let error = fixture
            .try_reconcile()
            .await
            .expect_err("exit status alone must not certify target integration");

        assert!(
            error
                .to_string()
                .contains("without a clean, completed integration"),
            "{failure}: {error}"
        );
    }
}

#[tokio::test]
async fn an_already_integrated_target_does_not_commit_new_dirty_work() {
    let fixture = TargetFixture::new();
    assert!(matches!(
        fixture.reconcile().await.1,
        SourceReconciliationOutcome::NeedsWork(_)
    ));
    let head = git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]);
    fs::write(
        fixture.repository.workspace.join("unreviewed.txt"),
        "leave dirty\n",
    )
    .assert_value();
    assert_eq!(
        fixture.reconcile().await.1,
        SourceReconciliationOutcome::Unchanged
    );
    assert_eq!(
        git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]),
        head
    );
    assert_eq!(
        git_output(&fixture.repository.workspace, &["status", "--porcelain=v1"]),
        "?? unreviewed.txt"
    );
}

#[tokio::test]
async fn divergent_target_changes_leave_real_conflicts_and_delivery_identity() {
    let mut fixture = TargetFixture::new();
    fixture.advance_target("README.md", "upstream readme\n");
    fs::write(
        fixture.repository.workspace.join("README.md"),
        "worker readme\n",
    )
    .assert_value();
    git(
        &fixture.repository.workspace,
        &["config", "user.name", "Worker"],
    );
    git(
        &fixture.repository.workspace,
        &["config", "user.email", "worker@example.invalid"],
    );
    let outcome = fixture.reconcile().await;
    let SourceReconciliationOutcome::NeedsWork(diagnostic) = outcome.1 else {
        panic!("expected conflict repair");
    };
    assert!(diagnostic.contains("CONFLICT"));
    assert_eq!(
        git_output(
            &fixture.repository.workspace,
            &["diff", "--name-only", "--diff-filter=U"]
        ),
        "README.md"
    );
    assert_eq!(
        git_output(&fixture.repository.workspace, &["rev-parse", "MERGE_HEAD"]),
        fixture.target_revision
    );
    assert_eq!(
        git_output(&fixture.repository.workspace, &["show", "HEAD:README.md"]),
        "worker readme"
    );
    assert_eq!(
        git_output(&fixture.repository.workspace, &["config", "user.name"]),
        "Zeroshot"
    );
    assert_eq!(
        git_output(&fixture.repository.workspace, &["config", "user.email"]),
        "delivery@zeroshot.invalid"
    );
    assert_eq!(
        fs::read_to_string(fixture.repository.workspace.join("result.txt")).assert_value(),
        "feature work\n"
    );
    assert_transport_is_scoped(&fixture);
}

#[tokio::test]
async fn host_commit_identity_authors_target_integration_and_configures_repairs() {
    use crate::native_v2_candidate::test_support::{
        BOT_AUTHOR, CO_AUTHOR, bot_commit_identity, co_author_identity, commit_attribution,
        commit_authorship,
    };
    let mut fixture = TargetFixture::new();
    let transport = fixture
        .authority
        .git_transport(&fixture.repository.workspace, "acme/project")
        .with_commit_identity(Some(bot_commit_identity()))
        .with_commit_co_author(Some(co_author_identity()));
    fixture.authority = fixture.authority.clone().with_git_transport(transport);
    git(
        &fixture.repository.workspace,
        &["config", "user.name", "Worker"],
    );

    let outcome = fixture.reconcile().await;
    assert!(matches!(
        outcome.1,
        SourceReconciliationOutcome::NeedsWork(_)
    ));
    assert_eq!(
        git_output(&fixture.repository.workspace, &["rev-parse", "HEAD^2"]),
        fixture.target_revision
    );
    let bot = format!("{BOT_AUTHOR}|{BOT_AUTHOR}");
    for (revision, kind) in [("HEAD", "target merge"), ("HEAD^1", "prepared revision")] {
        assert_eq!(
            commit_authorship(&fixture.repository.workspace, revision),
            bot,
            "{kind}"
        );
    }
    assert_eq!(
        commit_attribution(&fixture.repository.workspace, "HEAD^1"),
        format!("{bot}\nfeat: preserve candidate\n\nCo-authored-by: {CO_AUTHOR}")
    );
    for (arguments, kind) in [
        (
            &["show", "-s", "--format=%B", "HEAD"][..],
            "target merge message",
        ),
        (
            &["config", "--local", "--list"][..],
            "repository configuration left for repairs",
        ),
    ] {
        let text = git_output(&fixture.repository.workspace, arguments);
        assert!(
            !text.contains("Ada Lovelace") && !text.contains("trailer."),
            "{kind}"
        );
    }
    assert_eq!(
        git_output(&fixture.repository.workspace, &["config", "user.name"]),
        "zeroshot-app[bot]"
    );
    assert_eq!(
        git_output(&fixture.repository.workspace, &["config", "user.email"]),
        "326899010+zeroshot-app[bot]@users.noreply.github.com"
    );
}

#[tokio::test]
async fn explicit_source_pins_can_be_ahead_of_or_diverge_from_the_target() {
    for target_is_ancestor in [true, false] {
        let mut fixture = TargetFixture::new();
        if target_is_ancestor {
            git(
                &fixture.external,
                &["reset", "--hard", &fixture.repository.base],
            );
            git(&fixture.external, &["push", "--force", "origin", "main"]);
            fixture.capture_target();
        }
        commit_all(
            &fixture.repository.workspace,
            "pinned source outside target",
        );
        fixture.repository.base = git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]);
        git(
            &fixture.repository.workspace,
            &["push", "origin", "HEAD:pinned-source"],
        );
        fs::write(
            fixture.repository.workspace.join("new-work.txt"),
            "new candidate work\n",
        )
        .assert_value();
        let outcome = fixture.reconcile().await;
        if target_is_ancestor {
            assert_eq!(outcome.1, SourceReconciliationOutcome::Unchanged);
        } else {
            assert!(matches!(
                outcome.1,
                SourceReconciliationOutcome::NeedsWork(_)
            ));
        }
        assert_eq!(
            fs::read_to_string(fixture.repository.workspace.join("new-work.txt")).assert_value(),
            "new candidate work\n"
        );
        git(
            &fixture.repository.workspace,
            &[
                "merge-base",
                "--is-ancestor",
                &fixture.repository.base,
                "HEAD",
            ],
        );
    }
}

#[tokio::test]
async fn unrelated_target_history_does_not_discard_candidate_work() {
    let mut fixture = TargetFixture::new();
    git(&fixture.external, &["checkout", "--orphan", "rewritten"]);
    git(&fixture.external, &["rm", "-rf", "."]);
    fs::write(
        fixture.external.join("replacement.txt"),
        "replacement history\n",
    )
    .assert_value();
    commit_all(&fixture.external, "replace target history");
    git(
        &fixture.external,
        &["push", "--force", "origin", "HEAD:main"],
    );
    fixture.capture_target();
    let result = fixture.try_reconcile().await;
    let error = result.expect_err("unrelated histories must not be merged");
    assert!(
        error
            .to_string()
            .contains("refusing to merge unrelated histories")
    );
    assert_eq!(
        fs::read_to_string(fixture.repository.workspace.join("result.txt")).assert_value(),
        "feature work\n"
    );
    git(
        &fixture.repository.workspace,
        &[
            "merge-base",
            "--is-ancestor",
            &fixture.repository.base,
            "HEAD",
        ],
    );
    assert!(
        !fixture
            .repository
            .workspace
            .join("replacement.txt")
            .exists()
    );
}

#[tokio::test]
async fn upstream_changes_alone_cannot_manufacture_a_deliverable_candidate() {
    let fixture = TargetFixture::new();
    fs::remove_file(fixture.repository.workspace.join("result.txt")).assert_value();
    let result = fixture.try_reconcile().await;
    let error = result.expect_err("target changes alone must not create candidate work");
    assert!(error.to_string().contains("no deliverable mutation"));
    assert_eq!(
        git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]),
        fixture.repository.base
    );
    assert_eq!(
        fs::read_to_string(
            fixture
                .repository
                .workspace
                .join(".github/workflows/ci.yml")
        )
        .assert_value(),
        "old workflow\n"
    );
}

#[tokio::test]
async fn rewritten_candidate_history_requests_repair_before_staging_new_work() {
    let fixture = TargetFixture::new();
    git(&fixture.repository.workspace, &["reset", "--hard", "HEAD^"]);
    assert_provenance_error_preserves_work(&fixture).await;
}

async fn assert_provenance_error_preserves_work(fixture: &TargetFixture) {
    let head = git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]);
    let status = git_output(&fixture.repository.workspace, &["status", "--porcelain=v1"]);
    let error = fixture
        .try_reconcile()
        .await
        .expect_err("lost local ancestry requires repair");
    assert!(error.to_string().contains("merge-base"));
    assert_eq!(
        git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]),
        head
    );
    assert_eq!(
        git_output(&fixture.repository.workspace, &["status", "--porcelain=v1"]),
        status
    );
    assert_eq!(
        fs::read_to_string(fixture.repository.workspace.join("result.txt")).assert_value(),
        "feature work\n"
    );
}

fn git_wrapper(repository: &TestGitRepository) -> PathBuf {
    let remote = path_text(&repository.remote);
    assert!(!remote.contains('\''));
    repository.root.write_executable(
        "git-target-wrapper",
        &format!(
            r#"#!/bin/bash
set -eu
/usr/bin/printf 'token=%s\n' "${{GH_TOKEN-unset}}" >> "${{0}}.capture"
arguments=()
for argument in "$@"; do
  /usr/bin/printf 'arg=%s\n' "$argument" >> "${{0}}.capture"
  if [[ "$argument" == 'https://github.com/acme/project.git' ]]; then
    arguments+=('{remote}')
  else
    arguments+=("$argument")
  fi
done
exec /usr/bin/git "${{arguments[@]}}"
"#
        ),
    )
}

fn assert_transport_is_scoped(fixture: &TargetFixture) {
    let capture =
        fs::read_to_string(format!("{}.capture", fixture.git_program.display())).assert_value();
    for invocation in capture.split("token=").filter(|part| !part.is_empty()) {
        let is_fetch = invocation.contains("arg=fetch\n");
        assert!(invocation.starts_with(if is_fetch { "test-token\n" } else { "unset\n" }));
        if is_fetch {
            assert!(invocation.contains("arg=--no-write-fetch-head\n"));
            assert!(invocation.contains(&format!("arg={}\n", fixture.target_revision)));
        }
        if invocation.contains("arg=merge\n") {
            assert!(invocation.contains("arg=rerere.enabled=false\n"));
            assert!(invocation.contains("arg=core.hooksPath=/dev/null\n"));
        }
    }
    let config =
        fs::read_to_string(fixture.repository.workspace.join(".git/config")).assert_value();
    for secret in ["test-token", "extraheader"] {
        assert!(
            !config.contains(secret),
            "delivery credential persisted: {secret}"
        );
    }
}

#[tokio::test]
async fn queue_confirmation_preserves_candidate_despite_repeated_target_movement() {
    let mut fixture = TargetFixture::new();
    fs::write(fixture.repository.root.child("queue-policy.json"),
        r#"{"data":{"repository":{"nameWithOwner":"acme/project","mergeQueue":{"id":"MQ_exact"}}}}"#).assert_value();
    let before = git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]);
    let status = git_output(&fixture.repository.workspace, &["status", "--porcelain=v1"]);
    for index in 0..3 {
        fixture.advance_target("later.txt", &format!("advance {index}\n"));
        assert_eq!(
            fixture.try_reconcile().await.assert_value(),
            SourceTargetIntegration::Deferred
        );
        assert_eq!(
            git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]),
            before
        );
        assert_eq!(
            git_output(&fixture.repository.workspace, &["status", "--porcelain=v1"]),
            status
        );
        assert!(!fixture.repository.workspace.join("later.txt").exists());
    }
    // A new read must observe removal rather than reusing earlier positive evidence.
    fs::write(
        fixture.repository.root.child("queue-policy.json"),
        r#"{"data":{"repository":{"nameWithOwner":"acme/project","mergeQueue":null}}}"#,
    )
    .assert_value();
    assert!(matches!(
        fixture.try_reconcile().await.assert_value(),
        SourceTargetIntegration::Integrated {
            outcome: SourceReconciliationOutcome::NeedsWork(_),
            ..
        }
    ));
}

#[tokio::test]
async fn queue_lookup_binds_exact_nondefault_branch_and_repository() {
    let fixture = TargetFixture::new();
    fixture.repository.root.write_executable("gh-target-ref", r#"#!/bin/sh
for arg in "$@"; do printf '%s\n' "$arg"; done > "$HOME/target-queue-args"
printf '%s\n' '{"data":{"repository":{"nameWithOwner":"acme/project","mergeQueue":{"id":"MQ_release"}}}}'
"#);
    let target = DeliveryTarget::new(
        "acme/project",
        "release/exact-case",
        &fixture.repository.base,
    )
    .assert_value();
    assert_eq!(
        fixture
            .authority
            .reconcile_delivery_target(
                SourceTargetReconciliation {
                    workspace: &fixture.repository.workspace,
                    target: &target,
                    commit_message: "candidate",
                },
                SourceDeliveryCredential::token("test-token")
            )
            .await
            .assert_value(),
        SourceTargetIntegration::Deferred
    );
    let args =
        fs::read_to_string(fixture.repository.root.child("target-queue-args")).assert_value();
    for expected in [
        "owner=acme",
        "name=project",
        "branch=release/exact-case",
        "mergeQueue(branch: $branch)",
    ] {
        assert!(args.contains(expected), "{args}");
    }
    assert!(!args.contains("defaultBranch"));
}

#[tokio::test]
async fn incomplete_unknown_and_mismatched_queue_policy_never_skips_integration() {
    let fixture = TargetFixture::new();
    for response in [
        serde_json::json!({}),
        serde_json::json!({"data": {"repository": null}}),
        serde_json::json!({"data": {"repository": {"nameWithOwner":"acme/project"}}}),
        serde_json::json!({"data": {"repository": {"nameWithOwner":"other/project","mergeQueue":{"id":"MQ"}}}}),
        serde_json::json!({"data": {"repository": {"nameWithOwner":"acme/project","mergeQueue":true}}}),
        serde_json::json!({"data": {"repository": {"nameWithOwner":"acme/project","mergeQueue":{}}}}),
        serde_json::json!({"data": {"repository": {"nameWithOwner":"acme/project","mergeQueue":{"id":" "}}}}),
        serde_json::json!({"data": {"repository": {"nameWithOwner":"acme/project","mergeQueue":{"id":"MQ"}}}, "errors":[{"message":"unavailable"}]}),
    ] {
        fs::write(
            fixture.repository.root.child("queue-policy.json"),
            response.to_string(),
        )
        .assert_value();
        assert!(fixture.try_reconcile().await.is_err(), "{response}");
        assert_eq!(
            git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]),
            fixture.repository.base
        );
    }
}

#[tokio::test]
async fn queue_policy_permission_and_transient_errors_keep_existing_retry_classification() {
    let fixture = TargetFixture::new();
    for (status, retryable) in [(403, false), (503, true)] {
        fixture.repository.root.write_executable(
            "gh-target-ref",
            &format!("#!/bin/sh\nprintf 'gh: unavailable (HTTP {status})\\n' >&2\nexit 1\n"),
        );
        let error = fixture
            .try_reconcile()
            .await
            .expect_err("failed policy is not queue evidence");
        assert_eq!(error.api_status(), Some(status));
        assert_eq!(error.retryable_operation(), retryable);
        assert_eq!(
            git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]),
            fixture.repository.base
        );
    }
}
