//! Pull-request acceptance for the contributed Forgejo forge adapter. Every read and write is
//! observed through the loopback fixture, so the assertions cover what the adapter sends as well
//! as what it returns. Each test names the acceptance criterion it verifies, from `R1` to `R21`.
use openengine_cluster_testkit::assertions::{AssertAt, AssertError, AssertValue, JsonAt};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use openengine_cluster_protocol::SourceForgeBaseUrl;

use crate::source_code_provider::{
    DELIVERY_GENERATED_BODY_END, DELIVERY_GENERATED_BODY_START, DeliveryTarget, SourceChecks,
    SourceDeliveryCredential, SourceDeliveryRead, SourceReviewObservation, SourceReviewReceipt,
    SourceReviewRequest, SourceReviewState,
};

use super::api::{Api, PAGE_SIZE};
use super::authority::ForgejoDeliveryAuthority;
use super::http_fixture::{Fixture, Request, Response};
use super::review;
use super::wire;

const TOKEN: &str = "forgejo-test-token";
const BASE: &str = "https://forge.example";
const REPOSITORY: &str = "owner/name";
const SEARCH_PATH: &str = "/api/v1/repos/owner/name/pulls";
const REVISION: &str = "0123456789012345678901234567890123456789";
const OTHER_REVISION: &str = "1111111111111111111111111111111111111111";
const MERGE_REVISION: &str = "2222222222222222222222222222222222222222";
const HEAD_BRANCH: &str = "zeroshot/test";
const TARGET_BRANCH: &str = "main";
const TITLE: &str = "Fix the read";
const DESCRIPTION: &str = "Preserve behavior.";

fn credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token(TOKEN)
}

async fn authority(fixture: &Fixture) -> ForgejoDeliveryAuthority {
    ForgejoDeliveryAuthority::for_test(
        SourceForgeBaseUrl::new(BASE).assert_value(),
        Api::loopback(&fixture.endpoint()).assert_value(),
    )
}

fn target() -> DeliveryTarget {
    DeliveryTarget::new(REPOSITORY, TARGET_BRANCH, REVISION).expect("admitted delivery target")
}

fn review_request(target: &DeliveryTarget, title: &str, description: &str) -> SourceReviewRequest {
    SourceReviewRequest {
        known_review: None,
        include_terminal_reviews: false,
        target: target.clone(),
        head_branch: HEAD_BRANCH.to_owned(),
        head_revision: REVISION.to_owned(),
        title: title.to_owned(),
        description: description.to_owned(),
        source_issue: None,
    }
}

fn review_read<'a>(
    target: &'a DeliveryTarget,
    known: Option<&'a SourceReviewReceipt>,
) -> SourceDeliveryRead<'a> {
    SourceDeliveryRead {
        include_terminal_reviews: false,
        target,
        head_branch: HEAD_BRANCH,
        known_review: known,
        include_review: true,
    }
}

fn review_receipt(number: u64) -> SourceReviewReceipt {
    SourceReviewReceipt {
        review_id: number.to_string(),
        repository: REPOSITORY.to_owned(),
        target_branch: TARGET_BRANCH.to_owned(),
        head_branch: HEAD_BRANCH.to_owned(),
        head_revision: REVISION.to_owned(),
    }
}

fn observation(receipt: &SourceReviewReceipt, state: SourceReviewState) -> SourceReviewObservation {
    receipt.observation_with_readiness(state, false, false)
}

/// A pull request as the Forgejo API reports it. `head_ref` is the head branch name, which the
/// response carries in `head.label`; `head_pull_ref` is what `head.ref` reports, which is the
/// branch name only while the head branch exists and `refs/pull/<number>/head` once it is gone.
struct Pull {
    number: u64,
    state: &'static str,
    title: String,
    body: String,
    head_ref: &'static str,
    head_pull_ref: &'static str,
    head_repo: &'static str,
    head_sha: String,
    base_ref: &'static str,
    merged: bool,
    merge_sha: Option<String>,
    mergeable: bool,
    draft: bool,
}

fn pull(number: u64) -> Pull {
    Pull {
        number,
        state: "open",
        title: String::new(),
        body: String::new(),
        head_ref: HEAD_BRANCH,
        head_pull_ref: HEAD_BRANCH,
        head_repo: REPOSITORY,
        head_sha: REVISION.to_owned(),
        base_ref: TARGET_BRANCH,
        merged: false,
        merge_sha: None,
        mergeable: false,
        draft: false,
    }
}

