//! Real GitHub policy reads inside the native delivery driver; Git transport stays local.
use super::*;
use crate::native_v2_contract::NodeCompletion;
use crate::native_v2_runner::{DurableNodeEvent, DurableOutput, NodeHandle};
use crate::source_code_provider::adapters::core::{
    github::test_policy_page,
    http_fixture::{self, IntoResponse, Json, Request, Response},
};
use reqwest::{Method, StatusCode};
use std::sync::Mutex;

const MERGE_SHA: &str = "cccccccccccccccccccccccccccccccccccccccc";

const RUN: &str = "policy-read-retry";
const MESSAGE: &str = "Something went wrong while executing your query on 2026-11-21T18:29:14Z. Please include `2790:35FB30:99F13F9:E90DBDA:6B01E2F9` when reporting this issue.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Snapshot,
    Method,
    Reconciliation,
    Pending,
}

#[derive(Clone, Copy, Default)]
struct Calls {
    snapshot: usize,
    method: usize,
    reconciliation: usize,
    submissions: usize,
    merged: bool,
}

struct Scenario {
    stage: Stage,
    failures: usize,
    policy: Value,
    calls: Arc<Mutex<Calls>>,
}

impl Scenario {
    fn response(&self, request: Request) -> Response {
        assert_eq!(request.headers["authorization"], "Bearer test-token");
        if request.method == Method::GET {
            assert_eq!(
                request.uri.path(),
                "/repos/acme/project/rules/branches/main"
            );
            return Json(json!([])).into_response();
        }
        assert_eq!(request.method, Method::POST);
        assert_eq!(request.uri.path(), "/graphql");
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let query = body["query"].as_str().unwrap();
        let mut calls = self.calls.lock().unwrap();
        if query.contains("mergePullRequest") {
            return self.submit(&body["variables"], &mut calls);
        }
        let stage = if query.contains("query MergePolicy") {
            Stage::Method
        } else {
            Stage::Snapshot
        };
        self.read(stage, &mut calls)
    }

    fn read(&self, stage: Stage, calls: &mut Calls) -> Response {
        let count = if stage == Stage::Snapshot {
            calls.snapshot += 1;
            calls.snapshot
        } else {
            calls.method += 1;
            calls.method
        };
        if self.stage == Stage::Reconciliation
            && stage == Stage::Snapshot
            && calls.submissions == 1
            && calls.reconciliation < 3
        {
            calls.reconciliation += 1;
            return query_error();
        }
        if stage == self.stage && count <= self.failures {
            return query_error();
        }
        let mut policy = self.policy.clone();
        if calls.merged {
            let pull = &mut policy["data"]["repository"]["pullRequest"];
            pull["state"] = json!("MERGED");
            pull["merged"] = json!(true);
            pull["mergeCommit"] = json!({"oid": MERGE_SHA});
        }
        Json(policy).into_response()
    }

    fn submit(&self, variables: &Value, calls: &mut Calls) -> Response {
        let pull = &self.policy["data"]["repository"]["pullRequest"];
        assert_eq!(
            variables,
            &json!({"pullRequestId":pull["id"],"method":"MERGE","expectedHeadOid":pull["headRefOid"]})
        );
        calls.submissions += 1;
        if self.stage == Stage::Reconciliation && calls.submissions == 1 {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message":"unavailable test-token"})),
            )
                .into_response();
        }
        calls.merged = true;
        Json(json!({"data":{"mergePullRequest":{"pullRequest":{
            "id":pull["id"], "number":pull["number"], "repository":{"nameWithOwner":"acme/project"},
            "baseRefName":pull["baseRefName"], "headRefName":pull["headRefName"], "headRefOid":pull["headRefOid"],
            "merged":true, "mergeCommit":{"oid":MERGE_SHA}
        }}}})).into_response()
    }
}

fn query_error() -> Response {
    Json(json!({"errors":[{"message":MESSAGE}]})).into_response()
}

struct PolicyFixture {
    calls: Arc<Mutex<Calls>>,
    server: tokio::task::JoinHandle<()>,
}

impl PolicyFixture {
    fn calls(&self) -> Calls {
        *self.calls.lock().unwrap()
    }
}

