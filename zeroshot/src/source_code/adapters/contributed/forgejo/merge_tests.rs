//! Merge, withdrawal and conflict acceptance for the contributed Forgejo adapter. Every read and
//! write goes through the loopback fixture, so the assertions cover the write itself, the record the
//! adapter keeps about it, and what a later call does with that record. Each test names the
//! acceptance criterion it verifies, from `M1` to `M16`.

use std::path::PathBuf;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use openengine_cluster_protocol::SourceForgeBaseUrl;
use openengine_cluster_testkit::assertions::{AssertAt, AssertError, AssertSlice, AssertValue, JsonAt};

use crate::source_code_provider::{
    SourceConflictRequest, SourceDeliveryCredential, SourceMergeRequestOutcome,
    SourceMergeWithdrawal, SourceReviewReceipt,
};

use super::api::{Api, PAGE_SIZE};
use super::authority::ForgejoDeliveryAuthority;
use super::http_fixture::{Fixture, Request, Response};
use super::merge;
use super::wire;

const TOKEN: &str = "forgejo-test-token";
const BASE: &str = "https://forge.example";
const REPOSITORY: &str = "owner/name";
const REVIEW_ID: &str = "7";
const HEAD_BRANCH: &str = "zeroshot/test";
const TARGET_BRANCH: &str = "main";
const REVISION: &str = "0123456789012345678901234567890123456789";
const MOVED_REVISION: &str = "1111111111111111111111111111111111111111";
const MERGE_REVISION: &str = "2222222222222222222222222222222222222222";
const BASE_REVISION: &str = "3333333333333333333333333333333333333333";
const PULL_URI: &str = "/api/v1/repos/owner/name/pulls/7";
const MERGE_URI: &str = "/api/v1/repos/owner/name/pulls/7/merge";
const PERMISSION_TEXT: &str = "Forgejo merge permission is missing for owner/name pull request 7 (head branch zeroshot/test, base branch main, head revision 0123456789012345678901234567890123456789). Grant the token permission to merge into target branch main, then resume.";

fn credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token(TOKEN)
}

async fn new_authority(fixture: &Fixture) -> ForgejoDeliveryAuthority {
    ForgejoDeliveryAuthority::for_test(
        SourceForgeBaseUrl::new(BASE).assert_value(),
        Api::loopback(&fixture.endpoint()).assert_value(),
    )
}

fn receipt() -> SourceReviewReceipt {
    SourceReviewReceipt {
        review_id: REVIEW_ID.to_owned(),
        repository: REPOSITORY.to_owned(),
        target_branch: TARGET_BRANCH.to_owned(),
        head_branch: HEAD_BRANCH.to_owned(),
        head_revision: REVISION.to_owned(),
    }
}

/// The version read is made once per authority, before anything else.
fn version() -> Response {
    Response::json(json!({"version": "16.0.0"}))
}

fn repository_body(default: &str, allowed: &[&str]) -> Value {
    let allowed = |style: &str| allowed.contains(&style);
    json!({
        "full_name": REPOSITORY,
        "html_url": format!("{BASE}/{REPOSITORY}"),
        "archived": false,
        "unknown": "the wire shape ignores unknown fields",
        "default_merge_style": default,
        "allow_merge_commits": allowed("merge"),
        "allow_rebase": allowed("rebase"),
        "allow_rebase_explicit": allowed("rebase-merge"),
        "allow_squash_merge": allowed("squash"),
        "allow_fast_forward_only_merge": allowed("fast-forward-only"),
    })
}

fn repository(default: &str, allowed: &[&str]) -> Response {
    Response::json(repository_body(default, allowed))
}

fn repository_shape(default: &str, allowed: &[&str]) -> wire::Repository {
    serde_json::from_value(repository_body(default, allowed))
        .expect("the repository response reads as the wire shape")
}

struct Pull<'a> {
    state: &'a str,
    merged: bool,
    mergeable: bool,
    draft: bool,
    head: &'a str,
    merge_sha: Option<&'a str>,
}

