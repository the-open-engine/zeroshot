//! Independent remote and authenticated smart-HTTP Git; policy responses remain deterministic.
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use super::*;

#[path = "transport_http.rs"]
mod transport_http;
use transport_http::HttpGit;

#[path = "transport_matrix_repository.rs"]
mod matrix_repository;

const RUN: &str = "http-transport-matrix";

fn commit_file(directory: &Path, path: &str, text: &str) -> String {
    fs::write(directory.join(path), text).assert_value();
    git(directory, &["add", path]);
    git(
        directory,
        &[
            "-c",
            "user.name=Transport Test",
            "-c",
            "user.email=transport@example.invalid",
            "commit",
            "--no-verify",
            "-m",
            path,
        ],
    );
    git_output(directory, &["rev-parse", "HEAD"])
}

fn reference(remote: &Path, branch: &str) -> Option<String> {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(remote)
        .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
        .output()
        .assert_value();
    output.status.success().then(|| {
        String::from_utf8(output.stdout)
            .assert_value()
            .trim()
            .to_owned()
    })
}

fn contains_object(workspace: &Path, revision: &str) -> bool {
    Command::new("/usr/bin/git")
        .arg("-C")
        .arg(workspace)
        .args(["cat-file", "-e", &format!("{revision}^{{commit}}")])
        .output()
        .assert_value()
        .status
        .success()
}

fn authority(repo: &TempRepo, server: &HttpGit) -> GhCliDeliveryAuthority {
    let program = server.git_program(repo);
    GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        git_identity: None,
        git_program: program.clone(),
        gh_program: program,
        home_directory: repo.root.path().to_owned(),
        api_deadline: Duration::from_secs(10),
        push_deadline: Duration::from_secs(10),
    })
}

fn push_request(repo: &TempRepo, revision: String) -> GitHubPushRequest {
    GitHubPushRequest {
        workspace: repo.workspace.clone(),
        target: target(repo),
        head_branch: delivery_branch(RUN),
        head_revision: revision,
    }
}

async fn push_successfully(repo: &TempRepo, server: &HttpGit, request: &GitHubPushRequest) {
    authority(repo, server)
        .push_branch(request, GitHubCredential("test-token"))
        .await
        .assert_value();
    assert_eq!(
        reference(&repo.remote, &request.head_branch),
        Some(request.head_revision.clone())
    );
}

fn publish_reviewed_candidate(repo: &TempRepo) -> GitHubReviewReceipt {
    let candidate = commit_file(&repo.workspace, "result.txt", "reviewed\n");
    let branch = delivery_branch(RUN);
    git(
        &repo.workspace,
        &[
            "push",
            "origin",
            &format!("{candidate}:refs/heads/{branch}"),
        ],
    );
    GitHubReviewReceipt {
        review_id: "17".to_owned(),
        repository: "acme/project".to_owned(),
        target_branch: "main".to_owned(),
        head_branch: branch,
        head_revision: candidate,
    }
}

