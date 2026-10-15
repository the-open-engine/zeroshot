use super::*;
use crate::native_v2_delivery::validation::TrustedDeliveryValidator;
use std::path::Path;

struct ValidationHarness {
    repo: TempRepo,
    _private: tempfile::TempDir,
    executable: PathBuf,
    authority: Arc<FakeGitHub>,
    adapter: Arc<NativeV2DeliveryAdapter>,
}

impl ValidationHarness {
    fn new(script: Script, body: &str) -> Self {
        Self::with_poll(
            script,
            body,
            DeliveryPollPolicy::new(100, Duration::from_millis(2)).unwrap(),
        )
    }

    fn at_stage(stage: &str, response: &str) -> Self {
        Self::new(
            Script::NoCi,
            &format!(
                r#"case "$request" in
    *'"stage":"{stage}"'*) printf '%s' '{response}' ;;
    *) printf '%s' '{{"verdict":"accepted"}}' ;;
esac"#,
            ),
        )
    }

    fn with_poll(script: Script, body: &str, poll: DeliveryPollPolicy) -> Self {
        let repo = TempRepo::delivery();
        let private = tempfile::tempdir().unwrap();
        let executable = write_executable(
            private.path(),
            "validator.sh",
            &format!(
                "#!/bin/sh\nset -eu\nrequest=$(/bin/cat)\nprintf '%s\\n' \"$request\" >> \"$0.requests\"\n/usr/bin/env > \"$0.environment\"\n{body}\n"
            ),
        );
        let validator = TrustedDeliveryValidator::new(executable.clone(), &repo.workspace).unwrap();
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), script));
        let adapter = retained_adapter(
            &repo,
            authority.clone(),
            poll,
            DeliveryLineage::original("trusted-validation"),
        );
        let adapter = Arc::new(
            (*adapter)
                .clone()
                .with_trusted_delivery_validator(Some(validator)),
        );
        Self {
            repo,
            _private: private,
            executable,
            authority,
            adapter,
        }
    }

    async fn start(&self, mode: DeliveryMode) -> crate::native_v2_runner::NodeHandle {
        start_with_adapter_token(
            DeliveryRunRequest {
                repo: &self.repo,
                attempts: 100,
                mode,
                run_id: "trusted-validation",
                refresh: None,
            },
            self.adapter.clone(),
            PullRequestFeedback::Consider,
            Some("test-token"),
        )
        .await
    }

    async fn run(&self) -> Result<WorkerOutcome, NodeRunnerError> {
        let mut handle = self.start(DeliveryMode::MergeV3).await;
        handle
            .completion()
            .await
            .map(|completion| completion.outcome)
    }

    fn sidecar(&self, suffix: &str) -> PathBuf {
        PathBuf::from(format!("{}{suffix}", self.executable.to_str().unwrap()))
    }

    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(self.sidecar(".requests"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn assert_published_without_merge(&self) -> SourceReviewRequest {
        let mut published = self.authority.review_requests();
        assert_eq!(published.len(), 1);
        assert!(self.authority.pushed.load(Ordering::SeqCst));
        assert_eq!(self.authority.merge_requests.load(Ordering::SeqCst), 0);
        published.remove(0)
    }

    fn assert_no_publication(&self) {
        assert!(!self.authority.pushed.load(Ordering::SeqCst));
        assert!(self.authority.review_requests().is_empty());
        assert_eq!(self.authority.merge_requests.load(Ordering::SeqCst), 0);
    }

    fn assert_private_exchange(&self) {
        let requests = fs::read_to_string(self.sidecar(".requests")).unwrap();
        let environment = fs::read_to_string(self.sidecar(".environment")).unwrap();
        for secret in [
            "test-token",
            "GH_TOKEN",
            "ZEROSHOT_GITHUB_TOKEN_FILE",
            "ZEROSHOT_DELIVERY_CHECK_COMMAND",
        ] {
            assert!(
                !requests.contains(secret),
                "credential entered helper request"
            );
            assert!(
                !environment.contains(secret),
                "credential entered helper environment"
            );
        }
        assert!(!requests.contains(self.executable.to_str().unwrap()));
    }
}

async fn wait_for_marker(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("trusted helper did not reach the expected stage");
}

#[tokio::test]
async fn trusted_publication_refusal_prevents_push_and_review_creation() {
    let harness = ValidationHarness::new(
        Script::NoCi,
        "printf '{\"verdict\":\"refused\",\"reason\":\"Backlog evidence changed\"}'",
    );
    let outcome = harness.run().await;
    assert!(
        matches!(outcome, Err(NodeRunnerError::DriverDetail(ref reason))
        if reason.contains("Backlog evidence changed")),
        "{outcome:?}"
    );
    harness.assert_no_publication();
    assert_eq!(harness.requests().len(), 1);
    assert_eq!(harness.requests()[0]["stage"], "publish");
    harness.assert_private_exchange();
}

#[tokio::test]
async fn trusted_publication_repair_returns_the_authored_repair_without_remote_effects() {
    let harness = ValidationHarness::new(
        Script::NoCi,
        "printf '{\"verdict\":\"repair\",\"reason\":\"Reproduce the exact current-main failure\"}'",
    );
    let outcome = harness.run().await.unwrap();
    assert_delivery_signal(&outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
    assert!(outcome_diagnostic(&outcome).contains("Reproduce the exact current-main failure"));
    harness.assert_no_publication();
    let requests = harness.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0]["reviewId"].is_null());
    assert_eq!(requests[0]["repository"], "acme/project");
    assert_eq!(requests[0]["targetBranch"], "main");
    assert_eq!(
        requests[0]["tree"],
        git_output(&harness.repo.workspace, &["rev-parse", "HEAD^{tree}"])
    );
}