fn open_pull(head: &str) -> Pull<'_> {
    Pull {
        state: "open",
        merged: false,
        mergeable: true,
        draft: false,
        head,
        merge_sha: None,
    }
}

fn merged_pull(head: &str) -> Pull<'_> {
    Pull {
        merged: true,
        merge_sha: Some(MERGE_REVISION),
        ..open_pull(head)
    }
}

impl Pull<'_> {
    fn response(&self) -> Response {
        Response::json(json!({
            "number": 7,
            "state": self.state,
            "draft": self.draft,
            "merged": self.merged,
            "mergeable": self.mergeable,
            "merge_commit_sha": self.merge_sha,
            "head": {
                "ref": HEAD_BRANCH,
                "label": HEAD_BRANCH,
                "sha": self.head,
                "repo": { "full_name": REPOSITORY },
            },
            "base": {
                "ref": TARGET_BRANCH,
                "sha": BASE_REVISION,
                "repo": { "full_name": REPOSITORY },
            },
        }))
    }
}

/// The rule in effect for the target branch.
fn branch(required_approvals: u64, contexts: &[&str], enforced: bool) -> Response {
    Response::json(json!({
        "name": TARGET_BRANCH,
        "commit": { "id": BASE_REVISION },
        "enable_status_check": enforced,
        "status_check_contexts": contexts,
        "required_approvals": required_approvals,
    }))
}

fn status(id: u64, context: &str, result: &str, description: &str) -> Value {
    json!({
        "id": id,
        "context": context,
        "status": result,
        "description": description,
    })
}

/// A page response carries the total the pagination contract reads.
fn page(entries: &[Value]) -> Response {
    Response::json(json!(entries)).header("x-total-count", &entries.len().to_string())
}

/// The requests a merge request makes before its write, in the order it makes them: the repository
/// for the style, the pull request, the rule in effect for the target branch, and the head statuses.
fn gate_reads<'a>(
    fixture: &'a Fixture,
    pull: &Pull,
    rule: Response,
    statuses: Response,
) -> &'a Fixture {
    fixture
        .queue(repository("merge", &["merge"]))
        .queue(pull.response())
        .queue(rule)
        .queue(statuses)
}

/// The requests a merge request makes when the gate answers before the rule is read: the repository
/// for the style and the pull request.
fn gate_reads_without_rule<'a>(fixture: &'a Fixture, pull: &Pull) -> &'a Fixture {
    fixture
        .queue(repository("merge", &["merge"]))
        .queue(pull.response())
}

/// A response the client cannot read, which is what a transport failure looks like here. The
/// request itself is still recorded, so the write stays observable.
fn transport_failure() -> Response {
    Response::status(StatusCode::OK).header("bad header", "x")
}

fn recorded(authority: &ForgejoDeliveryAuthority) -> bool {
    authority
        .effects
        .lock()
        .unwrap()
        .contains(&merge::merge_key(REPOSITORY, REVIEW_ID))
}

fn remember(authority: &ForgejoDeliveryAuthority) {
    authority
        .effects
        .lock()
        .unwrap()
        .insert(merge::merge_key(REPOSITORY, REVIEW_ID));
}

fn request_body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).expect("a write sends JSON")
}

fn writes(requests: &[Request]) -> Vec<&Request> {
    requests
        .iter()
        .filter(|request| request.method != Method::GET)
        .collect()
}

fn repository_uri() -> String {
    format!("/api/v1/repos/{REPOSITORY}")
}

fn branch_uri() -> String {
    format!("/api/v1/repos/{REPOSITORY}/branches/{TARGET_BRANCH}")
}

fn statuses_uri() -> String {
    format!("/api/v1/repos/{REPOSITORY}/commits/{REVISION}/statuses?limit={PAGE_SIZE}&page=1")
}