fn pull_value(pull: &Pull) -> Value {
    json!({
        "number": pull.number,
        "state": pull.state,
        "title": pull.title,
        "body": pull.body,
        "draft": pull.draft,
        "merged": pull.merged,
        "merge_commit_sha": pull.merge_sha,
        "mergeable": pull.mergeable,
        "head": {
            "label": pull.head_ref,
            "ref": pull.head_pull_ref,
            "sha": pull.head_sha,
            "repo": { "full_name": pull.head_repo },
        },
        "base": { "ref": pull.base_ref, "sha": REVISION },
    })
}

fn pull_response(pull: &Pull) -> Response {
    Response::json(pull_value(pull))
}

fn version() -> Response {
    Response::json(json!({"version": "16.0.0"}))
}

fn repository() -> Response {
    Response::json(json!({
        "full_name": REPOSITORY,
        "html_url": format!("{BASE}/{REPOSITORY}"),
        "archived": false,
    }))
}

fn branch(sha: &str) -> Response {
    Response::json(json!({ "name": HEAD_BRANCH, "commit": { "id": sha } }))
}

/// The rule in effect for the target branch, as the branch endpoint reports it.
fn branch_rule() -> Response {
    Response::json(json!({
        "name": TARGET_BRANCH,
        "commit": { "id": REVISION },
        "enable_status_check": false,
        "status_check_contexts": [],
        "required_approvals": 0,
    }))
}

fn rule_uri() -> String {
    format!("/api/v1/repos/{REPOSITORY}/branches/{TARGET_BRANCH}")
}

fn statuses_uri() -> String {
    format!("/api/v1/repos/{REPOSITORY}/commits/{REVISION}/statuses?limit={PAGE_SIZE}&page=1")
}

fn page(total: usize, pulls: Vec<Value>) -> Response {
    Response::json(json!(pulls)).header("x-total-count", &total.to_string())
}

fn generated(description: &str) -> String {
    format!("{DELIVERY_GENERATED_BODY_START}\n{description}\n{DELIVERY_GENERATED_BODY_END}")
}

fn search_uri(state: &str) -> String {
    format!("{SEARCH_PATH}?state={state}&limit={PAGE_SIZE}&page=1")
}

fn request_body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).expect("a write sends JSON")
}

fn assert_reads(requests: &[Request]) {
    for request in requests {
        assert_eq!(request.method, Method::GET, "{request:?}");
    }
}

fn assert_writes(requests: &[Request], expected: &[Method]) {
    let methods = requests
        .iter()
        .filter(|request| request.method != Method::GET)
        .map(|request| request.method.clone())
        .collect::<Vec<Method>>();
    assert_eq!(methods, expected.to_vec(), "{requests:?}");
}

/// R1: with no pull request the write is a creation carrying head, base, the normalized title and
/// the generated body, and the receipt reports the number the forge allocated.
#[tokio::test]
async fn r1_a_new_pull_request_is_created_with_the_normalized_title_and_generated_body() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    let mut created = pull(7);
    created.title = TITLE.to_owned();
    created.body = generated(DESCRIPTION);
    fixture
        .queue(version())
        .queue(page(0, Vec::new()))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(pull_response(&created));

    let request = review_request(&target, "  WIP: [wip] Fix the read  ", DESCRIPTION);
    let receipt = review::open_or_update(&authority, &request, credential())
        .await
        .assert_value();
    assert_eq!(receipt, review_receipt(7));

    let requests = fixture.requests();
    assert_eq!(requests.len(), 7, "{requests:?}");
    assert_eq!(requests.assert_at(1).uri, search_uri("open"));
    let post = requests.assert_at(6);
    assert_eq!(post.method, Method::POST);
    assert_eq!(post.uri, SEARCH_PATH);
    let body = request_body(post);
    assert_eq!(body.assert_key("head"), &json!(HEAD_BRANCH));
    assert_eq!(body.assert_key("base"), &json!(TARGET_BRANCH));
    assert_eq!(body.assert_key("title"), &json!(TITLE));
    assert_eq!(body.assert_key("body"), &json!(generated(DESCRIPTION)));
}