const PENDING_REVIEW: &str = r#"case "$request" in
    *'"stage":"review"'*) printf '{"verdict":"pending"}' ;;
    *) printf '{"verdict":"accepted"}' ;;
esac"#;

#[tokio::test]
async fn pending_review_validation_preserves_ci_failure_and_new_bot_feedback() {
    for (script, signal, message) in [
        (
            Script::CiFailed,
            DELIVERY_CI_FAILED_LABEL,
            "Required CI checks failed",
        ),
        (
            Script::Feedback,
            DELIVERY_REPAIR_REQUIRED_LABEL,
            "Handle the empty input",
        ),
    ] {
        let harness = ValidationHarness::new(script, PENDING_REVIEW);
        let outcome = harness.run().await.unwrap();
        assert_delivery_signal(&outcome, signal);
        assert!(outcome_diagnostic(&outcome).contains(message));
        assert_eq!(harness.authority.inspections.load(Ordering::SeqCst), 1);
        assert_eq!(harness.authority.feedback_reads.load(Ordering::SeqCst), 1);
        assert_eq!(harness.authority.merge_requests.load(Ordering::SeqCst), 0);
        assert!(
            harness
                .requests()
                .iter()
                .any(|request| request["stage"] == "review")
        );
    }
}

#[tokio::test]
async fn published_receipt_precedes_a_permanent_first_policy_read_failure() {
    let harness = ValidationHarness::new(
        Script::PolicyForbidden,
        r#"case "$request" in
    *'"stage":"published"'*) printf '%s' "$request" > "$0.published" ;;
esac
printf '{"verdict":"accepted"}'"#,
    );
    let outcome = harness.run().await.unwrap();
    assert_eq!(
        outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
    );
    harness.assert_published_without_merge();
    let receipt: Value = serde_json::from_slice(&fs::read(harness.sidecar(".published")).unwrap())
        .expect("published callback must run before the unavailable policy read");
    assert_eq!(receipt["reviewId"], "17");
    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["stage"], "publish");
    assert_eq!(requests[1]["stage"], "published");
    for key in ["headRevision", "tree", "headBranch", "repository"] {
        assert_eq!(receipt[key], requests[0][key]);
    }
}

#[tokio::test]
async fn pending_merge_validation_blocks_merge_until_the_trusted_check_accepts() {
    let harness = ValidationHarness::new(
        Script::NoCi,
        r#"case "$request" in
    *'"stage":"merge"'*)
        if [ ! -f "$0.approved" ]; then
            printf waiting > "$0.pending"
            printf '{"verdict":"pending"}'
            exit 0
        fi ;;
esac
printf '{"verdict":"accepted"}'"#,
    );
    let mut handle = harness.start(DeliveryMode::MergeV3).await;
    wait_for_marker(&harness.sidecar(".pending")).await;
    assert!(harness.authority.pushed.load(Ordering::SeqCst));
    assert!(harness.authority.inspections.load(Ordering::SeqCst) > 0);
    assert_eq!(harness.authority.merge_requests.load(Ordering::SeqCst), 0);
    fs::write(harness.sidecar(".approved"), "approved").unwrap();
    let completion = tokio::time::timeout(Duration::from_secs(10), handle.completion())
        .await
        .unwrap()
        .unwrap();
    assert_delivery_signal(&completion.outcome, DELIVERY_MERGED_LABEL);
    assert_eq!(harness.authority.merge_requests.load(Ordering::SeqCst), 1);
    assert!(
        harness
            .requests()
            .iter()
            .filter(|request| request["stage"] == "merge")
            .count()
            >= 2
    );
    harness.assert_private_exchange();
}