/// M1: checks passed sends exactly one write carrying the style, the reviewed head and no branch
/// deletion, then a read that shows the merge; `Accepted`, and the record is gone.
#[tokio::test]
async fn m1_checks_passed_sends_one_write_and_releases_the_record() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads(
        &fixture,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::OK))
    .queue(merged_pull(REVISION).response());

    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Accepted
    );
    assert!(!recorded(&authority), "the write was confirmed");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 7, "{requests:?}");
    assert_eq!(requests.assert_at(1).uri, repository_uri());
    assert_eq!(requests.assert_at(2).uri, PULL_URI);
    assert_eq!(requests.assert_at(3).uri, branch_uri());
    assert_eq!(requests.assert_at(4).uri, statuses_uri());
    let write = requests.assert_at(5);
    assert_eq!(write.method, Method::POST);
    assert_eq!(write.uri, MERGE_URI);
    let body = request_body(write);
    assert_eq!(body.assert_key("Do"), &json!("merge"));
    assert_eq!(body.assert_key("head_commit_id"), &json!(REVISION));
    assert_eq!(body.assert_key("delete_branch_after_merge"), &json!(false));
    assert_eq!(
        requests.assert_at(6).uri,
        PULL_URI,
        "the read that shows the merge"
    );
}

/// M2: pending checks, a draft, and missing approvals each return `Pending` without any write.
#[tokio::test]
async fn m2_pending_checks_a_draft_and_missing_approvals_send_no_write() {
    let pending = Fixture::new().await;
    let authority = new_authority(&pending).await;
    pending.queue(version());
    gate_reads(
        &pending,
        &open_pull(REVISION),
        branch(0, &["ci"], true),
        page(&[status(1, "ci", "pending", "")]),
    );
    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    let requests = pending.requests();
    assert_eq!(requests.len(), 5, "{requests:?}");
    assert!(writes(&requests).is_empty(), "{requests:?}");

    // A draft is never ready, so nothing beyond the pull request is read for it.
    let draft = Fixture::new().await;
    let authority = new_authority(&draft).await;
    draft.queue(version());
    gate_reads_without_rule(
        &draft,
        &Pull {
            draft: true,
            ..open_pull(REVISION)
        },
    );
    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    let requests = draft.requests();
    assert_eq!(
        requests.len(),
        3,
        "the rule and the statuses are not read for a draft"
    );
    assert!(writes(&requests).is_empty(), "{requests:?}");

    // The readiness read is the review list, and it is made because the rule wants approvals.
    let approvals = Fixture::new().await;
    let authority = new_authority(&approvals).await;
    approvals.queue(version());
    gate_reads(
        &approvals,
        &open_pull(REVISION),
        branch(1, &[], false),
        page(&[]),
    )
    .queue(page(&[]));
    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    let requests = approvals.requests();
    assert_eq!(
        requests.len(),
        6,
        "the approvals are read because the rule wants them"
    );
    assert_eq!(
        requests.assert_at(5).uri,
        format!("/api/v1/repos/{REPOSITORY}/pulls/7/reviews?limit={PAGE_SIZE}&page=1")
    );
    assert!(writes(&requests).is_empty(), "{requests:?}");
}

/// M3: `mergeable` false through every recheck is `Conflict` without any write.
#[tokio::test(start_paused = true)]
async fn m3_ambiguous_mergeability_through_every_recheck_is_conflict_without_a_write() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    fixture.queue(repository("merge", &["merge"]));
    for _ in 0..4 {
        fixture.queue(
            Pull {
                mergeable: false,
                ..open_pull(REVISION)
            }
            .response(),
        );
    }

    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Conflict
    );
    assert!(
        !recorded(&authority),
        "a gate outcome is never recorded as an attempt"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 6, "{requests:?}");
    assert_eq!(requests.assert_slice(2..6).len(), 4, "{requests:?}");
    for request in requests.assert_slice(2..6) {
        assert_eq!(
            request.uri, PULL_URI,
            "every recheck reads the pull request"
        );
    }
    assert!(writes(&requests).is_empty(), "{requests:?}");
}