#[tokio::test]
async fn http_push_uses_reviewed_sha_even_when_workspace_head_advances() {
    let repo = TempRepo::delivery();
    let candidate = commit_file(&repo.workspace, "result.txt", "reviewed\n");
    let request = push_request(&repo, candidate.clone());
    let later = commit_file(&repo.workspace, "later.txt", "unreviewed\n");
    let server = HttpGit::start(&repo.remote);

    push_successfully(&repo, &server, &request).await;

    assert_ne!(
        reference(&repo.remote, &request.head_branch),
        Some(later.clone())
    );
    assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), later);
    assert!(server.faults.accepted.load(Ordering::SeqCst) >= 2);
    assert_eq!(server.faults.denied.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn http_accepted_push_with_lost_response_is_confirmed_from_remote_ref() {
    let repo = TempRepo::delivery();
    let candidate = commit_file(&repo.workspace, "result.txt", "delivered\n");
    let request = push_request(&repo, candidate.clone());
    let server = HttpGit::start(&repo.remote);
    server
        .faults
        .drop_push_response_once
        .store(true, Ordering::SeqCst);

    push_successfully(&repo, &server, &request).await;

    assert_eq!(server.faults.dropped_responses.load(Ordering::SeqCst), 1);
    assert_eq!(server.faults.receive_packs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn http_rejected_credential_is_an_authentication_error_and_cannot_create_a_ref() {
    let repo = TempRepo::delivery();
    let candidate = commit_file(&repo.workspace, "result.txt", "delivered\n");
    let request = push_request(&repo, candidate);
    let server = HttpGit::start(&repo.remote);
    server
        .faults
        .require_refreshed
        .store(true, Ordering::SeqCst);

    let failure = authority(&repo, &server)
        .push_branch(&request, GitHubCredential("test-token"))
        .await
        .expect_err("expired credential must fail");

    assert!(failure.authentication_failed(), "{failure}");
    assert!(server.faults.denied.load(Ordering::SeqCst) > 0);
    assert_eq!(server.faults.receive_packs.load(Ordering::SeqCst), 0);
    assert_eq!(reference(&repo.remote, &request.head_branch), None);
}

#[derive(Clone, Copy, PartialEq)]
enum Update {
    None,
    Known,
    LostResponse,
}

struct TransportAuthority {
    concrete: GhCliDeliveryAuthority,
    remote: PathBuf,
    faults: Arc<transport_http::TransportFaults>,
    review: Mutex<Option<GitHubReviewReceipt>>,
    update: Update,
    updated: AtomicBool,
    expire_during_update: AtomicBool,
    expire_before_push: AtomicBool,
    reject_expired_push: AtomicBool,
    merged: AtomicBool,
    updates: AtomicUsize,
    synchronizations: AtomicUsize,
    pushes: AtomicUsize,
}

impl TransportAuthority {
    fn new(repo: &TempRepo, server: &HttpGit, update: Update) -> Self {
        Self {
            concrete: authority(repo, server),
            remote: repo.remote.clone(),
            faults: server.faults.clone(),
            review: Mutex::new(None),
            update,
            updated: AtomicBool::new(false),
            expire_during_update: AtomicBool::new(false),
            expire_before_push: AtomicBool::new(false),
            reject_expired_push: AtomicBool::new(false),
            merged: AtomicBool::new(false),
            updates: AtomicUsize::new(0),
            synchronizations: AtomicUsize::new(0),
            pushes: AtomicUsize::new(0),
        }
    }

    fn advance_remote(
        &self,
        workspace: &Path,
        review: &GitHubReviewReceipt,
    ) -> GitHubReviewReceipt {
        let writer = self.remote.parent().assert_value().join("remote-writer");
        git(
            self.remote.parent().assert_value(),
            &[
                "clone",
                self.remote.to_str().assert_value(),
                writer.to_str().assert_value(),
            ],
        );
        git(&writer, &["checkout", &review.head_branch]);
        let revision = commit_file(&writer, "remote.txt", "server-side update\n");
        git(&writer, &["push", "origin", &review.head_branch]);
        assert!(
            !contains_object(workspace, &revision),
            "remote update must not create worker objects"
        );
        assert_eq!(
            git_output(workspace, &["rev-parse", "HEAD"]),
            review.head_revision
        );
        let mut updated = review.clone();
        updated.head_revision = revision;
        *self.review.lock().assert_value() = Some(updated.clone());
        updated
    }
}

#[async_trait]
impl GitHubDeliveryAuthority for TransportAuthority {
    async fn observe_delivery(
        &self,
        request: GitHubDeliveryRead<'_>,
        _credential: GitHubCredential<'_>,
    ) -> Result<GitHubDeliverySnapshot, GitHubAuthorityError> {
        let head_revision = reference(&self.remote, request.head_branch);
        let review = self.review.lock().assert_value().clone().map(|mut review| {
            if let Some(head) = &head_revision {
                review.head_revision.clone_from(head);
            }
            review.observation(GitHubReviewState::Open {
                checks: GitHubChecks::Passed,
            })
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
        self.concrete
            .reconcile_delivery_target(request, credential)
            .await
    }

    async fn reconcile_delivery_head(
        &self,
        request: GitHubHeadReconciliation<'_>,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubReconciliationOutcome, GitHubAuthorityError> {
        self.concrete
            .reconcile_delivery_head(request, credential)
            .await
    }

    async fn push_branch(
        &self,
        request: &GitHubPushRequest,
        credential: GitHubCredential<'_>,
    ) -> Result<(), GitHubAuthorityError> {
        self.pushes.fetch_add(1, Ordering::SeqCst);
        if self.expire_before_push.swap(false, Ordering::SeqCst) {
            self.faults.require_refreshed.store(true, Ordering::SeqCst);
            self.faults.reject_all_credentials.store(
                self.reject_expired_push.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
        }
        self.concrete.push_branch(request, credential).await
    }

    async fn open_or_update_review(
        &self,
        request: &GitHubReviewRequest,
        _credential: GitHubCredential<'_>,
    ) -> Result<GitHubReviewReceipt, GitHubAuthorityError> {
        assert_eq!(
            reference(&self.remote, &request.head_branch),
            Some(request.head_revision.clone())
        );
        let review = GitHubReviewReceipt {
            review_id: "17".to_owned(),
            repository: request.target.repository.clone(),
            target_branch: request.target.target_branch.clone(),
            head_branch: request.head_branch.clone(),
            head_revision: reference(&self.remote, &request.head_branch).assert_value(),
        };
        *self.review.lock().assert_value() = Some(review.clone());
        Ok(review)
    }

    async fn inspect_review(
        &self,
        review: &GitHubReviewReceipt,
        _credential: GitHubCredential<'_>,
    ) -> Result<GitHubReviewObservation, GitHubAuthorityError> {
        let state = if self.merged.load(Ordering::SeqCst) {
            GitHubReviewState::Merged {
                merge_revision: review.head_revision.clone(),
            }
        } else {
            GitHubReviewState::Open {
                checks: GitHubChecks::Passed,
            }
        };
        assert_eq!(
            reference(&self.remote, &review.head_branch),
            Some(review.head_revision.clone())
        );
        Ok(review.observation(state))
    }

    async fn request_merge(
        &self,
        _review: &GitHubReviewReceipt,
        _credential: GitHubCredential<'_>,
    ) -> Result<GitHubMergeRequestOutcome, GitHubAuthorityError> {
        if self.update != Update::None && !self.updated.load(Ordering::SeqCst) {
            return Ok(GitHubMergeRequestOutcome::HeadUpdateRequired);
        }
        self.merged.store(true, Ordering::SeqCst);
        Ok(GitHubMergeRequestOutcome::Accepted)
    }

    async fn update_review_head(
        &self,
        workspace: &Path,
        review: &GitHubReviewReceipt,
        _credential: GitHubCredential<'_>,
    ) -> Result<GitHubHeadUpdateOutcome, GitHubAuthorityError> {
        assert_eq!(
            self.updates.fetch_add(1, Ordering::SeqCst),
            0,
            "accepted update must not be repeated"
        );
        let updated = self.advance_remote(workspace, review);
        self.updated.store(true, Ordering::SeqCst);
        if self.expire_during_update.load(Ordering::SeqCst) {
            self.faults.require_refreshed.store(true, Ordering::SeqCst);
        }
        self.faults.fail_fetch_once.store(true, Ordering::SeqCst);
        if self.update == Update::LostResponse {
            return Err(GitHubAuthorityError::api(
                Some(502),
                "update response lost after server commit",
            ));
        }
        Ok(GitHubHeadUpdateOutcome::Updated(updated))
    }

    async fn synchronize_review_head(
        &self,
        request: GitHubHeadSynchronization<'_>,
        credential: GitHubCredential<'_>,
    ) -> Result<(), GitHubAuthorityError> {
        self.synchronizations.fetch_add(1, Ordering::SeqCst);
        self.concrete
            .synchronize_review_head(request, credential)
            .await
    }
}

fn adapter(repo: &TempRepo, authority: Arc<TransportAuthority>) -> Arc<NativeV2DeliveryAdapter> {
    retained_adapter(
        repo,
        authority,
        DeliveryPollPolicy::new(5, Duration::from_millis(1)).assert_value(),
        DeliveryLineage::original(RUN),
    )
}

fn adopting_adapter(
    repo: &TempRepo,
    authority: Arc<TransportAuthority>,
) -> Arc<NativeV2DeliveryAdapter> {
    retained_adapter(
        repo,
        authority,
        DeliveryPollPolicy::new(5, Duration::from_millis(1)).assert_value(),
        DeliveryLineage::resumed(RUN),
    )
}

async fn execute(
    repo: &TempRepo,
    adapter: Arc<NativeV2DeliveryAdapter>,
    refresh: bool,
) -> DeliveryExecution {
    let binding = delivery_runtime_binding(repo, DeliveryMode::Merge).await;
    let request = DeliveryRunRequest {
        repo,
        attempts: 5,
        mode: DeliveryMode::Merge,
        run_id: RUN,
        refresh: refresh.then(|| {
            Arc::new(RefreshedDeliveryEnvironment { binding })
                as Arc<dyn crate::native_v2_runner::RuntimeEnvironmentRefresh>
        }),
    };
    tokio::time::timeout(Duration::from_secs(20), run_with_adapter(request, adapter))
        .await
        .assert_value_with("transport delivery must make bounded progress")
}

#[tokio::test]
async fn http_expired_push_credential_refreshes_or_stops_without_graph_repair() {
    for (refresh, permanently_denied, trusted_static) in [
        (false, false, false),
        (false, false, true),
        (true, false, false),
        (true, true, false),
    ] {
        let repo = TempRepo::delivery();
        let server = HttpGit::start(&repo.remote);
        let authority = Arc::new(TransportAuthority::new(&repo, &server, Update::None));
        authority.expire_before_push.store(true, Ordering::SeqCst);
        authority
            .reject_expired_push
            .store(permanently_denied, Ordering::SeqCst);

        let delivery = adapter(&repo, authority.clone());
        let delivery = if trusted_static {
            Arc::new(
                Arc::unwrap_or_clone(delivery)
                    .with_trusted_github_token(Some(Arc::from("test-token"))),
            )
        } else {
            delivery
        };
        let execution = execute(&repo, delivery, refresh).await;

        assert!(server.faults.denied.load(Ordering::SeqCst) > 0);
        assert_eq!(
            authority.pushes.load(Ordering::SeqCst),
            1 + usize::from(refresh)
        );
        if refresh && !permanently_denied {
            assert_delivery_signal(&execution.outcome, DELIVERY_MERGED_LABEL);
            assert!(server.faults.accepted.load(Ordering::SeqCst) > 0);
            assert_eq!(server.faults.receive_packs.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(execution.outcome, WorkerOutcome::authentication_refusal());
            assert_eq!(reference(&repo.remote, &delivery_branch(RUN)), None);
        }
    }
}

async fn updated_delivery(
    update: Update,
    refresh: bool,
) -> (
    TempRepo,
    HttpGit,
    Arc<TransportAuthority>,
    DeliveryExecution,
) {
    let repo = TempRepo::delivery();
    let server = HttpGit::start(&repo.remote);
    let authority = Arc::new(TransportAuthority::new(&repo, &server, update));
    authority
        .expire_during_update
        .store(refresh, Ordering::SeqCst);
    let execution = execute(&repo, adapter(&repo, authority.clone()), refresh).await;
    assert_eq!(server.faults.fetch_failures.load(Ordering::SeqCst), 1);
    assert_eq!(authority.updates.load(Ordering::SeqCst), 1);
    assert_eq!(
        fs::read_to_string(repo.workspace.join("remote.txt")).assert_value(),
        "server-side update\n"
    );
    (repo, server, authority, execution)
}

#[tokio::test]
async fn http_known_remote_update_retries_a_real_fetch_503_without_graph_repair() {
    for refresh in [false, true] {
        let (repo, server, authority, execution) = updated_delivery(Update::Known, refresh).await;

        assert_delivery_signal(&execution.outcome, DELIVERY_MERGED_LABEL);
        assert_eq!(
            authority.synchronizations.load(Ordering::SeqCst),
            2 + usize::from(refresh)
        );
        assert_eq!(server.faults.denied.load(Ordering::SeqCst) > 0, refresh);
        assert_eq!(
            git_output(&repo.workspace, &["rev-parse", "HEAD"]),
            reference(&repo.remote, &delivery_branch(RUN)).assert_value()
        );
    }
}

#[tokio::test]
async fn http_lost_update_receipt_fetches_remote_head_and_requires_existing_work_loop() {
    let (_repo, _server, authority, execution) =
        updated_delivery(Update::LostResponse, false).await;

    assert_delivery_signal(&execution.outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
    assert_eq!(authority.pushes.load(Ordering::SeqCst), 1);
    assert!(!authority.merged.load(Ordering::SeqCst));
    assert!(
        outcome_diagnostic(&execution.outcome).contains("update response lost after server commit")
    );
}

#[tokio::test]
async fn fresh_resumed_adapter_reconciles_a_remote_descendant_before_delivery() {
    let repo = TempRepo::delivery();
    let published = publish_reviewed_candidate(&repo);
    let server = HttpGit::start(&repo.remote);
    let authority = Arc::new(TransportAuthority::new(&repo, &server, Update::None));
    let observed = authority.advance_remote(&repo.workspace, &published);

    let execution = execute(&repo, adopting_adapter(&repo, authority.clone()), false).await;

    assert_delivery_signal(&execution.outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
    assert_eq!(
        git_output(&repo.workspace, &["rev-parse", "HEAD"]),
        observed.head_revision
    );
    assert_eq!(authority.pushes.load(Ordering::SeqCst), 0);
    assert!(!authority.merged.load(Ordering::SeqCst));
}

#[tokio::test]
async fn http_remote_update_preserves_dirty_repairs_without_reusing_old_approval() {
    let repo = TempRepo::delivery();
    let published = publish_reviewed_candidate(&repo);
    let server = HttpGit::start(&repo.remote);
    let authority = TransportAuthority::new(&repo, &server, Update::Known);
    let observed = authority.advance_remote(&repo.workspace, &published);
    let repair = commit_file(&repo.workspace, "repair.txt", "committed repair\n");
    fs::write(repo.workspace.join("dirty.txt"), "uncommitted repair\n").assert_value();

    let result = authority
        .reconcile_delivery_head(
            GitHubHeadReconciliation {
                workspace: &repo.workspace,
                published: &published,
                observed: &observed,
                commit_message: "Preserve local repair",
                authorized_update: true,
                adopting_existing: false,
            },
            GitHubCredential("test-token"),
        )
        .await
        .assert_value();

    assert!(
        matches!(result, GitHubReconciliationOutcome::NeedsWork(_)),
        "{result:?}"
    );
    git(
        &repo.workspace,
        &["merge-base", "--is-ancestor", &repair, "HEAD"],
    );
    git(
        &repo.workspace,
        &[
            "merge-base",
            "--is-ancestor",
            &observed.head_revision,
            "HEAD",
        ],
    );
    assert_eq!(
        fs::read_to_string(repo.workspace.join("dirty.txt")).assert_value(),
        "uncommitted repair\n"
    );
    assert_eq!(
        reference(&repo.remote, &published.head_branch),
        Some(observed.head_revision)
    );
    assert_eq!(server.faults.receive_packs.load(Ordering::SeqCst), 0);
}
