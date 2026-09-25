use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum Script {
    NoCi,
    Feedback,
    FeedbackReadTransient,
    FeedbackTooLarge,
    PushRejected,
    PolicyForbidden,
    PolicySchemaInvalid,
    PreflightClosed,
    PreflightMerged,
    PreflightRefusedAfterTransientRead,
    InspectFailed,
    MergeFailed,
    CiFailed,
    ReconcileCompletesThenUnavailable,
    TargetIntegrationResponseLost,
    LaterTargetIntegrationFails,
    ConflictMaterializationFailsAfterMutation,
    LargeCiDiagnostic,
    Conflict,
    ConflictAtMerge,
    RegistrationRace,
    MultipleRegistrationWaves,
    DeferredMerge,
    StrictBehind,
    HeadAdoptionRace,
    HeadAdoptionAfterRepair,
    HeadAdoptionRejected,
    HeadAdoptionUnavailable,
    HeadRecoveryClosed,
    HeadRecoveryMerged,
    HeadRecoveryReviewMissing,
    HeadUpdateConflict,
    HeadUpdateIdentityMismatch,
    HeadUpdatePending,
    HeadUpdateResponseLost,
    HeadUpdateUnavailable,
    RepeatedBehind,
    ProtectedBranch,
    ReviewSyncRace,
    CiFailsThenMerges,
    ConflictThenMerges,
    StaleConflictThenMerges,
    NeverConfirmsMerge,
    InvalidReviewObservation,
    PendingReadiness,
    ReviewIdentityMismatch,
    TerminalClosed,
    CredentialExpires,
    ReviewSyncCredentialExpires,
}

pub(super) struct FakeGitHub {
    remote: PathBuf,
    script: Script,
    pub(super) pushed: AtomicBool,
    merge_requested: AtomicBool,
    pub(super) merge_requests: AtomicUsize,
    pub(super) head_updates: AtomicUsize,
    pub(super) head_sync_attempts: AtomicUsize,
    pub(super) inspections: AtomicUsize,
    pub(super) feedback_reads: AtomicUsize,
    pub(super) delivery_reads: AtomicUsize,
    pub(super) reviews: Mutex<Vec<GitHubReviewRequest>>,
    pub(super) review_sync_attempts: AtomicUsize,
    pub(super) conflict_materializations: AtomicUsize,
    pub(super) target_reconciliations: AtomicUsize,
}

impl FakeGitHub {
    pub(super) fn new(remote: PathBuf, script: Script) -> Self {
        Self {
            remote,
            script,
            pushed: AtomicBool::new(false),
            merge_requested: AtomicBool::new(false),
            merge_requests: AtomicUsize::new(0),
            head_updates: AtomicUsize::new(0),
            head_sync_attempts: AtomicUsize::new(0),
            inspections: AtomicUsize::new(0),
            feedback_reads: AtomicUsize::new(0),
            delivery_reads: AtomicUsize::new(0),
            reviews: Mutex::new(Vec::new()),
            review_sync_attempts: AtomicUsize::new(0),
            conflict_materializations: AtomicUsize::new(0),
            target_reconciliations: AtomicUsize::new(0),
        }
    }

    fn review_state(&self, inspection: usize) -> GitHubReviewState {
        match self.script {
            Script::NoCi
            | Script::Feedback
            | Script::FeedbackReadTransient
            | Script::FeedbackTooLarge
            | Script::TargetIntegrationResponseLost
            | Script::LaterTargetIntegrationFails
            | Script::CredentialExpires
            | Script::ReviewSyncRace
            | Script::PolicyForbidden
            | Script::PolicySchemaInvalid
            | Script::PreflightClosed
            | Script::PreflightMerged
            | Script::PreflightRefusedAfterTransientRead
            | Script::InspectFailed
            | Script::InvalidReviewObservation
            | Script::MergeFailed
            | Script::ReviewIdentityMismatch => self.no_ci_state(),
            Script::ReviewSyncCredentialExpires => self.no_ci_state(),
            Script::RegistrationRace => self.registration_race_state(inspection),
            Script::MultipleRegistrationWaves => self.multiple_registration_waves_state(inspection),
            Script::CiFailsThenMerges => self.ci_repair_state(inspection),
            Script::ConflictThenMerges | Script::StaleConflictThenMerges => {
                self.conflict_repair_state()
            }
            Script::PendingReadiness => open_review(GitHubChecks::Pending),
            Script::TerminalClosed => GitHubReviewState::Closed,
            _ => self.static_review_state(),
        }
    }