#[tokio::test]
async fn an_authorized_head_update_requires_validation_of_the_new_revision() {
    let harness = ValidationHarness::new(
        Script::StrictBehind,
        r#"head=$(printf '%s' "$request" | /usr/bin/sed -n 's/.*"headRevision":"\([^"]*\)".*/\1/p')
if [ ! -f "$0.original" ]; then printf '%s' "$head" > "$0.original"; fi
original=$(/bin/cat "$0.original")
if [ "$head" != "$original" ]; then
    printf '{"verdict":"repair","reason":"New head requires fresh regression proof"}'
else
    printf '{"verdict":"accepted"}'
fi"#,
    );
    let outcome = harness.run().await.unwrap();
    assert_delivery_signal(&outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
    assert!(outcome_diagnostic(&outcome).contains("New head requires fresh regression proof"));
    assert_eq!(harness.authority.head_updates.load(Ordering::SeqCst), 1);
    assert_eq!(harness.authority.merge_requests.load(Ordering::SeqCst), 1);
    let requests = harness.requests();
    let first = &requests[0]["headRevision"];
    assert!(
        requests
            .iter()
            .any(|request| request["headRevision"] != *first)
    );
    assert_ne!(
        first.as_str().unwrap(),
        git_output(&harness.repo.workspace, &["rev-parse", "HEAD"])
    );
}

#[tokio::test]
async fn trusted_acceptance_cannot_publish_a_mutated_candidate_workspace() {
    let harness = ValidationHarness::new(
        Script::NoCi,
        "printf changed > injected.txt\nprintf '{\"verdict\":\"accepted\"}'",
    );
    let outcome = harness.run().await;
    assert!(
        matches!(outcome, Err(NodeRunnerError::DriverDetail(ref reason))
        if reason.contains("changed the candidate workspace")),
        "{outcome:?}"
    );
    harness.assert_no_publication();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn adapter_cancellation_waits_for_the_trusted_helper_process_tree_to_disappear() {
    let harness = ValidationHarness::new(
        Script::NoCi,
        "/bin/sleep 60 &\nprintf '%s\\n%s\\n' \"$$\" \"$!\" > \"$0.pids\"\nwait",
    );
    let mut handle = harness.start(DeliveryMode::MergeV3).await;
    wait_for_marker(&harness.sidecar(".pids")).await;
    let pids = fs::read_to_string(harness.sidecar(".pids"))
        .unwrap()
        .lines()
        .map(|line| line.parse::<i32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        pids.len(),
        2,
        "both helper and descendant must be observed before cancellation"
    );
    handle.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.completion())
        .await
        .expect("adapter returned without settling trusted-helper cleanup");
    assert!(
        matches!(outcome, Err(NodeRunnerError::Cancelled)),
        "{outcome:?}"
    );
    for pid in pids {
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "trusted helper process {pid} survived settlement"
        );
    }
    harness.assert_no_publication();
    harness.assert_private_exchange();
}

#[tokio::test]
async fn terminal_remote_reviews_settle_before_a_refusing_hook_or_late_feedback_read() {
    for (script, closed) in [(Script::NoCi, false), (Script::TerminalClosed, true)] {
        let harness = ValidationHarness::new(
            script,
            r#"case "$request" in
    *'"stage":"merge"'*) printf terminal > "$0.terminal" ;;
    *'"stage":"review"'*)
        if [ -f "$0.terminal" ]; then
            printf '{"verdict":"refused","reason":"A terminal review must bypass this hook"}'
            exit 0
        fi ;;
esac
printf '{"verdict":"accepted"}'"#,
        );
        if closed {
            fs::write(harness.sidecar(".terminal"), "closed").unwrap();
        }
        let outcome = harness.run().await.unwrap();
        if closed {
            assert_eq!(
                outcome,
                WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
            );
        } else {
            assert_delivery_signal(&outcome, DELIVERY_MERGED_LABEL);
        }
        let expected = usize::from(!closed);
        assert_eq!(
            harness.authority.merge_requests.load(Ordering::SeqCst),
            expected
        );
        assert_eq!(
            harness.authority.feedback_reads.load(Ordering::SeqCst),
            expected
        );
        assert_eq!(
            harness
                .requests()
                .iter()
                .filter(|request| request["stage"] == "review")
                .count(),
            expected,
            "terminal observation must bypass the hook and any late feedback query"
        );
    }
}