/// M4: already merged is `Accepted` without any write, and the read releases any record.
#[tokio::test]
async fn m4_an_already_merged_pull_request_is_accepted_without_a_write() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads_without_rule(&fixture, &merged_pull(REVISION));
    remember(&authority);

    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Accepted
    );
    assert!(!recorded(&authority), "the merged read releases the record");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(requests.assert_at(2).uri, PULL_URI);
    assert!(writes(&requests).is_empty(), "{requests:?}");
}

/// M5: a head that differs from the receipt is the bound-head identity error without any write,
/// and the record of an earlier attempt is not released by it.
#[tokio::test]
async fn m5_a_head_that_differs_from_the_receipt_is_the_identity_error_without_a_write() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads_without_rule(&fixture, &open_pull(MOVED_REVISION));
    remember(&authority);

    let error = merge::request(&authority, &receipt(), credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo pull request 7 changed the bound review \
             head: head revision {MOVED_REVISION} instead of {REVISION}. Restore the bound review \
             identity before resuming."
        )
    );
    assert!(
        recorded(&authority),
        "an unconfirmed attempt stays recorded for the read that can decide it"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(requests.assert_at(2).uri, PULL_URI);
    assert!(writes(&requests).is_empty(), "{requests:?}");
}

/// M6: 405 returns `Pending` and removes the record, so a second call sends a second write.
#[tokio::test]
async fn m6_http_405_returns_pending_and_lets_the_next_call_write_again() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads(
        &fixture,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::METHOD_NOT_ALLOWED));

    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    assert!(
        !recorded(&authority),
        "a refusal that is not a lost response releases the record"
    );

    gate_reads(
        &fixture,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::OK))
    .queue(merged_pull(REVISION).response());
    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Accepted
    );
    assert!(!recorded(&authority), "the second write was confirmed");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 12, "{requests:?}");
    assert_eq!(writes(&requests).len(), 2, "{requests:?}");
    assert_eq!(
        requests.assert_at(5).uri,
        MERGE_URI,
        "the first call wrote once"
    );
    assert_eq!(
        requests.assert_at(10).uri,
        MERGE_URI,
        "the second call wrote again"
    );
    for write in writes(&requests) {
        assert_eq!(write.method, Method::POST);
        assert_eq!(write.uri, MERGE_URI, "{requests:?}");
    }
}

/// M7: 409 with an unchanged head is `Conflict`; with a moved head it is `Pending`.
#[tokio::test]
async fn m7_http_409_is_conflict_for_an_unchanged_head_and_pending_for_a_moved_one() {
    let unchanged = Fixture::new().await;
    let authority = new_authority(&unchanged).await;
    unchanged.queue(version());
    gate_reads(
        &unchanged,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::CONFLICT))
    .queue(open_pull(REVISION).response());
    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Conflict
    );
    assert!(
        !recorded(&authority),
        "a conflict is not an unconfirmed attempt"
    );

    let moved = Fixture::new().await;
    let authority = new_authority(&moved).await;
    moved.queue(version());
    gate_reads(
        &moved,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::CONFLICT))
    .queue(open_pull(MOVED_REVISION).response());
    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    assert!(
        !recorded(&authority),
        "a moved head is not an unconfirmed attempt"
    );

    let unchanged_requests = unchanged.requests();
    let moved_requests = moved.requests();
    assert_eq!(
        unchanged_requests.len(),
        7,
        "one write and the read that settles it"
    );
    assert_eq!(moved_requests.len(), 7, "{moved_requests:?}");
    assert_eq!(
        writes(&unchanged_requests).len(),
        1,
        "{unchanged_requests:?}"
    );
    assert_eq!(writes(&moved_requests).len(), 1, "{moved_requests:?}");
}