    fn static_review_state(&self) -> GitHubReviewState {
        if self.exercises_head_update() || matches!(self.script, Script::RepeatedBehind) {
            return self.no_ci_state();
        }
        match self.script {
            Script::CiFailed | Script::ReconcileCompletesThenUnavailable => {
                open_review(failed_checks())
            }
            Script::LargeCiDiagnostic => open_review(GitHubChecks::Failed {
                diagnostic: "failed check: build\n".to_owned() + &"λ🦀".repeat(20_000),
            }),
            Script::Conflict | Script::ConflictMaterializationFailsAfterMutation => {
                GitHubReviewState::Conflict
            }
            Script::ConflictAtMerge => open_review(GitHubChecks::NotRequired),
            Script::DeferredMerge => self.no_ci_state(),
            Script::ProtectedBranch | Script::NeverConfirmsMerge => {
                open_review(GitHubChecks::Passed)
            }
            _ => open_review(GitHubChecks::Pending),
        }
    }

    fn no_ci_state(&self) -> GitHubReviewState {
        if self.merge_requested.load(Ordering::SeqCst) {
            merged_review()
        } else {
            open_review(GitHubChecks::NotRequired)
        }
    }

    fn ci_repair_state(&self, inspection: usize) -> GitHubReviewState {
        if inspection == 1 {
            return open_review(failed_checks());
        }
        if self.merge_requested.load(Ordering::SeqCst) {
            merged_review()
        } else {
            open_review(GitHubChecks::Passed)
        }
    }

    fn conflict_repair_state(&self) -> GitHubReviewState {
        if self.conflict_materializations.load(Ordering::SeqCst) == 0 {
            GitHubReviewState::Conflict
        } else {
            self.no_ci_state()
        }
    }

    fn registration_race_state(&self, inspection: usize) -> GitHubReviewState {
        if self.merge_requested.load(Ordering::SeqCst) {
            merged_review()
        } else if inspection == 1 {
            open_review(GitHubChecks::NotRequired)
        } else {
            open_review(GitHubChecks::Passed)
        }
    }

    fn multiple_registration_waves_state(&self, inspection: usize) -> GitHubReviewState {
        if self.merge_requested.load(Ordering::SeqCst) {
            merged_review()
        } else if matches!(inspection, 2 | 4) {
            open_review(GitHubChecks::Pending)
        } else {
            open_review(GitHubChecks::Passed)
        }
    }

    fn merge_is_pending(&self) -> bool {
        let requests = self.merge_requests.load(Ordering::SeqCst);
        match self.script {
            Script::ProtectedBranch => true,
            Script::DeferredMerge => requests <= 4,
            Script::RegistrationRace => requests == 1,
            Script::MultipleRegistrationWaves => requests <= 2,
            _ => false,
        }
    }

    fn uses_refreshed_credential(&self) -> bool {
        matches!(
            self.script,
            Script::CredentialExpires | Script::ReviewSyncCredentialExpires
        )
    }

    fn exercises_head_update(&self) -> bool {
        matches!(
            self.script,
            Script::StrictBehind
                | Script::HeadAdoptionRace
                | Script::HeadAdoptionAfterRepair
                | Script::HeadAdoptionRejected
                | Script::HeadAdoptionUnavailable
                | Script::HeadRecoveryClosed
                | Script::HeadRecoveryMerged
                | Script::HeadRecoveryReviewMissing
                | Script::HeadUpdateConflict
                | Script::HeadUpdateIdentityMismatch
                | Script::HeadUpdatePending
                | Script::HeadUpdateResponseLost
                | Script::HeadUpdateUnavailable
        )
    }

