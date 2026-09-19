use super::*;
use crate::native_v2_candidate::test_support::{commit_all, path_text};

const WORKFLOW: &str = ".github/workflows/ci.yml";
const UPDATED_WORKFLOW: &str = "name: updated upstream CI\n";

struct UpdatedTarget {
    repo: TempRepo,
    upstream: PathBuf,
    revision: String,
}

impl UpdatedTarget {
    fn new() -> Self {
        let mut repo = TempRepo::candidate();
        fs::create_dir_all(repo.workspace.join(".github/workflows")).assert_value();
        fs::write(repo.workspace.join(WORKFLOW), "name: original CI\n").assert_value();
        commit_all(&repo.workspace, "original workflow");
        git(&repo.workspace, &["push", "origin", "main"]);
        repo.base = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        fs::write(repo.workspace.join("result.txt"), "worker feature\n").assert_value();
        let upstream = repo.root.child("upstream");
        git(
            repo.root.path(),
            &["clone", path_text(&repo.remote), path_text(&upstream)],
        );
        let mut fixture = Self {
            repo,
            upstream,
            revision: String::new(),
        };
        fixture.advance(UPDATED_WORKFLOW);
        fixture
    }

    fn advance(&mut self, workflow: &str) {
        fs::write(self.upstream.join(WORKFLOW), workflow).assert_value();
        commit_all(&self.upstream, "update upstream CI");
        git(&self.upstream, &["push", "origin", "main"]);
        self.revision = git_output(&self.upstream, &["rev-parse", "HEAD"]);
    }

    fn harness(script: Script) -> (Self, Arc<FakeGitHub>, Arc<NativeV2DeliveryAdapter>) {
        let fixture = Self::new();
        let authority = Arc::new(FakeGitHub::new(fixture.repo.remote.clone(), script));
        let adapter = retained_adapter(
            &fixture.repo,
            authority.clone(),
            DeliveryPollPolicy::new(3, Duration::ZERO).assert_value(),
            DeliveryLineage::original("updated-target-before-push"),
        );
        (fixture, authority, adapter)
    }

    fn request(&self, mode: DeliveryMode) -> DeliveryRunRequest<'_> {
        DeliveryRunRequest {
            repo: &self.repo,
            attempts: 3,
            mode,
            run_id: "updated-target-before-push",
            refresh: None,
        }
    }

    fn head(&self) -> String {
        git_output(&self.repo.workspace, &["rev-parse", "HEAD"])
    }

    fn assert_review_baseline(&self, outcome: &WorkerOutcome) {
        let diagnostic = outcome_diagnostic(outcome);
        assert!(
            diagnostic.contains(&format!("sourceRevision: {}", self.repo.base)),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains(&format!("reviewBaseRevision: {}", self.revision)),
            "{diagnostic}"
        );
    }

    fn assert_integrated(&self, authority: &FakeGitHub, outcome: &WorkerOutcome) {
        assert_delivery_signal(outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
        assert!(!authority.pushed.load(Ordering::SeqCst));
        assert!(authority.review_requests().is_empty());
        self.assert_review_baseline(outcome);
        for ancestor in [&self.repo.base, &self.revision] {
            git(
                &self.repo.workspace,
                &["merge-base", "--is-ancestor", ancestor, "HEAD"],
            );
        }
        assert_eq!(
            git_output(
                &self.repo.workspace,
                &["diff", &self.revision, "HEAD", "--", WORKFLOW]
            ),
            ""
        );
        assert_eq!(
            fs::read_to_string(self.repo.workspace.join("result.txt")).assert_value(),
            "worker feature\n"
        );
        assert_eq!(
            git_output(&self.repo.workspace, &["status", "--porcelain=v1"]),
            ""
        );
        let local = self.head();
        assert_ne!(local, self.repo.base);
        assert_ne!(local, self.revision);
        assert!(
            !git_output(
                &self.repo.workspace,
                &["diff", &self.repo.base, "HEAD", "--", WORKFLOW]
            )
            .is_empty(),
            "the original source must remain distinct from the updated review base"
        );
    }
}

#[tokio::test]
async fn updated_target_workflow_is_integrated_and_reviewed_before_first_push() {
    for mode in [DeliveryMode::PullRequest, DeliveryMode::Merge] {
        let (fixture, authority, adapter) = UpdatedTarget::harness(Script::NoCi);
        let admitted_source = admitted(&fixture.repo, mode).await.source;

        let integrated = run_with_adapter(fixture.request(mode), adapter.clone()).await;

        fixture.assert_integrated(&authority, &integrated.outcome);
        assert_eq!(admitted_source.revision.as_str(), fixture.repo.base);
        let reviewed_head = fixture.head();

        // Returning through the authored repair/review loop authorizes the integrated candidate.
        let published = run_with_adapter(fixture.request(mode), adapter).await;
        let receipt = assert_delivery_signal(&published.outcome, mode.success_outcome());
        assert_eq!(receipt["headRevision"], reviewed_head);
        assert_eq!(fixture.head(), reviewed_head);
        assert_eq!(
            git_output(
                &fixture.repo.remote,
                &[
                    "rev-parse",
                    &format!(
                        "refs/heads/{}",
                        delivery_branch("updated-target-before-push")
                    )
                ]
            ),
            reviewed_head
        );
        assert_eq!(authority.review_requests().len(), 1);
    }
}