/// M8: 403 is the permission error with the documented text, and the token is never in it.
#[tokio::test]
async fn m8_http_403_is_the_permission_error_with_the_documented_text_and_no_token() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads(
        &fixture,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::FORBIDDEN));

    let error = merge::request(&authority, &receipt(), credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(403));
    assert!(!error.authentication_failed(), "{error}");
    assert_eq!(
        format!("{error}"),
        format!("source API request failed: {PERMISSION_TEXT}")
    );
    for part in [
        REPOSITORY,
        HEAD_BRANCH,
        TARGET_BRANCH,
        REVISION,
        REVIEW_ID,
        "Grant the token permission",
    ] {
        assert!(format!("{error}").contains(part), "{error}");
    }
    assert!(!format!("{error}").contains(TOKEN), "{error}");
    assert!(
        !recorded(&authority),
        "a permission refusal releases the record"
    );

    let requests = fixture.requests();
    assert_eq!(
        requests.len(),
        6,
        "no settle read after a permission refusal"
    );
    assert_eq!(requests.assert_at(5).uri, MERGE_URI);
    assert_eq!(writes(&requests).len(), 1, "{requests:?}");
}

/// M9: 401 with a valid user read is the same permission error; 401 where the user read also fails
/// keeps status 401.
#[tokio::test]
async fn m9_http_401_with_a_valid_user_is_the_permission_error_and_otherwise_keeps_401() {
    let known = Fixture::new().await;
    let authority = new_authority(&known).await;
    known.queue(version());
    gate_reads(
        &known,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::UNAUTHORIZED))
    .queue(Response::json(json!({ "login": "zeroshot" })));

    let error = merge::request(&authority, &receipt(), credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(401));
    assert!(
        !error.authentication_failed(),
        "the read proved it is permission"
    );
    assert_eq!(
        format!("{error}"),
        format!("source API request failed: {PERMISSION_TEXT}")
    );
    assert!(
        !recorded(&authority),
        "the record is released before the permission error"
    );
    let requests = known.requests();
    assert_eq!(
        requests.assert_at(6).uri,
        "/api/v1/user",
        "the read that decides"
    );
    assert_eq!(requests.len(), 7, "{requests:?}");

    let refused = Fixture::new().await;
    let authority = new_authority(&refused).await;
    refused.queue(version());
    gate_reads(
        &refused,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::UNAUTHORIZED))
    .queue(Response::status(StatusCode::UNAUTHORIZED));

    let error = merge::request(&authority, &receipt(), credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(401));
    assert!(
        error.authentication_failed(),
        "no user read, so it stays 401"
    );
    let requests = refused.requests();
    assert_eq!(requests.len(), 7, "{requests:?}");
}

/// M10: a transport failure followed by a read that shows the merge is `Accepted`.
#[tokio::test]
async fn m10_a_transport_failure_followed_by_a_merged_read_is_accepted() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads(
        &fixture,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(transport_failure())
    .queue(merged_pull(REVISION).response());

    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Accepted
    );
    assert!(
        !recorded(&authority),
        "the read confirmed the attempt, so the record is released"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 7, "{requests:?}");
    assert_eq!(requests.assert_at(5).uri, MERGE_URI);
    assert_eq!(
        requests.assert_at(6).uri,
        PULL_URI,
        "the read that shows the merge"
    );
    assert_eq!(writes(&requests).len(), 1, "{requests:?}");
}