/// R2: an existing pull request with the same head and the same text is not written again.
#[tokio::test]
async fn r2_an_existing_pull_with_the_same_head_and_text_sends_no_patch() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    let mut existing = pull(7);
    existing.title = TITLE.to_owned();
    existing.body = generated(DESCRIPTION);
    fixture
        .queue(version())
        .queue(page(1, vec![pull_value(&existing)]))
        .queue(repository())
        .queue(branch(REVISION));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let receipt = review::open_or_update(&authority, &request, credential())
        .await
        .assert_value();
    assert_eq!(receipt, review_receipt(7));

    let requests = fixture.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_reads(&requests);
}

/// R3: a changed title sends exactly one write, and that write is an update of the found pull.
#[tokio::test]
async fn r3_a_changed_title_sends_one_patch_and_no_post() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    let mut existing = pull(7);
    existing.title = "Earlier title".to_owned();
    existing.body = generated(DESCRIPTION);
    let mut confirmed = pull(7);
    confirmed.title = TITLE.to_owned();
    confirmed.body = generated(DESCRIPTION);
    fixture
        .queue(version())
        .queue(page(1, vec![pull_value(&existing)]))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(pull_response(&confirmed));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let receipt = review::open_or_update(&authority, &request, credential())
        .await
        .assert_value();
    assert_eq!(receipt, review_receipt(7));

    let requests = fixture.requests();
    assert_eq!(requests.len(), 5, "{requests:?}");
    assert_writes(&requests, &[Method::PATCH]);
    let patch = requests.assert_at(4);
    assert_eq!(patch.uri, format!("{SEARCH_PATH}/7"), "{requests:?}");
    let body = request_body(patch);
    assert_eq!(body.assert_key("title"), &json!(TITLE));
    assert_eq!(body.assert_key("body"), &json!(generated(DESCRIPTION)));
}

/// R4: an existing pull request that carries another head revision is an identity error, and
/// nothing is written.
#[tokio::test]
async fn r4_an_existing_pull_with_another_head_revision_is_an_identity_error_with_no_write() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    let mut existing = pull(7);
    existing.title = TITLE.to_owned();
    existing.body = generated(DESCRIPTION);
    existing.head_sha = OTHER_REVISION.to_owned();
    fixture
        .queue(version())
        .queue(page(1, vec![pull_value(&existing)]))
        .queue(repository())
        .queue(branch(OTHER_REVISION));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let error = review::open_or_update(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo pull request 7 in {REPOSITORY} (head branch \
             {HEAD_BRANCH}, base branch {TARGET_BRANCH}, head revision {OTHER_REVISION}) does not \
             carry the delivered revision {REVISION}"
        )
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_reads(&requests);
}

/// R5: the head branch must be at the candidate revision before a creation is attempted.
#[tokio::test]
async fn r5_a_head_branch_not_at_the_candidate_revision_is_an_identity_error_with_no_post() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    fixture
        .queue(version())
        .queue(page(0, Vec::new()))
        .queue(repository())
        .queue(branch(OTHER_REVISION))
        .queue(repository())
        .queue(branch(OTHER_REVISION));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let error = review::open_or_update(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo pull request creation head is not the \
             published candidate for {REPOSITORY}: head branch {HEAD_BRANCH} is at \
             {OTHER_REVISION}, expected {REVISION}; base branch {TARGET_BRANCH}"
        )
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 6, "{requests:?}");
    assert_reads(&requests);
}

/// R6: an uncertain creation is reconciled by reading again, and a matching pull found by that
/// read is adopted rather than created a second time.
#[tokio::test]
async fn r6_a_creation_that_fails_in_transit_adopts_the_pull_request_the_read_finds() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    let mut found = pull(7);
    found.title = TITLE.to_owned();
    found.body = generated(DESCRIPTION);
    fixture
        .queue(version())
        .queue(page(0, Vec::new()))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(repository())
        .queue(branch(REVISION))
        // A header name the client rejects makes this response a transport failure.
        .queue(Response::status(StatusCode::OK).header("x invalid", "value"))
        .queue(page(1, vec![pull_value(&found)]));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let receipt = review::open_or_update(&authority, &request, credential())
        .await
        .assert_value();
    assert_eq!(receipt, review_receipt(7));

    // The read confirmed the attempt, so its record is cleared.
    assert!(
        authority.effects.lock().unwrap().is_empty(),
        "{authority:?}"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 8, "{requests:?}");
    assert_writes(&requests, &[Method::POST]);
    assert_eq!(requests.assert_at(7).uri, search_uri("open"));
}

