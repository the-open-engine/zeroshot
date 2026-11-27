//! Required-checks, mergeability and readiness acceptance for the contributed Forgejo adapter.
//! Every read goes through the loopback fixture, so each test also shows which requests the
//! observation made. Each test names the acceptance criterion it verifies, from `K1` to `K17`.
use openengine_cluster_testkit::assertions::{AssertAt, AssertError, AssertValue};
use serde_json::{Value, json};

use openengine_cluster_protocol::SourceForgeBaseUrl;

use crate::source_code_provider::{
    SourceChecks, SourceDeliveryCredential, SourceDeliveryError, SourceReviewState,
};

use super::api::{Api, PAGE_SIZE};
use super::authority::ForgejoDeliveryAuthority;
use super::checks::{MAX_DIAGNOSTIC_CHARS, MAX_LINE_CHARS, OpenReview, context_matches, observe_open};
use super::http_fixture::{Fixture, Response};
use super::wire;

const TOKEN: &str = "forgejo-test-token";
const BASE: &str = "https://forge.example";
const REPOSITORY: &str = "owner/name";
const REVISION: &str = "0123456789012345678901234567890123456789";
const BASE_REVISION: &str = "1111111111111111111111111111111111111111";
const TARGET_BRANCH: &str = "main";
const PULL_URI: &str = "/api/v1/repos/owner/name/pulls/7";

fn credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token(TOKEN)
}

async fn authority(fixture: &Fixture) -> ForgejoDeliveryAuthority {
    ForgejoDeliveryAuthority::for_test(
        SourceForgeBaseUrl::new(BASE).assert_value(),
        Api::loopback(&fixture.endpoint()).assert_value(),
    )
}

async fn observe(
    fixture: &Fixture,
    pull: &wire::PullRequest,
) -> Result<OpenReview, SourceDeliveryError> {
    let authority = authority(fixture).await;
    observe_open(&authority, REPOSITORY, pull, credential()).await
}

fn pull_value(mergeable: bool, draft: bool) -> Value {
    json!({
        "number": 7,
        "state": "open",
        "draft": draft,
        "merged": false,
        "mergeable": mergeable,
        "head": { "sha": REVISION },
        "base": { "ref": TARGET_BRANCH, "sha": BASE_REVISION },
    })
}

fn pull(mergeable: bool, draft: bool) -> wire::PullRequest {
    serde_json::from_value(pull_value(mergeable, draft))
        .expect("the pull response reads as the wire shape")
}

fn pull_response(mergeable: bool, draft: bool) -> Response {
    Response::json(pull_value(mergeable, draft))
}

fn branch(enforced: bool, contexts: &[&str], required_approvals: u64) -> Response {
    Response::json(json!({
        "name": TARGET_BRANCH,
        "commit": { "id": BASE_REVISION },
        "unknown": "the wire shape ignores unknown fields",
        "enable_status_check": enforced,
        "status_check_contexts": contexts,
        "required_approvals": required_approvals,
    }))
}

fn status(id: u64, context: &str, status: &str, description: &str) -> Value {
    json!({
        "id": id,
        "context": context,
        "status": status,
        "description": description,
        "unknown": "the wire shape ignores unknown fields",
    })
}

struct Review {
    id: u64,
    state: &'static str,
    official: bool,
    stale: bool,
    dismissed: bool,
}

fn review_value(review: &Review) -> Value {
    json!({
        "id": review.id,
        "state": review.state,
        "official": review.official,
        "stale": review.stale,
        "dismissed": review.dismissed,
        "user": { "login": "someone" },
    })
}

/// A page response carries the total the pagination contract reads.
fn page(entries: &[Value]) -> Response {
    Response::json(json!(entries)).header("x-total-count", &entries.len().to_string())
}

fn branch_uri() -> String {
    format!("/api/v1/repos/{REPOSITORY}/branches/{TARGET_BRANCH}")
}

fn statuses_uri() -> String {
    format!("/api/v1/repos/{REPOSITORY}/commits/{REVISION}/statuses?limit={PAGE_SIZE}&page=1")
}

fn reviews_uri() -> String {
    format!("/api/v1/repos/{REPOSITORY}/pulls/7/reviews?limit={PAGE_SIZE}&page=1")
}