/// M11: an unconfirmed write keeps its record; the next call removes it and returns `Pending`
/// without writing, and the call after that writes again.
#[tokio::test]
async fn m11_an_unconfirmed_write_keeps_its_record_until_the_next_calls_resolve_it() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads(
        &fixture,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(transport_failure())
    .queue(open_pull(REVISION).response());

    let error = merge::request(&authority, &receipt(), credential())
        .await
        .assert_error();
    assert!(error.retryable_operation(), "{error}");
    assert!(format!("{error}").contains("transport failed"), "{error}");
    assert!(!format!("{error}").contains(TOKEN), "{error}");
    assert!(recorded(&authority), "the outcome is unknown");

    // A read that finds the record reports it as `Pending` and releases it, without writing.
    gate_reads_without_rule(&fixture, &open_pull(REVISION));
    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    assert!(
        !recorded(&authority),
        "the record is released before the next attempt"
    );

    gate_reads(
        &fixture,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::OK))
    .queue(merged_pull(REVISION).response());
    assert_eq!(
        merge::request(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Accepted
    );
    assert!(!recorded(&authority), "the third write was confirmed");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 15, "{requests:?}");
    assert_eq!(writes(&requests).len(), 2, "{requests:?}");
    assert_eq!(
        requests.assert_at(5).uri,
        MERGE_URI,
        "the first call wrote once"
    );
    assert_eq!(
        requests.assert_at(6).uri,
        PULL_URI,
        "the read that could not decide"
    );
    assert_eq!(
        requests.assert_at(8).uri,
        PULL_URI,
        "the read that found the record"
    );
    assert_eq!(
        requests.assert_at(13).uri,
        MERGE_URI,
        "the call after that wrote again"
    );
    assert_eq!(
        requests.assert_at(14).uri,
        PULL_URI,
        "the read that confirmed it"
    );
}

/// M12: `merge_style` takes the default when it is allowed, otherwise the allowed order, and a
/// repository that allows none is an identity error naming the repository.
#[tokio::test]
async fn m12_merge_style_prefers_the_default_then_the_allowed_order() {
    assert_eq!(
        merge::merge_style(&repository_shape("squash", &["squash"])),
        Ok("squash")
    );
    assert_eq!(
        merge::merge_style(&repository_shape(
            "squash",
            &["merge", "rebase", "rebase-merge"]
        )),
        Ok("merge")
    );
    assert_eq!(
        merge::merge_style(&repository_shape(
            "fast-forward-only",
            &["rebase-merge", "squash"]
        )),
        Ok("rebase-merge")
    );
    assert_eq!(
        merge::merge_style(&repository_shape("rebase", &["fast-forward-only"])),
        Ok("fast-forward-only")
    );
    assert_eq!(
        merge::merge_style(&repository_shape("squash", &["rebase"])),
        Ok("rebase")
    );

    let error = merge::merge_style(&repository_shape("squash", &[])).assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo repository {} allows no merge style, so its default merge style {} cannot be used",
            wire::repository_identity(REPOSITORY),
            wire::observed("squash")
        )
    );
}