/// R7: a conflict is reconciled only when the read finds the same head. A definite refusal stands
/// and releases the record, because a refused write cannot have been applied twice, so the next
/// attempt sends it again.
#[tokio::test]
async fn r7_a_conflict_without_a_matching_pull_request_returns_the_refusal() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    fixture
        .queue(version())
        .queue(page(0, Vec::new()))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(
            Response::status(StatusCode::CONFLICT)
                .with_body(r#"{"message":"these branches already have a pull request"}"#),
        )
        .queue(page(0, Vec::new()));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let error = review::open_or_update(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(409));
    assert!(format!("{error}").contains("HTTP 409"), "{error}");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 8, "{requests:?}");
    assert_writes(&requests, &[Method::POST]);
    assert_eq!(requests.assert_at(7).uri, search_uri("open"));

    // A definite refusal is not an uncertain write, so its record is released.
    assert!(
        authority.effects.lock().unwrap().is_empty(),
        "{authority:?}"
    );

    // With the record released, the next attempt sends the write again.
    let mut created = pull(12);
    created.title = TITLE.to_owned();
    created.body = generated(DESCRIPTION);
    fixture
        .queue(page(0, Vec::new()))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(pull_response(&created));
    let receipt = review::open_or_update(&authority, &request, credential())
        .await
        .assert_value();
    assert_eq!(receipt, review_receipt(12));

    let requests = fixture.requests();
    assert_eq!(requests.len(), 14, "{requests:?}");
    assert_writes(&requests, &[Method::POST, Method::POST]);
}

/// R8: two matching open pull requests are ambiguous, so the read names both numbers.
#[tokio::test]
async fn r8_two_matching_open_pull_requests_are_an_identity_error_naming_both_numbers() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    let mut first = pull(7);
    first.title = TITLE.to_owned();
    let mut second = pull(9);
    second.title = TITLE.to_owned();
    fixture
        .queue(version())
        .queue(page(2, vec![pull_value(&first), pull_value(&second)]));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let error = review::open_or_update(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Multiple Forgejo pull requests match head branch \
             {HEAD_BRANCH} in {REPOSITORY} targeting {TARGET_BRANCH}; review numbers: 7, 9. \
             Resolve the ambiguous pull requests before resuming."
        )
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_reads(&requests);
}

/// R9: a pull whose head belongs to another repository is a fork, so the search ignores it.
#[tokio::test]
async fn r9_a_fork_pull_request_with_the_same_branch_name_is_ignored_by_the_search() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    let mut fork = pull(7);
    fork.head_repo = "someone-else/name";
    fork.title = TITLE.to_owned();
    let mut created = pull(12);
    created.title = TITLE.to_owned();
    created.body = generated(DESCRIPTION);
    fixture
        .queue(version())
        .queue(page(1, vec![pull_value(&fork)]))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(pull_response(&created));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let receipt = review::open_or_update(&authority, &request, credential())
        .await
        .assert_value();
    assert_eq!(receipt, review_receipt(12));

    let requests = fixture.requests();
    assert_eq!(requests.len(), 7, "{requests:?}");
    assert_writes(&requests, &[Method::POST]);
}

/// R10: the observed states map as the observation table says.
#[tokio::test]
async fn r10_merged_closed_and_open_pull_requests_map_as_the_observation_table() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let known = review_receipt(7);

    let mut merged = pull(7);
    merged.state = "closed";
    merged.merged = true;
    merged.merge_sha = Some(MERGE_REVISION.to_owned());
    let mut merged_without_revision = pull(7);
    merged_without_revision.state = "closed";
    merged_without_revision.merged = true;
    let mut closed = pull(7);
    closed.state = "closed";
    let mut open = pull(7);
    open.mergeable = true;

    fixture
        .queue(version())
        .queue(pull_response(&merged))
        .queue(pull_response(&merged_without_revision))
        .queue(pull_response(&closed))
        .queue(pull_response(&open))
        .queue(branch_rule())
        .queue(page(0, Vec::new()));

    assert_eq!(
        review::inspect(&authority, &known, credential())
            .await
            .assert_value(),
        observation(
            &known,
            SourceReviewState::Merged {
                merge_revision: MERGE_REVISION.to_owned(),
            }
        )
    );

    let error = review::inspect(&authority, &known, credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), None);
    assert!(error.retryable_operation(), "{error}");
    assert_eq!(
        format!("{error}"),
        "source API request failed: Forgejo pull request 7 is merged without a merge commit revision"
    );

    assert_eq!(
        review::inspect(&authority, &known, credential())
            .await
            .assert_value(),
        observation(&known, SourceReviewState::Closed)
    );
    assert_eq!(
        review::inspect(&authority, &known, credential())
            .await
            .assert_value(),
        observation(
            &known,
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired,
            }
        )
    );
    // A mergeable open pull request is observed through the rule in effect for its target branch
    // and the statuses of its head. Nothing is enforced and nothing has reported, so no check is
    // required, and readiness needs an approval the rule does not ask for.
    let parsed = serde_json::from_value::<wire::PullRequest>(pull_value(&open))
        .expect("the pull response reads as the wire shape");
    assert!(
        parsed.mergeable,
        "the response claims the pull request is mergeable"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 7, "{requests:?}");
    assert_reads(&requests);
    for request in &requests[1..5] {
        assert_eq!(request.uri, format!("{SEARCH_PATH}/7"), "{request:?}");
    }
    assert_eq!(requests.assert_at(5).uri, rule_uri(), "{requests:?}");
    assert_eq!(requests.assert_at(6).uri, statuses_uri(), "{requests:?}");
}