fn open(checks: SourceChecks) -> SourceReviewState {
    SourceReviewState::Open { checks }
}

/// K1: a required context that passed is a pass, and readiness needs approvals, so nothing is
/// read for it.
#[tokio::test]
async fn k1_required_context_passed_without_approvals_reads_no_reviews() {
    let fixture = Fixture::new().await;
    fixture
        .queue(branch(true, &["ci"], 0))
        .queue(page(&[status(1, "ci", "success", "ok")]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Passed));
    assert!(
        !review.pull_request_ready,
        "nothing is waiting for a person"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests.assert_at(0).uri, branch_uri());
    assert_eq!(requests.assert_at(1).uri, statuses_uri());
}

/// K2: a failed required context reports the context, the status and the description.
#[tokio::test]
async fn k2_failed_required_context_reports_context_status_and_description() {
    let fixture = Fixture::new().await;
    fixture.queue(branch(true, &["ci"], 0)).queue(page(&[status(
        1,
        "ci",
        "failure",
        "3 tests failed",
    )]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(
        review.state,
        open(SourceChecks::Failed {
            diagnostic: "ci: failure: 3 tests failed".to_owned(),
        })
    );
    assert!(!review.pull_request_ready);
    assert_eq!(
        fixture.requests().len(),
        2,
        "no review read for a failed check"
    );
}

/// K3: a required context that has not reported yet keeps the delivery waiting.
#[tokio::test]
async fn k3_pending_required_context_stays_pending() {
    let fixture = Fixture::new().await;
    fixture
        .queue(branch(true, &["ci"], 0))
        .queue(page(&[status(1, "ci", "pending", "")]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Pending));
    assert!(!review.pull_request_ready);
}

/// K4: a required context with no status at all is not a pass by absence.
#[tokio::test]
async fn k4_required_context_without_any_status_stays_pending() {
    let fixture = Fixture::new().await;
    fixture.queue(branch(true, &["ci"], 0)).queue(page(&[]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Pending));
    assert!(!review.pull_request_ready);
}

/// K17: every named context has to report, so one passed context does not stand in for another.
#[tokio::test]
async fn k17_a_second_required_context_without_a_status_stays_pending() {
    let fixture = Fixture::new().await;
    fixture
        .queue(branch(true, &["build", "test"], 0))
        .queue(page(&[status(1, "build", "success", "")]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Pending));
    assert!(!review.pull_request_ready);
}

/// K5: a failure outside the required contexts does not decide the result.
#[tokio::test]
async fn k5_failure_outside_the_required_contexts_is_ignored() {
    let fixture = Fixture::new().await;
    fixture.queue(branch(true, &["ci/*"], 0)).queue(page(&[
        status(1, "ci/build", "success", "built"),
        status(2, "deploy", "failure", "ignored by the rule"),
    ]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Passed));
    assert!(!review.pull_request_ready);
}

/// K6: the newest entry for a context decides, whichever order the history arrives in.
#[tokio::test]
async fn k6_newest_entry_per_context_decides_regardless_of_response_order() {
    let older_failure = Fixture::new().await;
    older_failure.queue(branch(true, &["ci"], 0)).queue(page(&[
        status(9, "ci", "success", "newer run"),
        status(3, "ci", "failure", "older run"),
    ]));
    let review = observe(&older_failure, &pull(true, false))
        .await
        .assert_value();
    assert_eq!(review.state, open(SourceChecks::Passed));

    let older_success = Fixture::new().await;
    older_success.queue(branch(true, &["ci"], 0)).queue(page(&[
        status(3, "ci", "success", "older run"),
        status(9, "ci", "failure", "newer run"),
    ]));
    let review = observe(&older_success, &pull(true, false))
        .await
        .assert_value();
    assert_eq!(
        review.state,
        open(SourceChecks::Failed {
            diagnostic: "ci: failure: newer run".to_owned(),
        })
    );
}

/// K7: an enforced check with no contexts and no status keeps the delivery waiting.
#[tokio::test]
async fn k7_enforced_without_contexts_and_no_status_stays_pending() {
    let fixture = Fixture::new().await;
    fixture.queue(branch(true, &[], 0)).queue(page(&[]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Pending));
    assert!(!review.pull_request_ready);
}

/// K8: nothing enforced and nothing reported means no check is required.
#[tokio::test]
async fn k8_nothing_enforced_and_no_status_is_not_required() {
    let fixture = Fixture::new().await;
    fixture.queue(branch(false, &[], 0)).queue(page(&[]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::NotRequired));
    assert!(!review.pull_request_ready, "readiness needs approvals");
}

/// K9: a failure counts even when no check is enforced.
#[tokio::test]
async fn k9_failure_counts_without_an_enforced_check() {
    let fixture = Fixture::new().await;
    fixture.queue(branch(false, &[], 0)).queue(page(&[status(
        1,
        "ci",
        "error",
        "runner could not start",
    )]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(
        review.state,
        open(SourceChecks::Failed {
            diagnostic: "ci: error: runner could not start".to_owned(),
        })
    );
}

/// K10: `warning` and `skipped` pass, and a result the table does not name is an API error.
#[tokio::test]
async fn k10_warning_and_skipped_pass_and_an_unknown_result_is_an_api_error() {
    let fixture = Fixture::new().await;
    fixture.queue(branch(true, &["ci"], 0)).queue(page(&[
        status(1, "ci", "warning", "flaky"),
        status(2, "docs", "skipped", ""),
    ]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Passed));

    let unknown = Fixture::new().await;
    unknown.queue(branch(true, &["ci"], 0)).queue(page(&[status(
        1,
        "ci",
        "neutral",
        "not a result",
    )]));

    let error = observe(&unknown, &pull(true, false)).await.assert_error();
    assert_eq!(error.api_status(), None);
    assert_eq!(
        format!("{error}"),
        "source API request failed: Forgejo status ci reported neutral as its result"
    );
}

/// K11: fewer valid approvals than the rule requires means the pull request waits for a person,
/// and a stale or dismissed entry does not count.
#[tokio::test]
async fn k11_one_valid_approval_of_two_required_is_ready_and_reads_reviews() {
    let fixture = Fixture::new().await;
    fixture
        .queue(branch(true, &["ci"], 2))
        .queue(page(&[status(1, "ci", "success", "ok")]))
        .queue(page(&[
            review_value(&Review {
                id: 1,
                state: "APPROVED",
                official: true,
                stale: false,
                dismissed: false,
            }),
            review_value(&Review {
                id: 2,
                state: "APPROVED",
                official: true,
                stale: true,
                dismissed: false,
            }),
            review_value(&Review {
                id: 3,
                state: "APPROVED",
                official: true,
                stale: false,
                dismissed: true,
            }),
            review_value(&Review {
                id: 4,
                state: "COMMENTED",
                official: false,
                stale: false,
                dismissed: false,
            }),
        ]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Passed));
    assert!(review.pull_request_ready, "one approval of two is waiting");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(requests.assert_at(2).uri, reviews_uri());
}

/// K12: every required approval is present, so the pull request is not waiting for a person.
#[tokio::test]
async fn k12_all_required_approvals_present_is_not_ready() {
    let fixture = Fixture::new().await;
    fixture
        .queue(branch(true, &[], 2))
        .queue(page(&[
            status(1, "ci", "success", "ok"),
            status(2, "docs", "skipped", ""),
        ]))
        .queue(page(&[
            review_value(&Review {
                id: 1,
                state: "APPROVED",
                official: true,
                stale: false,
                dismissed: false,
            }),
            review_value(&Review {
                id: 2,
                state: "APPROVED",
                official: true,
                stale: false,
                dismissed: false,
            }),
        ]));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Passed));
    assert!(
        !review.pull_request_ready,
        "the checks are not waiting for a person"
    );
}

/// K13: an ambiguous `mergeable` that turns true on the second read continues with the checks.
#[tokio::test]
async fn k13_mergeable_turning_true_on_the_second_read_continues_with_the_checks() {
    let fixture = Fixture::new().await;
    fixture
        .queue(pull_response(false, false))
        .queue(pull_response(true, false))
        .queue(branch(true, &["ci"], 0))
        .queue(page(&[status(1, "ci", "success", "ok")]));

    let review = observe(&fixture, &pull(false, false)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Passed));
    assert!(!review.pull_request_ready);

    let requests = fixture.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_eq!(requests.assert_at(0).uri, PULL_URI);
    assert_eq!(requests.assert_at(1).uri, PULL_URI);
    assert_eq!(requests.assert_at(2).uri, branch_uri());
    assert_eq!(requests.assert_at(3).uri, statuses_uri());
}

/// K14: an ambiguous `mergeable` through every read is the conflict state, and nothing else is
/// read.
#[tokio::test]
async fn k14_mergeable_false_on_every_read_is_conflict_after_three_reads() {
    let fixture = Fixture::new().await;
    for _ in 0..3 {
        fixture.queue(pull_response(false, false));
    }

    let review = observe(&fixture, &pull(false, false)).await.assert_value();
    assert_eq!(review.state, SourceReviewState::Conflict);
    assert!(!review.pull_request_ready);

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    for request in &requests {
        assert_eq!(request.uri, PULL_URI, "{request:?}");
    }
}

/// K15: a draft is never ready, and its checks are not read at all.
#[tokio::test]
async fn k15_draft_reports_pending_without_reading_branch_or_statuses() {
    let fixture = Fixture::new().await;

    let review = observe(&fixture, &pull(true, true)).await.assert_value();
    assert_eq!(review.state, open(SourceChecks::Pending));
    assert!(!review.pull_request_ready);

    let requests = fixture.requests();
    assert_eq!(requests.len(), 0, "{requests:?}");
}

/// K16: pattern matching, the diagnostic bounds, and the token staying out of the diagnostic.
#[tokio::test]
async fn k16_pattern_matching_line_bounds_and_token_redaction() {
    assert!(context_matches("ci", "ci"));
    assert!(context_matches("ci/*", "ci/build"));
    assert!(context_matches("build-?", "build-1"));
    assert!(!context_matches("build-?", "build-12"));
    assert!(!context_matches("ci", "test"));
    assert!(!context_matches("ci/[ab]", "ci/a"));
    assert!(context_matches("ci/[ab]", "ci/[ab]"));

    let long = "x".repeat(2000);
    let fixture = Fixture::new().await;
    let entries = (1_u64..=10)
        .map(|index| {
            status(
                index,
                &format!("ci-{index}"),
                "failure",
                &format!("{TOKEN} {long}"),
            )
        })
        .collect::<Vec<Value>>();
    fixture.queue(branch(false, &[], 0)).queue(page(&entries));

    let review = observe(&fixture, &pull(true, false)).await.assert_value();
    let SourceReviewState::Open {
        checks: SourceChecks::Failed { diagnostic },
    } = review.state
    else {
        panic!("every counted failure is reported as a failed check");
    };
    assert_eq!(diagnostic.chars().count(), MAX_DIAGNOSTIC_CHARS);
    assert_eq!(diagnostic.lines().count(), 8);
    for line in diagnostic.lines() {
        assert!(line.chars().count() <= MAX_LINE_CHARS, "{line}");
    }
    assert!(!diagnostic.contains(TOKEN), "{diagnostic}");
    assert!(diagnostic.contains("[REDACTED]"), "{diagnostic}");
    assert!(diagnostic.starts_with("ci-1: failure: "), "{diagnostic}");
}

/// K18: a changed pull is a temporary error before any rule or status reads.
#[tokio::test]
async fn k18_a_pull_changed_during_the_recheck_is_a_temporary_error() {
    for (field, value, diagnostic) in [
        ("/state", json!("closed"), "closed as its state"),
        ("/head/sha", json!(BASE_REVISION), "as its head revision"),
        ("/base/ref", json!("other-branch"), "as its target branch"),
        (
            "/base",
            json!({
                "ref": TARGET_BRANCH,
                "sha": BASE_REVISION,
                "repo": { "full_name": "other/name" },
            }),
            "another target repository",
        ),
    ] {
        let mut changed = pull_value(true, false);
        *changed.pointer_mut(field).assert_value() = value;
        let fixture = Fixture::new().await;
        fixture.queue(Response::json(changed));

        let error = observe(&fixture, &pull(false, false)).await.assert_error();
        assert!(error.retryable_operation(), "{error}");
        assert!(error.to_string().contains(diagnostic), "{error}");

        let requests = fixture.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(requests.assert_at(0).uri, PULL_URI, "{requests:?}");
    }
}