#[tokio::test]
async fn pending_publication_validation_honors_the_callers_finite_poll_policy() {
    let harness = ValidationHarness::with_poll(
        Script::NoCi,
        r#"printf '{"verdict":"pending"}'"#,
        DeliveryPollPolicy::new(3, Duration::from_millis(2)).unwrap(),
    );
    let mut handle = harness.start(DeliveryMode::MergeV3).await;
    let completed = tokio::time::timeout(Duration::from_secs(10), handle.completion()).await;
    if completed.is_err() {
        handle.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(10), handle.completion())
            .await
            .expect("pending-publication cleanup did not settle");
        assert!(
            matches!(stopped, Err(NodeRunnerError::Cancelled)),
            "{stopped:?}"
        );
    }
    let completion = completed
        .expect("pending publication ignored the finite poll policy")
        .unwrap();
    assert_eq!(
        completion.outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
    );
    harness.assert_no_publication();
    let requests = harness.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| request["stage"] == "publish"));
    let mut output = handle.take_initial_output().unwrap();
    let mut timed_out = false;
    while let Ok(event) = output.recv().await {
        if let crate::native_v2_runner::DurableNodeEvent::Output { output, .. } = event {
            timed_out |= output
                .text
                .contains("trusted publication validation timed out");
        }
    }
    assert!(
        timed_out,
        "missing publication validation timeout diagnostic"
    );
}

#[tokio::test]
async fn trusted_post_publication_repair_retains_the_published_receipt_without_merging() {
    for stage in ["published", "review", "merge"] {
        let harness = ValidationHarness::at_stage(
            stage,
            r#"{"verdict":"repair","reason":"Recheck the current regression evidence"}"#,
        );
        let outcome = harness.run().await.unwrap();
        let receipt = assert_delivery_signal(&outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
        assert!(outcome_diagnostic(&outcome).contains("Recheck the current regression evidence"));
        let published = harness.assert_published_without_merge();
        assert_eq!(receipt["pullRequestId"], "17");
        assert_eq!(receipt["repository"], published.target.repository);
        assert_eq!(receipt["targetBranch"], published.target.target_branch);
        assert_eq!(receipt["headRevision"], published.head_revision);
        assert_eq!(receipt["mergeRevision"], "");
        let requests = harness.requests();
        let rejected = requests.last().unwrap();
        assert_eq!(rejected["stage"], stage);
        assert_eq!(rejected["reviewId"], "17");
        assert_eq!(rejected["headRevision"], receipt["headRevision"]);
        assert_eq!(rejected["tree"], requests[0]["tree"]);
        harness.assert_private_exchange();
    }
}

#[tokio::test]
async fn trusted_post_publication_refusal_stops_without_agent_repair_or_merge() {
    for stage in ["published", "review", "merge"] {
        let harness = ValidationHarness::at_stage(
            stage,
            r#"{"verdict":"refused","reason":"Backlog evidence no longer authorizes this fix"}"#,
        );
        let outcome = harness.run().await;
        assert!(
            matches!(outcome, Err(NodeRunnerError::DriverDetail(ref reason))
                if reason.contains("Backlog evidence no longer authorizes this fix")),
            "stage {stage}: {outcome:?}"
        );
        harness.assert_published_without_merge();
        let requests = harness.requests();
        let refused = requests.last().unwrap();
        assert_eq!(refused["stage"], stage);
        assert_eq!(refused["reviewId"], "17");
        assert_eq!(refused["headRevision"], requests[0]["headRevision"]);
        harness.assert_private_exchange();
    }
}

async fn queued_validation(
    script: Script,
    response: &str,
) -> (ValidationHarness, Result<WorkerOutcome, NodeRunnerError>) {
    let harness = ValidationHarness::new(
        script,
        &format!(
            r#"if [ -f "$0.reject" ]; then printf '%s' '{response}'; else printf '{{"verdict":"accepted"}}'; fi"#
        ),
    );
    let mut handle = harness.start(DeliveryMode::MergeV3).await;
    wait_for_live_queue(&harness).await;
    fs::write(harness.sidecar(".reject"), "reject").unwrap();
    let outcome = handle
        .completion()
        .await
        .map(|completion| completion.outcome);
    (harness, outcome)
}

async fn wait_for_live_queue(harness: &ValidationHarness) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !harness.authority.queue_live.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("delivery never queued the candidate");
}