/// M13: `withdraw` of a merged pull request returns `Merged` with its revision; of an open one it
/// returns `Withdrawn` and removes the record.
#[tokio::test]
async fn m13_withdraw_reports_a_merged_pull_request_and_releases_an_open_one() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture
        .queue(version())
        .queue(merged_pull(REVISION).response());
    assert_eq!(
        merge::withdraw(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeWithdrawal::Merged {
            merge_revision: MERGE_REVISION.to_owned(),
        }
    );
    assert_eq!(
        fixture.requests().len(),
        2,
        "the version and the pull request read"
    );

    fixture.queue(open_pull(REVISION).response());
    assert_eq!(
        merge::withdraw(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeWithdrawal::Withdrawn
    );
    assert!(!recorded(&authority), "nothing was recorded to release");

    // An attempt that is still held is released by the withdrawal, so it cannot merge later.
    remember(&authority);
    fixture.queue(open_pull(REVISION).response());
    assert_eq!(
        merge::withdraw(&authority, &receipt(), credential())
            .await
            .assert_value(),
        SourceMergeWithdrawal::Withdrawn
    );
    assert!(
        !recorded(&authority),
        "an unconfirmed attempt cannot merge later"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert!(writes(&requests).is_empty(), "{requests:?}");
    for request in requests.assert_slice(1..4) {
        assert_eq!(request.uri, PULL_URI, "{requests:?}");
    }
}

/// M14: conflict materialization reports the missing transport, and an absent target branch is an
/// identity error naming the branch and the repository.
#[tokio::test]
async fn m14_conflict_materialization_needs_the_transport_and_a_present_target_branch() {
    let request = SourceConflictRequest {
        workspace: PathBuf::from(".zeroshot/work"),
        review: receipt(),
    };

    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture
        .queue(version())
        .queue(open_pull(REVISION).response())
        .queue(repository("merge", &["merge"]))
        .queue(branch(0, &[], false));

    let error = merge::materialize_conflict(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        "source delivery identity mismatch: Forgejo Git delivery requires the admitted Git transport"
    );

    let missing = Fixture::new().await;
    let authority = new_authority(&missing).await;
    missing
        .queue(version())
        .queue(open_pull(REVISION).response())
        .queue(repository("merge", &["merge"]))
        .queue(Response::status(StatusCode::NOT_FOUND));

    let error = merge::materialize_conflict(&authority, &request, credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo target branch is absent: branch {} in repository {}. Restore the target branch before resuming.",
            wire::branch_identity(TARGET_BRANCH),
            wire::repository_identity(REPOSITORY)
        )
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_eq!(requests.assert_at(1).uri, PULL_URI);
    assert_eq!(requests.assert_at(2).uri, repository_uri());
    assert_eq!(requests.assert_at(3).uri, branch_uri());
    assert!(
        writes(&requests).is_empty(),
        "the target revision is read, never written"
    );
    let missing_requests = missing.requests();
    assert_eq!(missing_requests.len(), 4, "{missing_requests:?}");
}

/// M15: no write body ever carries `force_merge` or `merge_when_checks_succeed`, and nothing in it
/// asks for the head branch to be removed.
#[tokio::test]
async fn m15_no_write_body_carries_force_merge_or_merge_when_checks_succeed() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads(
        &fixture,
        &open_pull(REVISION),
        branch(0, &[], false),
        page(&[]),
    )
    .queue(Response::status(StatusCode::OK))
    .queue(merged_pull(REVISION).response());
    merge::request(&authority, &receipt(), credential())
        .await
        .assert_value();

    let requests = fixture.requests();
    let write_requests = writes(&requests);
    let write = write_requests.assert_at(0);
    let body = request_body(write);
    let mut keys = body
        .as_object()
        .expect("the write body is an object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<&str>>();
    keys.sort();
    assert_eq!(
        keys,
        ["Do", "delete_branch_after_merge", "head_commit_id"],
        "{body}"
    );
    assert_eq!(body.assert_key("delete_branch_after_merge"), &json!(false));
    assert!(!body.to_string().contains("force_merge"), "{body}");
    assert!(
        !body.to_string().contains("merge_when_checks_succeed"),
        "{body}"
    );
    assert_eq!(body.assert_key("Do"), &json!("merge"));
    assert_eq!(body.assert_key("head_commit_id"), &json!(REVISION));
}

/// M16: `request` for an open pull request whose head differs from the receipt is the bound-head
/// identity error, and no `POST` is sent.
#[tokio::test]
async fn m16_a_moved_head_is_the_identity_error_and_no_post_is_sent() {
    let fixture = Fixture::new().await;
    let authority = new_authority(&fixture).await;
    fixture.queue(version());
    gate_reads_without_rule(&fixture, &open_pull(MOVED_REVISION));

    let error = merge::request(&authority, &receipt(), credential())
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        format!(
            "source delivery identity mismatch: Forgejo pull request 7 changed the bound review \
             head: head revision {MOVED_REVISION} instead of {REVISION}. Restore the bound review \
             identity before resuming."
        )
    );
    assert!(
        !recorded(&authority),
        "a refused read is not an unconfirmed attempt"
    );

    let requests = fixture.requests();
    assert_eq!(
        requests.len(),
        3,
        "the rule and the statuses are not read for a refused head"
    );
    assert_eq!(requests.assert_at(2).uri, PULL_URI);
    assert!(writes(&requests).is_empty(), "{requests:?}");
}