/// R11: a read that does not ask for the pull request reads none and still reports the branch.
#[tokio::test]
async fn r11_a_read_without_a_review_requests_no_pull_and_still_reports_the_branch_revision() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();
    let read = SourceDeliveryRead {
        include_terminal_reviews: false,
        target: &target,
        head_branch: HEAD_BRANCH,
        known_review: None,
        include_review: false,
    };

    fixture
        .queue(version())
        .queue(repository())
        .queue(branch(REVISION));

    let snapshot = review::observe(&authority, read, credential())
        .await
        .assert_value();
    assert_eq!(snapshot.review, None);
    assert_eq!(snapshot.head_revision.as_deref(), Some(REVISION));

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_reads(&requests);
    assert_eq!(
        requests.assert_at(2).uri,
        format!("/api/v1/repos/owner/name/branches/{HEAD_BRANCH}")
    );
}

/// R12: an open pull that disagrees with its branch is retried before it is classified.
#[tokio::test]
async fn r12_an_open_pull_that_disagrees_with_its_branch_is_retried_then_is_an_identity_error() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    let mut disagreeing = pull(7);
    disagreeing.title = TITLE.to_owned();
    fixture.queue(version());
    for _ in 0..3 {
        fixture
            .queue(page(1, vec![pull_value(&disagreeing)]))
            .queue(repository())
            .queue(branch(OTHER_REVISION));
    }

    // Paused time keeps the recheck delay from making this test wait.
    tokio::time::pause();
    let error = review::observe(&authority, review_read(&target, None), credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo pull request 7 in {REPOSITORY} on branch \
             {HEAD_BRANCH} still disagrees with its ref after rechecking: pull head {REVISION}; \
             branch head {OTHER_REVISION}"
        )
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 10, "{requests:?}");
    assert_reads(&requests);
    for round in 0..3 {
        assert_eq!(
            requests.assert_at(1 + round * 3).uri,
            search_uri("open"),
            "{requests:?}"
        );
    }
}

/// R13: `inspect` echoes the receipt identity and refuses a pull on another head branch.
#[tokio::test]
async fn r13_inspect_echoes_the_receipt_identity_and_refuses_another_head_branch() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let known = review_receipt(7);

    let mut mergeable = pull(7);
    mergeable.mergeable = true;
    let mut other_branch = pull(7);
    other_branch.head_ref = "someone-else/branch";
    fixture
        .queue(version())
        .queue(pull_response(&mergeable))
        .queue(branch_rule())
        .queue(page(0, Vec::new()))
        .queue(pull_response(&other_branch));

    let observed = review::inspect(&authority, &known, credential())
        .await
        .assert_value();
    assert_eq!(
        observed,
        observation(
            &known,
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired,
            }
        )
    );
    assert_eq!(observed.review_id, known.review_id);
    assert_eq!(observed.repository, known.repository);
    assert_eq!(observed.target_branch, known.target_branch);
    assert_eq!(observed.head_branch, known.head_branch);
    assert_eq!(observed.head_revision, known.head_revision);

    let error = review::inspect(&authority, &known, credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo pull request 7 is on branch \
             someone-else/branch instead of {HEAD_BRANCH}"
        )
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 5, "{requests:?}");
    assert_reads(&requests);
    for request in [&requests[1], &requests[4]] {
        assert_eq!(request.uri, format!("{SEARCH_PATH}/7"), "{request:?}");
    }
    assert_eq!(requests.assert_at(2).uri, rule_uri(), "{requests:?}");
    assert_eq!(requests.assert_at(3).uri, statuses_uri(), "{requests:?}");
}