const QUEUED_REPAIR: &str = r#"{"verdict":"repair","reason":"Queued evidence needs another pass"}"#;

#[tokio::test]
async fn queued_host_repair_and_refusal_withdraw_before_handoff() {
    for response in [
        QUEUED_REPAIR,
        r#"{"verdict":"refused","reason":"Claim no longer authorizes the queued fix"}"#,
    ] {
        let (harness, outcome) = queued_validation(Script::Queued, response).await;
        if response == QUEUED_REPAIR {
            let receipt =
                assert_delivery_signal(&outcome.unwrap(), DELIVERY_REPAIR_REQUIRED_LABEL).clone();
            assert_eq!(receipt["pullRequestId"], "17");
        } else {
            assert!(
                matches!(outcome, Err(NodeRunnerError::DriverDetail(ref reason)) if reason.contains("Claim no longer authorizes"))
            );
        }
        assert!(!harness.authority.queue_live.load(Ordering::SeqCst));
        assert_eq!(harness.authority.withdrawn_reviews.lock().unwrap().len(), 1);
        assert_eq!(harness.authority.merge_requests.load(Ordering::SeqCst), 1);
        harness.assert_private_exchange();
    }
}

#[tokio::test]
async fn queued_feedback_is_replayed_after_failed_withdrawal_then_safely_handed_to_repair() {
    let harness = ValidationHarness::new(
        Script::QueuedFeedbackWithdrawalFails,
        r#"printf '{"verdict":"accepted"}'"#,
    );
    let first = harness.run().await;
    assert!(
        matches!(first, Err(NodeRunnerError::DriverDetail(ref reason)) if reason.contains("Cannot confirm safe merge withdrawal") && reason.contains("acme/project#17") && !reason.contains("test-token"))
    );
    assert!(harness.authority.queue_live.load(Ordering::SeqCst));
    harness
        .authority
        .withdrawal_allowed
        .store(true, Ordering::SeqCst);
    let repaired = harness.run().await.unwrap();
    assert_delivery_signal(&repaired, DELIVERY_REPAIR_REQUIRED_LABEL);
    assert!(outcome_diagnostic(&repaired).contains("Handle the empty input"));
    assert!(!harness.authority.queue_live.load(Ordering::SeqCst));
    assert_eq!(harness.authority.merge_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refused_queue_withdrawal_never_authorizes_a_fixer() {
    let (harness, outcome) = queued_validation(Script::QueuedWithdrawalFails, QUEUED_REPAIR).await;
    assert!(
        matches!(outcome, Err(NodeRunnerError::DriverDetail(ref reason)) if reason.contains("No repair handoff is authorized") && reason.contains("Queued evidence needs another pass"))
    );
    assert!(harness.authority.queue_live.load(Ordering::SeqCst));
}

#[tokio::test]
async fn an_exact_merge_during_withdrawal_wins_over_a_late_repair() {
    let (harness, outcome) = queued_validation(Script::QueuedMergeRace, QUEUED_REPAIR).await;
    let outcome = outcome.unwrap();
    let receipt = assert_delivery_signal(&outcome, DELIVERY_MERGED_LABEL);
    assert_eq!(receipt["mergeRevision"], "c".repeat(40));
    assert!(!harness.authority.queue_live.load(Ordering::SeqCst));
}

#[tokio::test]
async fn cancellation_withdraws_a_live_queue_before_releasing_the_run() {
    let harness = ValidationHarness::new(Script::Queued, r#"printf '{"verdict":"accepted"}'"#);
    let mut handle = harness.start(DeliveryMode::MergeV3).await;
    wait_for_live_queue(&harness).await;
    handle.cancel();
    assert!(matches!(
        handle.completion().await,
        Err(NodeRunnerError::Cancelled)
    ));
    assert!(!harness.authority.queue_live.load(Ordering::SeqCst));
    assert_eq!(harness.authority.withdrawn_reviews.lock().unwrap().len(), 1);
}