    fn immediate_head_update_outcome(
        &self,
    ) -> Option<Result<GitHubHeadUpdateOutcome, GitHubAuthorityError>> {
        match self.script {
            Script::HeadUpdatePending => Some(Ok(GitHubHeadUpdateOutcome::Pending)),
            Script::HeadUpdateConflict => Some(Ok(GitHubHeadUpdateOutcome::Conflict)),
            Script::HeadUpdateUnavailable
            | Script::HeadRecoveryReviewMissing
            | Script::HeadRecoveryClosed
            | Script::HeadRecoveryMerged => Some(Err(GitHubAuthorityError::Unavailable)),
            _ => None,
        }
    }

    pub(super) fn review_requests(&self) -> MutexGuard<'_, Vec<GitHubReviewRequest>> {
        self.reviews.lock().assert_value_with("review request lock")
    }
}

pub(super) fn delivery_harness(script: Script) -> (TempRepo, Arc<FakeGitHub>) {
    let repo = TempRepo::delivery();
    let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), script));
    (repo, authority)
}

fn advance_conflicting_target(remote: &Path) {
    let root = remote.parent().assert_value();
    let target = root.join("conflicting-target");
    git(
        root,
        &[
            "clone",
            remote.to_str().assert_value(),
            target.to_str().assert_value(),
        ],
    );
    fs::write(target.join("result.txt"), "target\n").assert_value();
    git(&target, &["add", "result.txt"]);
    git(
        &target,
        &[
            "-c",
            "user.name=Target",
            "-c",
            "user.email=target@example.invalid",
            "commit",
            "--no-verify",
            "--message",
            "advance target",
        ],
    );
    git(&target, &["push", "origin", "main"]);
}

fn merged_review() -> GitHubReviewState {
    GitHubReviewState::Merged {
        merge_revision: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
    }
}

fn open_review(checks: GitHubChecks) -> GitHubReviewState {
    GitHubReviewState::Open { checks }
}

fn failed_checks() -> GitHubChecks {
    GitHubChecks::Failed {
        diagnostic: "Required CI checks failed:\n- hidden policy concluded failure".to_owned(),
    }
}