impl Drop for PolicyFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn fixture(
    repo: &TempRepo,
    stage: Stage,
    failures: usize,
    poll: DeliveryPollPolicy,
) -> (Arc<FakeGitHub>, Arc<NativeV2DeliveryAdapter>, PolicyFixture) {
    crate::native_v2_candidate::test_support::commit_all(
        &repo.workspace,
        "fix: reviewed candidate",
    );
    let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
    let (mergeable, state) = if stage == Stage::Pending {
        ("UNKNOWN", "BLOCKED")
    } else {
        ("MERGEABLE", "CLEAN")
    };
    let mut policy = test_policy_page(mergeable, state, None, (false, None));
    let pull = &mut policy["data"]["repository"]["pullRequest"];
    pull["headRefName"] = json!(delivery_head_branch(&RunId::new(RUN)));
    pull["headRefOid"] = json!(head);
    let base = &mut pull["baseRef"];
    base["target"]["oid"] = json!(repo.base);
    base["compare"]["baseTarget"]["oid"] = json!(repo.base);
    base["compare"]["headTarget"]["oid"] = json!(head);
    let (listener, address) = http_fixture::loopback().await;
    let calls = Arc::new(Mutex::new(Calls::default()));
    let scenario = Scenario {
        stage,
        failures,
        policy,
        calls: calls.clone(),
    };
    let server = tokio::spawn(http_fixture::serve(listener, move |request| {
        scenario.response(request)
    }));
    let mut authority = FakeGitHub::new(repo.remote.clone(), Script::NoCi);
    authority.policy_authority = Some(
        GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            api_deadline: Duration::from_secs(15),
            ..GhCliAuthorityConfig::hosted(repo.root.path().into())
        })
        .with_test_api(&address),
    );
    let authority = Arc::new(authority);
    let adapter = retained_adapter(
        repo,
        authority.clone(),
        poll,
        DeliveryLineage::original(RUN),
    );
    (authority, adapter, PolicyFixture { calls, server })
}

fn request(repo: &TempRepo) -> DeliveryRunRequest<'_> {
    DeliveryRunRequest {
        repo,
        attempts: 1,
        mode: DeliveryMode::Merge,
        run_id: RUN,
        refresh: None,
    }
}

fn poll_policies() -> [DeliveryPollPolicy; 2] {
    [
        DeliveryPollPolicy::default(),
        DeliveryPollPolicy::new(8, Duration::ZERO).unwrap(),
    ]
}

async fn start(
    repo: &TempRepo,
    adapter: Arc<NativeV2DeliveryAdapter>,
) -> (NodeHandle, DurableOutput) {
    let mut handle = start_with_adapter_token(
        request(repo),
        adapter,
        PullRequestFeedback::Consider,
        Some("test-token"),
    )
    .await;
    let events = handle.take_initial_output().unwrap();
    (handle, events)
}

async fn drain(mut events: DurableOutput) -> String {
    let mut diagnostic = String::new();
    while let Ok(event) = events.recv().await {
        match event {
            DurableNodeEvent::DeliveryState(state) => events
                .acknowledge_delivery_state(state.generation.get())
                .unwrap(),
            DurableNodeEvent::Output { output, .. } => {
                diagnostic.push_str(&output.text);
                diagnostic.push('\n');
            }
            DurableNodeEvent::TokenUsage(_) => {}
        }
    }
    diagnostic
}

async fn run_with_diagnostics(
    repo: &TempRepo,
    adapter: Arc<NativeV2DeliveryAdapter>,
) -> (Result<NodeCompletion, NodeRunnerError>, String) {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (mut handle, events) = start(repo, adapter).await;
        tokio::join!(handle.completion(), drain(events))
    })
    .await
    .unwrap()
}

fn assert_exhausted_diagnostic(diagnostic: &str) {
    assert!(diagnostic.contains("retry allowance exhausted"));
    assert!(!diagnostic.contains("test-token"));
}

fn assert_withdrawal_attempt_preserves_head(repo: &TempRepo, authority: &FakeGitHub) {
    assert_eq!(authority.withdrawn_reviews.lock().unwrap().len(), 1);
    let review = &authority.review_requests()[0];
    assert_eq!(
        git_output(&repo.remote, &["rev-parse", &review.head_branch]),
        review.head_revision
    );
}

#[tokio::test]
async fn policy_read_exhaustion_cannot_restart_in_default_or_finite_delivery_polls() {
    for stage in [Stage::Snapshot, Stage::Method] {
        for poll in poll_policies() {
            let repo = TempRepo::delivery();
            let (authority, adapter, fixture) = fixture(&repo, stage, 3, poll).await;
            let started = std::time::Instant::now();
            let (completion, diagnostic) = run_with_diagnostics(&repo, adapter).await;
            assert_eq!(
                completion.unwrap().outcome,
                WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
            );
            assert_exhausted_diagnostic(&diagnostic);
            assert!(started.elapsed() >= Duration::from_secs(3));
            // A fresh successful read confirms mandatory withdrawal; the poll must never resubmit.
            let calls = fixture.calls();
            if stage == Stage::Snapshot {
                assert_eq!(calls.snapshot, 4);
            } else {
                assert_eq!(calls.method, 3);
            }
            assert_eq!(calls.submissions, 0);
            assert_withdrawal_attempt_preserves_head(&repo, &authority);
        }
    }
}