#[tokio::test]
async fn target_advancing_during_review_requires_another_review_before_push() {
    let (mut fixture, authority, adapter) = UpdatedTarget::harness(Script::NoCi);
    let mode = DeliveryMode::PullRequest;
    let integrated = run_with_adapter(fixture.request(mode), adapter.clone()).await;
    fixture.assert_integrated(&authority, &integrated.outcome);
    fixture.advance("name: newer upstream CI\n");

    let reintegrated = run_with_adapter(fixture.request(mode), adapter.clone()).await;

    fixture.assert_integrated(&authority, &reintegrated.outcome);
    assert_eq!(
        fs::read_to_string(fixture.repo.workspace.join(WORKFLOW)).assert_value(),
        "name: newer upstream CI\n"
    );
    let published = run_with_adapter(fixture.request(mode), adapter).await;
    assert_delivery_signal(&published.outcome, DELIVERY_OPENED_LABEL);
}

#[tokio::test]
async fn target_integration_with_lost_confirmation_still_requires_review() {
    let (fixture, authority, adapter) =
        UpdatedTarget::harness(Script::TargetIntegrationResponseLost);

    let integrated = run_with_adapter(fixture.request(DeliveryMode::PullRequest), adapter).await;

    fixture.assert_integrated(&authority, &integrated.outcome);
    assert_eq!(authority.target_reconciliations.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn later_ci_feedback_retains_the_integrated_review_baseline() {
    let (fixture, authority, adapter) = UpdatedTarget::harness(Script::CiFailed);
    let integrated = run_with_adapter(fixture.request(DeliveryMode::Merge), adapter.clone()).await;
    fixture.assert_integrated(&authority, &integrated.outcome);

    let ci_failure = run_with_adapter(fixture.request(DeliveryMode::Merge), adapter).await;

    assert_delivery_signal(&ci_failure.outcome, DELIVERY_CI_FAILED_LABEL);
    fixture.assert_review_baseline(&ci_failure.outcome);
}

#[tokio::test]
async fn failed_later_target_integration_does_not_reuse_the_previous_review_baseline() {
    let (mut fixture, authority, adapter) =
        UpdatedTarget::harness(Script::LaterTargetIntegrationFails);
    let mode = DeliveryMode::PullRequest;
    let integrated = run_with_adapter(fixture.request(mode), adapter.clone()).await;
    fixture.assert_integrated(&authority, &integrated.outcome);
    let previous_target = fixture.revision.clone();
    fixture.advance("name: latest upstream CI\n");

    let failure = run_with_adapter(fixture.request(mode), adapter).await;

    assert_delivery_signal(&failure.outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
    let diagnostic = outcome_diagnostic(&failure.outcome);
    assert!(
        diagnostic.contains("reviewBaseRevision: unavailable"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains(&format!("reviewBaseRevision: {previous_target}")));
    assert!(!authority.pushed.load(Ordering::SeqCst));
    assert_eq!(
        fs::read_to_string(fixture.repo.workspace.join(WORKFLOW)).assert_value(),
        "name: latest upstream CI\n"
    );
}

#[tokio::test]
async fn failed_conflict_materialization_does_not_reuse_the_previous_review_baseline() {
    let (fixture, authority, adapter) =
        UpdatedTarget::harness(Script::ConflictMaterializationFailsAfterMutation);
    let mode = DeliveryMode::Merge;
    let integrated = run_with_adapter(fixture.request(mode), adapter.clone()).await;
    fixture.assert_integrated(&authority, &integrated.outcome);

    let failure = run_with_adapter(fixture.request(mode), adapter).await;

    assert_delivery_signal(&failure.outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
    let merge_head = git_output(&fixture.repo.workspace, &["rev-parse", "MERGE_HEAD"]);
    assert_ne!(merge_head, fixture.revision);
    assert!(!git_output(&fixture.repo.workspace, &["ls-files", "--unmerged"]).is_empty());
    let diagnostic = outcome_diagnostic(&failure.outcome);
    assert!(
        diagnostic.contains("reviewBaseRevision: unavailable"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains(&format!("reviewBaseRevision: {}", fixture.revision)));
}

#[tokio::test]
async fn changed_conflict_observation_retains_the_confirmed_review_baseline() {
    let (fixture, authority, adapter) = UpdatedTarget::harness(Script::StaleConflictThenMerges);
    let mode = DeliveryMode::Merge;
    let integrated = run_with_adapter(fixture.request(mode), adapter.clone()).await;
    fixture.assert_integrated(&authority, &integrated.outcome);

    let completed = run_with_adapter(fixture.request(mode), adapter).await;

    assert_delivery_signal(&completed.outcome, DELIVERY_MERGED_LABEL);
    fixture.assert_review_baseline(&completed.outcome);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        1
    );
}