/// R14: title normalization and the links built from the admitted instance.
#[tokio::test]
async fn r14_normalized_title_and_links_keep_the_admitted_instance() {
    assert_eq!(review::normalized_title("WIP: [wip] Fix the read"), TITLE);
    assert_eq!(review::normalized_title("wip:"), "Zeroshot change");
    assert_eq!(review::normalized_title(""), "Zeroshot change");

    let longest = "x".repeat(300);
    assert_eq!(review::normalized_title(&longest).chars().count(), 255);
    // The bound is applied before the trailing trim, so the characters past 255 are the ones lost.
    assert_eq!(
        review::normalized_title(&format!("{longest}.")),
        "x".repeat(255)
    );
    assert_eq!(
        review::normalized_title(&format!("{}   ", "y".repeat(252))),
        "y".repeat(252)
    );

    let known = review_receipt(7);
    let links = review::links(BASE, &known);
    assert_eq!(links.url, format!("{BASE}/{REPOSITORY}/pulls/7"));
    assert_eq!(
        links.commit_url,
        format!("{BASE}/{REPOSITORY}/commit/{REVISION}")
    );

    let prefixed = review::links("https://forge.example/git/", &known);
    assert_eq!(
        prefixed.url,
        format!("https://forge.example/git/{REPOSITORY}/pulls/7")
    );
    assert_eq!(
        prefixed.commit_url,
        format!("https://forge.example/git/{REPOSITORY}/commit/{REVISION}")
    );
}

/// R15: a merged pull request reports its head as the pull ref, so the head branch is read from
/// `head.label` and the observation still reports the merge revision.
#[tokio::test]
async fn r15_inspect_reads_a_merged_pull_whose_head_ref_is_the_pull_ref() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let known = review_receipt(7);

    let mut merged = pull(7);
    merged.state = "closed";
    merged.merged = true;
    merged.merge_sha = Some(MERGE_REVISION.to_owned());
    merged.head_pull_ref = "refs/pull/7/head";

    fixture.queue(version()).queue(pull_response(&merged));

    assert_eq!(
        review::inspect(&authority, &known, credential())
            .await
            .assert_value(),
        observation(
            &known,
            SourceReviewState::Merged {
                merge_revision: MERGE_REVISION.to_owned(),
            }
        )
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_reads(&requests);
    assert_eq!(requests.assert_at(1).uri, format!("{SEARCH_PATH}/7"));
}

/// R16: the search matches that merged pull request through `head.label`, so a read that allows
/// terminal reviews finds it.
#[tokio::test]
async fn r16_the_search_finds_a_merged_pull_through_its_head_label() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();
    let read = SourceDeliveryRead {
        include_terminal_reviews: true,
        target: &target,
        head_branch: HEAD_BRANCH,
        known_review: None,
        include_review: true,
    };

    let mut merged = pull(7);
    merged.state = "closed";
    merged.merged = true;
    merged.merge_sha = Some(MERGE_REVISION.to_owned());
    merged.head_pull_ref = "refs/pull/7/head";

    fixture
        .queue(version())
        .queue(page(1, vec![pull_value(&merged)]))
        .queue(repository())
        .queue(branch(REVISION));

    let snapshot = review::observe(&authority, read, credential())
        .await
        .assert_value();
    assert_eq!(
        snapshot.review,
        Some(observation(
            &review_receipt(7),
            SourceReviewState::Merged {
                merge_revision: MERGE_REVISION.to_owned(),
            }
        ))
    );
    assert_eq!(snapshot.head_revision.as_deref(), Some(REVISION));

    let requests = fixture.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_reads(&requests);
    assert_eq!(requests.assert_at(1).uri, search_uri("all"));
}