#[async_trait]
impl GitHubDeliveryAuthority for FakeGitHub {
    async fn observe_delivery(
        &self,
        request: GitHubDeliveryRead<'_>,
        _credential: GitHubCredential<'_>,
    ) -> Result<GitHubDeliverySnapshot, GitHubAuthorityError> {
        let read = self.delivery_reads.fetch_add(1, Ordering::SeqCst);
        if matches!(self.script, Script::PreflightRefusedAfterTransientRead) && read == 0 {
            return Err(GitHubAuthorityError::Unavailable);
        }
        let output = tokio::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&self.remote)
            .args([
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", request.head_branch),
            ])
            .output()
            .await
            .assert_value();
        let head_revision = output.status.success().then(|| {
            String::from_utf8(output.stdout)
                .assert_value()
                .trim()
                .to_owned()
        });
        let review = head_revision.as_ref().and_then(|head| {
            if matches!(self.script, Script::HeadRecoveryReviewMissing) {
                return None;
            }
            let reviews = self.reviews.lock().assert_value();
            let request = reviews.last()?;
            let state = match self.script {
                Script::HeadRecoveryClosed => GitHubReviewState::Closed,
                Script::HeadRecoveryMerged => merged_review(),
                Script::PreflightClosed => GitHubReviewState::Closed,
                Script::PreflightMerged => merged_review(),
                _ => self.no_ci_state(),
            };
            Some(
                GitHubReviewReceipt {
                    review_id: "17".to_owned(),
                    repository: request.target.repository.clone(),
                    target_branch: request.target.target_branch.clone(),
                    head_branch: request.head_branch.clone(),
                    head_revision: head.clone(),
                }
                .observation(state),
            )
        });
        Ok(GitHubDeliverySnapshot {
            review,
            head_revision,
        })
    }

    async fn reconcile_delivery_target(
        &self,
        request: GitHubTargetReconciliation<'_>,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubTargetIntegration, GitHubAuthorityError> {
        let attempt = self.target_reconciliations.fetch_add(1, Ordering::SeqCst);
        let authority = crate::native_v2_candidate::test_support::local_delivery_authority(
            request.workspace,
            &self.remote,
        );
        let result = authority
            .reconcile_delivery_target(request, credential)
            .await?;
        if matches!(self.script, Script::TargetIntegrationResponseLost) && attempt == 0 {
            assert!(matches!(
                result.outcome,
                GitHubReconciliationOutcome::NeedsWork(_)
            ));
            return Err(GitHubAuthorityError::api(
                Some(503),
                "target integration completed but confirmation was unavailable",
            ));
        }
        if matches!(self.script, Script::LaterTargetIntegrationFails) && attempt == 1 {
            assert!(matches!(
                result.outcome,
                GitHubReconciliationOutcome::NeedsWork(_)
            ));
            return Err(GitHubAuthorityError::repairable(
                "target integration completed but its response was malformed",
            ));
        }
        Ok(result)
    }

    async fn reconcile_delivery_head(
        &self,
        request: GitHubHeadReconciliation<'_>,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubReconciliationOutcome, GitHubAuthorityError> {
        if matches!(self.script, Script::PreflightRefusedAfterTransientRead)
            && request.published.head_revision != request.observed.head_revision
        {
            return Ok(GitHubReconciliationOutcome::Refused(
                "the remote branch no longer has lineage-owned ancestry".to_owned(),
            ));
        }
        if matches!(self.script, Script::ReconcileCompletesThenUnavailable)
            && request.published.head_revision != request.observed.head_revision
        {
            let attempt = self.head_sync_attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                git(
                    request.workspace,
                    &[
                        "fetch",
                        self.remote.to_str().assert_value(),
                        &request.observed.head_revision,
                    ],
                );
                git(
                    request.workspace,
                    &["merge", "--ff-only", &request.observed.head_revision],
                );
                return Err(GitHubAuthorityError::api(
                    Some(503),
                    "operation completed but confirmation was unavailable",
                ));
            }
            return Ok(GitHubReconciliationOutcome::Unchanged);
        }
        if request.published.head_revision == request.observed.head_revision {
            return Ok(GitHubReconciliationOutcome::Unchanged);
        }
        self.synchronize_review_head(
            GitHubHeadSynchronization {
                workspace: request.workspace,
                previous: request.published,
                updated: request.observed,
            },
            credential,
        )
        .await?;
        Ok(if request.authorized_update {
            GitHubReconciliationOutcome::Adopted
        } else {
            GitHubReconciliationOutcome::NeedsWork(
                "reconciled an observed remote update".to_owned(),
            )
        })
    }

    async fn push_branch(
        &self,
        request: &GitHubPushRequest,
        credential: GitHubCredential<'_>,
    ) -> Result<(), GitHubAuthorityError> {
        assert_eq!(credential.expose(), "test-token");
        if matches!(self.script, Script::PushRejected) {
            let mut command = tokio::process::Command::new("/bin/sh");
            command.args([
                "-c",
                "printf '%s\\n' 'unfamiliar remote refusal: uploaded pack rejected' >&2; exit 1",
            ]);
            let failure = crate::native_v2_delivery::command::capture(
                &mut command,
                std::time::Duration::from_secs(1),
            )
            .await
            .and_then(GitCommandFailure::require_success)
            .expect_err("explicit Git command failure");
            return Err(failure.into());
        }
        if !push_succeeded(request, &self.remote).await {
            return Err(GitHubAuthorityError::Rejected);
        }
        self.pushed.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn open_or_update_review(
        &self,
        request: &GitHubReviewRequest,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubReviewReceipt, GitHubAuthorityError> {
        assert!(self.pushed.load(Ordering::SeqCst));
        let attempt = self.review_sync_attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if matches!(self.script, Script::ReviewSyncCredentialExpires)
            && credential.expose() == "test-token"
        {
            return Err(GitHubAuthorityError::api(
                Some(401),
                "HTTP 401: bad credentials",
            ));
        }
        let expected = if matches!(self.script, Script::ReviewSyncCredentialExpires) {
            "refreshed-token"
        } else {
            "test-token"
        };
        assert_eq!(credential.expose(), expected);
        if matches!(self.script, Script::ReviewSyncRace) && attempt == 1 {
            return Err(GitHubAuthorityError::api(
                Some(422),
                concat!(
                    "HTTP 422: validation failed; PullRequest head invalid ",
                    "pull request head revision is not visible"
                ),
            ));
        }
        self.reviews
            .lock()
            .assert_value_with("review request lock")
            .push(request.clone());
        let mut receipt = GitHubReviewReceipt {
            review_id: "17".to_owned(),
            repository: request.target.repository.clone(),
            target_branch: request.target.target_branch.clone(),
            head_branch: request.head_branch.clone(),
            head_revision: request.head_revision.clone(),
        };
        if matches!(self.script, Script::ReviewIdentityMismatch) {
            receipt.repository = "other/project".to_owned();
        }
        Ok(receipt)
    }

    async fn inspect_review(
        &self,
        review: &GitHubReviewReceipt,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubReviewObservation, GitHubAuthorityError> {
        if matches!(self.script, Script::PolicyForbidden) {
            return Err(GitHubAuthorityError::api(
                Some(403),
                "GraphQL: Resource not accessible by integration (HTTP 403)",
            ));
        }
        if matches!(self.script, Script::PolicySchemaInvalid) {
            return Err(GitHubAuthorityError::api(
                None,
                "GitHub returned an invalid policy response: missing field `repository`",
            ));
        }
        if matches!(self.script, Script::InspectFailed) {
            return Err(GitHubAuthorityError::api(
                Some(503),
                "remote inspection service returned an unfamiliar error",
            ));
        }
        let inspection = self.inspections.fetch_add(1, Ordering::SeqCst) + 1;
        if matches!(self.script, Script::CredentialExpires) && credential.expose() == "test-token" {
            return Err(GitHubAuthorityError::api(Some(401), "Bad credentials"));
        }
        let expected = if self.uses_refreshed_credential() {
            "refreshed-token"
        } else {
            "test-token"
        };
        assert_eq!(credential.expose(), expected);
        let state = self.review_state(inspection);
        let mut observation = review.observation(state);
        if matches!(self.script, Script::InvalidReviewObservation) {
            observation.head_branch = "zeroshot/unowned-head".to_owned();
        }
        Ok(observation)
    }

    async fn inspect_review_feedback(
        &self,
        _review: &GitHubReviewReceipt,
        _credential: GitHubCredential<'_>,
    ) -> Result<GitHubReviewFeedback, GitHubAuthorityError> {
        let read = self.feedback_reads.fetch_add(1, Ordering::SeqCst);
        if matches!(self.script, Script::FeedbackReadTransient) && read == 0 {
            return Err(GitHubAuthorityError::Unavailable);
        }
        let items = matches!(self.script, Script::Feedback | Script::FeedbackTooLarge)
            .then(|| GitHubReviewFeedbackItem {
                key: "review_comment:41".to_owned(),
                version: "v1".to_owned(),
                author: "review-bot".to_owned(),
                location: Some("path=src/lib.rs line=7".to_owned()),
                body: if matches!(self.script, Script::FeedbackTooLarge) {
                    "x".repeat(4 * 1024 * 1024 + 1)
                } else {
                    "Handle the empty input before returning.".to_owned()
                },
            })
            .into_iter()
            .collect();
        Ok(GitHubReviewFeedback { items })
    }

    async fn request_merge(
        &self,
        _review: &GitHubReviewReceipt,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubMergeRequestOutcome, GitHubAuthorityError> {
        if matches!(self.script, Script::MergeFailed) {
            return Err(GitHubAuthorityError::api(
                None,
                "remote merge service connection reset",
            ));
        }
        let expected = if self.uses_refreshed_credential() {
            "refreshed-token"
        } else {
            "test-token"
        };
        assert_eq!(credential.expose(), expected);
        self.merge_requests.fetch_add(1, Ordering::SeqCst);
        if matches!(self.script, Script::ConflictAtMerge) {
            return Ok(GitHubMergeRequestOutcome::Conflict);
        }
        let updates = self.head_updates.load(Ordering::SeqCst);
        if (self.exercises_head_update() && updates == 0)
            || (matches!(self.script, Script::RepeatedBehind) && updates < 2)
        {
            return Ok(GitHubMergeRequestOutcome::HeadUpdateRequired);
        }
        if self.merge_is_pending() {
            return Ok(GitHubMergeRequestOutcome::Pending);
        }
        self.merge_requested.store(true, Ordering::SeqCst);
        Ok(GitHubMergeRequestOutcome::Accepted)
    }

    async fn update_review_head(
        &self,
        workspace: &Path,
        review: &GitHubReviewReceipt,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubHeadUpdateOutcome, GitHubAuthorityError> {
        assert_eq!(credential.expose(), "test-token");
        if let Some(outcome) = self.immediate_head_update_outcome() {
            return outcome;
        }
        if !self.exercises_head_update() && !matches!(self.script, Script::RepeatedBehind) {
            return Ok(GitHubHeadUpdateOutcome::Pending);
        }
        let status = tokio::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(workspace)
            .args([
                "-c",
                "user.name=GitHub",
                "-c",
                "user.email=noreply@github.com",
                "commit",
                "--allow-empty",
                "--no-verify",
                "--message",
                "Merge branch 'main' into delivery branch",
            ])
            .status()
            .await
            .assert_value();
        if !status.success() {
            return Err(GitHubAuthorityError::Rejected);
        }
        let output = tokio::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(workspace)
            .args(["rev-parse", "HEAD"])
            .output()
            .await
            .assert_value();
        let head_revision = String::from_utf8(output.stdout)
            .assert_value()
            .trim()
            .to_owned();
        let status = tokio::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(workspace)
            .arg("push")
            .arg(&self.remote)
            .arg(format!("HEAD:refs/heads/{}", review.head_branch))
            .status()
            .await
            .assert_value();
        if !status.success() {
            return Err(GitHubAuthorityError::Rejected);
        }
        self.head_updates.fetch_add(1, Ordering::SeqCst);
        let mut updated = review.clone();
        updated.head_revision = head_revision;
        if matches!(self.script, Script::HeadUpdateIdentityMismatch) {
            updated.head_branch = "zeroshot/unowned-head".to_owned();
        }
        if matches!(self.script, Script::HeadUpdateResponseLost) {
            return Err(GitHubAuthorityError::api(
                Some(503),
                "head update completed but its response was lost",
            ));
        }
        if matches!(self.script, Script::HeadAdoptionAfterRepair) {
            git(workspace, &["reset", "--hard", &review.head_revision]);
        }
        Ok(GitHubHeadUpdateOutcome::Updated(updated))
    }

    async fn synchronize_review_head(
        &self,
        request: GitHubHeadSynchronization<'_>,
        credential: GitHubCredential<'_>,
    ) -> Result<(), GitHubAuthorityError> {
        assert_eq!(credential.expose(), "test-token");
        let attempt = self.head_sync_attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if matches!(self.script, Script::HeadAdoptionAfterRepair) {
            if attempt == 1 {
                return Err(GitHubAuthorityError::api(
                    None,
                    "local fetch failed before adoption",
                ));
            }
            git(
                request.workspace,
                &["merge", "--ff-only", &request.updated.head_revision],
            );
        }
        if matches!(self.script, Script::HeadAdoptionRace) && attempt == 1 {
            return Err(GitHubAuthorityError::Unavailable);
        }
        if matches!(self.script, Script::HeadAdoptionRejected) {
            return Err(GitHubAuthorityError::Rejected);
        }
        if matches!(self.script, Script::HeadAdoptionUnavailable) {
            return Err(GitHubAuthorityError::Unavailable);
        }
        Ok(())
    }

    async fn materialize_merge_conflict(
        &self,
        request: &GitHubConflictRequest,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubConflictOutcome, GitHubAuthorityError> {
        assert_eq!(credential.expose(), "test-token");
        self.conflict_materializations
            .fetch_add(1, Ordering::SeqCst);
        if matches!(self.script, Script::StaleConflictThenMerges) {
            return Ok(GitHubConflictOutcome::ObservationChanged);
        }
        advance_conflicting_target(&self.remote);
        let target_revision = git_output(&self.remote, &["rev-parse", "refs/heads/main"]);
        let fetch = std::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&request.workspace)
            .args(["fetch", "--no-tags", "--quiet"])
            .arg(&self.remote)
            .arg(&target_revision)
            .status()
            .assert_value();
        if !fetch.success() {
            return Err(GitHubAuthorityError::Rejected);
        }
        let merge = std::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&request.workspace)
            .args([
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
                &target_revision,
            ])
            .status()
            .assert_value();
        if merge.code() != Some(1) {
            return Err(GitHubAuthorityError::Rejected);
        }
        if matches!(
            self.script,
            Script::ConflictMaterializationFailsAfterMutation
        ) {
            return Err(GitHubAuthorityError::repairable(
                "conflict inspection failed after integrating a newer target",
            ));
        }
        let paths = std::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&request.workspace)
            .args(["diff", "--name-only", "--diff-filter=U", "-z"])
            .output()
            .assert_value();
        if !paths.status.success() {
            return Err(GitHubAuthorityError::Rejected);
        }
        let conflicted_paths = String::from_utf8(paths.stdout)
            .assert_value()
            .split_terminator('\0')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if conflicted_paths.is_empty() {
            return Err(GitHubAuthorityError::Rejected);
        }
        Ok(GitHubConflictOutcome::Materialized(
            GitHubConflictMaterialization {
                target_revision,
                conflicted_paths,
            },
        ))
    }
}

async fn push_succeeded(request: &GitHubPushRequest, remote: &Path) -> bool {
    let mut command = tokio::process::Command::new("/usr/bin/git");
    command
        .arg("-C")
        .arg(&request.workspace)
        .arg("push")
        .arg(remote)
        .arg(format!("HEAD:refs/heads/{}", request.head_branch));
    command.status().await.assert_value().success()
}

pub(super) fn write_executable(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    openengine_cluster_testkit::fixture::write_executable(&path, contents, 0o700)
        .assert_value_with("write test executable");
    path
}

pub(super) fn argument_lines(capture: &str) -> String {
    capture
        .lines()
        .filter(|line| line.starts_with("arg="))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) const GH_SCRIPT: &str = r#"#!/bin/sh
set -eu
capture="${0}.capture"
{
  /usr/bin/printf '%s\n' '---'
  /usr/bin/printf 'token=%s\n' "${GH_TOKEN-unset}"
  /usr/bin/printf 'host=%s\n' "${GH_HOST-unset}"
  /usr/bin/printf 'home=%s\n' "${HOME-unset}"
  /usr/bin/printf 'path=%s\n' "${PATH-unset}"
  for argument in "$@"; do /usr/bin/printf 'arg=%s\n' "$argument"; done
} >> "$capture"
endpoint=$2
method=GET
previous=
for argument in "$@"; do
  if [ "$previous" = "--method" ]; then method=$argument; fi
  previous=$argument
done
print_review() {
  /usr/bin/printf '%s%s%s%s%s%s%s%s\n' \
    '{"number":17,"title":"fix: repair checkout",' \
    '"body":"<!-- zeroshot-delivery:generated:v1:start -->\n' \
    'Repair the checkout flow.\n\nCloses #208\n' \
    '<!-- zeroshot-delivery:generated:v1:end -->",' \
    '"state":"open","merged":false,"merge_commit_sha":null,"base":' \
    '{"ref":"main","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","repo":{"full_name":"acme/project"}},' \
    '"head":{"ref":"zeroshot/v2-test","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",' \
    '"repo":{"full_name":"acme/project"}}}'
}
case "$endpoint:$method" in
  repos/acme/project/git/ref/heads/zeroshot/v2-test:GET)
    /usr/bin/printf '%s%s\n' \
      '{"ref":"refs/heads/zeroshot/v2-test","object":{"sha":' \
      '"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","type":"commit"}}'
    ;;
  repos/acme/project/pulls:GET)
    /usr/bin/printf '%s\n' '[]'
    ;;
  repos/acme/project/pulls:POST)
    print_review
    ;;
  repos/acme/project/pulls/17:GET)
    print_review
    ;;
  repos/acme/project/pulls/17:PATCH)
    print_review
    ;;
  repos/acme/project/issues/208:GET)
    /usr/bin/printf '%s\n' '{"comments":0}'
    ;;
  repos/acme/project/issues/208/comments:GET)
    /usr/bin/printf '%s\n' '[]'
    ;;
  repos/acme/project/issues/208/comments:POST)
    /usr/bin/printf '%s\n' '{"id":71}'
    ;;
  graphql:GET)
    /usr/bin/printf '%s%s%s%s%s%s\n' \
      '[{"data":{"repository":{"nameWithOwner":"acme/project","mergeCommitAllowed":true,' \
      '"squashMergeAllowed":true,"rebaseMergeAllowed":true,"pullRequest":{' \
      '"id":"PR_node_17","number":17,"state":"OPEN","merged":false,"mergeCommit":null,' \
      '"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","isDraft":false,' \
      '"isInMergeQueue":false,"isMergeQueueEnabled":false,"baseRefName":"main",' \
      '"baseRef":{"name":"main","refUpdateRule":{"requiredApprovingReviewCount":0,' \
      '"requiredStatusCheckContexts":[],' \
      '"requiresCodeOwnerReviews":false,"requiresConversationResolution":false,' \
      '"requiresLinearHistory":false,"requiresSignatures":false}},' \
      '"headRefName":"zeroshot/v2-test",' \
      '"headRefOid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","commits":{"nodes":[{' \
      '"commit":{"statusCheckRollup":null}}]}}}}}]'
    ;;
  repos/acme/project/commits/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/check-runs:GET)
    /usr/bin/printf '%s\n' '{"total_count":0,"check_runs":[]}'
    ;;
  repos/acme/project/commits/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/status:GET)
    /usr/bin/printf '%s\n' '{"state":"pending","statuses":[]}'
    ;;
  repos/acme/project/pulls/17/merge:PUT)
    /usr/bin/printf '%s\n' '{"merged":true,"sha":"cccccccccccccccccccccccccccccccccccccccc"}'
    ;;
  merge:GET)
    ;;
  *) exit 19 ;;