#[tokio::test]
async fn policy_read_recovery_submits_once_through_the_delivery_driver() {
    for stage in [Stage::Snapshot, Stage::Method] {
        let repo = TempRepo::delivery();
        let (_, adapter, fixture) = fixture(
            &repo,
            stage,
            2,
            DeliveryPollPolicy::new(8, Duration::ZERO).unwrap(),
        )
        .await;
        let execution = run_with_adapter(request(&repo), adapter).await;
        assert_delivery_signal(&execution.outcome, DELIVERY_MERGED_LABEL);
        let calls = fixture.calls();
        assert_eq!(calls.submissions, 1);
        assert!(calls.merged);
        assert_eq!(calls.method, if stage == Stage::Method { 3 } else { 1 });
    }
}

#[tokio::test]
async fn policy_read_reconciliation_exhaustion_never_restarts_or_resubmits() {
    for poll in poll_policies() {
        let repo = TempRepo::delivery();
        let (authority, adapter, fixture) = fixture(&repo, Stage::Reconciliation, 0, poll).await;
        let (completion, diagnostic) = run_with_diagnostics(&repo, adapter).await;
        assert_eq!(
            fixture.calls().submissions,
            1,
            "an exhausted reconciliation must not permit another mutation"
        );
        assert_eq!(fixture.calls().reconciliation, 3);
        assert_exhausted_diagnostic(&diagnostic);
        assert!(diagnostic.contains("HTTP 503"));
        let Err(NodeRunnerError::DriverDetail(reason)) = completion else {
            panic!("expected to fail closed when withdrawal cannot confirm a direct merge");
        };
        assert!(reason.contains("direct merge submission remains unconfirmed"));
        assert!(reason.contains("No repair handoff is authorized"));
        assert!(!reason.contains("test-token"));
        assert!(!fixture.calls().merged);
        assert_withdrawal_attempt_preserves_head(&repo, &authority);
    }
}

#[tokio::test]
async fn policy_read_backoff_observes_native_delivery_cancellation() {
    let repo = TempRepo::delivery();
    let (_, adapter, fixture) =
        fixture(&repo, Stage::Snapshot, 1, DeliveryPollPolicy::default()).await;
    cancel_after_reads(&repo, adapter, &fixture, 1).await;
    assert_eq!(
        fixture.calls().snapshot,
        2,
        "one failed read and mandatory withdrawal, no backoff retry"
    );
    assert_eq!(fixture.calls().submissions, 0);
}

async fn cancel_after_reads(
    repo: &TempRepo,
    adapter: Arc<NativeV2DeliveryAdapter>,
    fixture: &PolicyFixture,
    count: usize,
) {
    let (mut handle, events) = start(repo, adapter).await;
    let (completion, _) = tokio::join!(
        async {
            tokio::time::timeout(Duration::from_secs(30), async {
                while fixture.calls().snapshot < count {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                handle.cancel();
                handle.completion().await
            })
            .await
            .unwrap()
        },
        drain(events),
    );
    assert!(matches!(completion, Err(NodeRunnerError::Cancelled)));
}

#[tokio::test]
async fn policy_read_allowance_preserves_ordinary_delivery_polling() {
    let repo = TempRepo::delivery();
    let (_, adapter, pending) =
        fixture(&repo, Stage::Pending, 0, DeliveryPollPolicy::default()).await;
    cancel_after_reads(&repo, adapter, &pending, 2).await;
    assert_eq!(pending.calls().submissions, 0);
    let repo = TempRepo::delivery();
    let (_, adapter, fixture) = fixture(
        &repo,
        Stage::Pending,
        0,
        DeliveryPollPolicy::new(5, Duration::ZERO).unwrap(),
    )
    .await;
    let execution = run_with_adapter(request(&repo), adapter).await;
    assert_eq!(
        execution.outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
    );
    assert_eq!(
        fixture.calls().snapshot,
        6,
        "five healthy polls and mandatory withdrawal"
    );
    assert_eq!(fixture.calls().submissions, 0);
}