/// R17: a pull whose `head.ref` is the head branch but whose `head.label` names another branch is
/// not this delivery's, so the search ignores it.
#[tokio::test]
async fn r17_a_pull_with_the_matching_ref_and_another_label_is_ignored_by_the_search() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();
    let read = SourceDeliveryRead {
        include_terminal_reviews: false,
        target: &target,
        head_branch: HEAD_BRANCH,
        known_review: None,
        include_review: true,
    };

    let mut other = pull(7);
    other.head_ref = "someone-else/branch";

    fixture
        .queue(version())
        .queue(page(1, vec![pull_value(&other)]))
        .queue(repository())
        .queue(branch(REVISION));

    let snapshot = review::observe(&authority, read, credential())
        .await
        .assert_value();
    assert_eq!(snapshot.review, None);
    assert_eq!(snapshot.head_revision.as_deref(), Some(REVISION));

    let requests = fixture.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_reads(&requests);
    assert_eq!(requests.assert_at(1).uri, search_uri("open"));
}

/// R18: a definite refusal with no pull request found releases the creation record and keeps its
/// status, because a refused write cannot have been applied twice.
#[tokio::test]
async fn r18_a_definite_refusal_with_no_pull_found_releases_the_creation_record() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    fixture
        .queue(version())
        .queue(page(0, Vec::new()))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(
            Response::status(StatusCode::UNPROCESSABLE_ENTITY)
                .with_body(r#"{"message":"title cannot be empty"}"#),
        )
        .queue(page(0, Vec::new()));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let error = review::open_or_update(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(422));
    assert!(format!("{error}").contains("HTTP 422"), "{error}");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 8, "{requests:?}");
    assert_writes(&requests, &[Method::POST]);
    assert_eq!(requests.assert_at(7).uri, search_uri("open"));

    assert!(
        authority.effects.lock().unwrap().is_empty(),
        "{authority:?}"
    );
}

/// R19: a server error stays uncertain, so the record remains and the write is never sent a second
/// time, and the reconciliation names the response it could not confirm.
#[tokio::test]
async fn r19_an_uncertain_response_keeps_the_creation_record_and_names_the_lost_response() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let target = target();

    fixture
        .queue(version())
        .queue(page(0, Vec::new()))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(Response::status(StatusCode::INTERNAL_SERVER_ERROR))
        .queue(page(0, Vec::new()));

    let request = review_request(&target, TITLE, DESCRIPTION);
    let error = review::open_or_update(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), None);
    assert!(format!("{error}").contains("response was lost"), "{error}");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 8, "{requests:?}");
    assert_writes(&requests, &[Method::POST]);

    // The attempt is still unconfirmed, so the record stays and no second write is sent.
    assert_eq!(authority.effects.lock().unwrap().len(), 1, "{authority:?}");

    fixture
        .queue(page(0, Vec::new()))
        .queue(repository())
        .queue(branch(REVISION))
        .queue(repository())
        .queue(branch(REVISION));
    let refused = review::open_or_update(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{refused}"),
        "source API request failed: Forgejo review creation remains uncertain"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 13, "{requests:?}");
    assert_writes(&requests, &[Method::POST]);
}

/// R20: a pull request read whose response has no number is not the wire shape, so it is the API
/// error rather than a pull request numbered zero.
#[tokio::test]
async fn r20_a_pull_request_read_without_its_number_is_the_invalid_json_api_error() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let known = review_receipt(7);

    let mut without_number = pull_value(&pull(7));
    without_number
        .as_object_mut()
        .expect("a pull response is an object")
        .remove("number");
    fixture
        .queue(version())
        .queue(Response::json(without_number));

    let error = review::inspect(&authority, &known, credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), None);
    assert_eq!(
        format!("{error}"),
        "source API request failed: Forgejo returned invalid JSON"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_reads(&requests);
    assert_eq!(requests.assert_at(1).uri, format!("{SEARCH_PATH}/7"));
}

/// R21: `inspect` refuses an open pull request whose head moved away from the head the receipt
/// bound, before any check or CI read.
#[tokio::test]
async fn r21_inspect_refuses_an_open_pull_whose_head_moved_from_the_bound_review_head() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let known = review_receipt(7);

    let mut moved = pull(7);
    moved.head_sha = OTHER_REVISION.to_owned();
    fixture.queue(version()).queue(pull_response(&moved));

    let error = review::inspect(&authority, &known, credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo pull request 7 changed the bound review \
             head: head revision {OTHER_REVISION} instead of {REVISION}. Restore the bound review \
             identity before resuming."
        )
    );

    // Nothing beyond the pull request is read for a refused head: no status page and no run.
    let requests = fixture.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_reads(&requests);
    assert_eq!(requests.assert_at(1).uri, format!("{SEARCH_PATH}/7"));
}