esac
"#;

pub(super) const GIT_SCRIPT: &str = r#"#!/bin/sh
set -eu
capture="${0}.capture"
{
  /usr/bin/printf '%s\n' '---'
  /usr/bin/printf 'token=%s\n' "${GH_TOKEN-unset}"
  /usr/bin/printf 'home=%s\n' "${HOME-unset}"
  /usr/bin/printf 'path=%s\n' "${PATH-unset}"
  /usr/bin/printf 'config_count=%s\n' "${GIT_CONFIG_COUNT-unset}"
  /usr/bin/printf 'config_key_1=%s\n' "${GIT_CONFIG_KEY_1-unset}"
  /usr/bin/printf 'config_value_1=%s\n' "${GIT_CONFIG_VALUE_1-unset}"
  for argument in "$@"; do /usr/bin/printf 'arg=%s\n' "$argument"; done
} >> "$capture"
"#;

pub(super) const GH_MISMATCH_SCRIPT: &str = r#"#!/bin/sh
/usr/bin/printf '%s%s%s%s\n' \
  '[{"number":17,"state":"open","merged":false,"merge_commit_sha":null,"base":' \
  '{"ref":"other","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","repo":{"full_name":"acme/project"}},' \
  '"head":{"ref":"zeroshot/v2-test","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",' \
  '"repo":{"full_name":"acme/project"}}}]'
"#;
