use std::sync::{Arc, Mutex};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use super::*;

#[path = "http_fixture.rs"]
mod http_fixture;
use http_fixture::{IntoResponse, Json, Response};

const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MERGED: &str = "cccccccccccccccccccccccccccccccccccccccc";
const REPOSITORY: &str = "group/sub/project";
const ORIGIN: &str = "https://gitlab.example/service";

struct StateData {
    version: String,
    project: Value,
    review: Option<Value>,
    notes: Vec<Value>,
    note_pages: BTreeMap<u32, Vec<Value>>,
    note_headers: BTreeMap<u32, Vec<String>>,
    discussion_note: Value,
    calls: Vec<(String, String, Value)>,
    lose_create: bool,
    lose_merge: bool,
    move_head_during_feedback: bool,
    oversized_notes: bool,
    responses: BTreeMap<(String, String), (StatusCode, Value)>,
}

struct Fixture {
    authority: GitLabDeliveryAuthority,
    state: Arc<Mutex<StateData>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let state = Arc::new(Mutex::new(StateData {
            version: "19.5.1-ee".into(),
            project: project(),
            review: None,
            notes: Vec::new(),
            note_pages: BTreeMap::new(),
            note_headers: BTreeMap::new(),
            discussion_note: note(1, "Please test the edge case."),
            calls: Vec::new(),
            lose_create: false,
            lose_merge: false,
            move_head_during_feedback: false,
            oversized_notes: false,
            responses: BTreeMap::new(),
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/api/v4/", listener.local_addr().unwrap());
        let handler_state = state.clone();
        let server = tokio::spawn(http_fixture::serve(listener, move |request| {
            handler(&handler_state, request)
        }));
        let mut authority =
            GitLabDeliveryAuthority::new(SourceForgeBaseUrl::new(ORIGIN).unwrap(), None).unwrap();
        authority.api = Api::loopback(&endpoint).unwrap();
        Self {
            authority,
            state,
            server,
        }
    }

    fn open_review(&self) {
        self.state.lock().unwrap().review = Some(merge_request());
    }

    fn response(&self, method: &str, suffix: &str, response: (StatusCode, Value)) {
        self.state
            .lock()
            .unwrap()
            .responses
            .insert((method.to_owned(), suffix.to_owned()), response);
    }

    fn writes(&self, suffix: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(method, path, _)| method != "GET" && path.ends_with(suffix))
            .count()
    }
}

fn project() -> Value {
    json!({
        "id":11, "path_with_namespace":REPOSITORY,
        "web_url":format!("{ORIGIN}/{REPOSITORY}"), "merge_method":"merge",
        "only_allow_merge_if_pipeline_succeeds":true,
        "allow_merge_on_skipped_pipeline":false,
        "only_allow_merge_if_all_discussions_are_resolved":true,
        "merge_trains_enabled":false,
    })
}

fn merge_request() -> Value {
    json!({
        "id":123, "iid":17, "source_project_id":11, "target_project_id":11,
        "source_branch":"zeroshot/test", "target_branch":"main", "sha":HEAD,
        "state":"opened", "detailed_merge_status":"mergeable", "draft":false,
        "has_conflicts":false, "blocking_discussions_resolved":true,
        "head_pipeline":{"id":81,"project_id":11,"sha":HEAD,"status":"success","source":"push"},
        "merge_commit_sha":null, "squash_commit_sha":null,
        "title":"Fix the behavior",
        "description":format!("{DELIVERY_GENERATED_BODY_START}\nPreserve behavior.\n{DELIVERY_GENERATED_BODY_END}"),
        "merge_when_pipeline_succeeds":false,
    })
}

fn note(id: u64, body: &str) -> Value {
    json!({
        "id":id,"author":{"id":31},"body":body,"noteable_type":"MergeRequest",
        "noteable_id":123,"noteable_iid":17,"project_id":11,"system":false,
        "updated_at":"2026-10-28T12:00:00Z",
    })
}

fn request() -> SourceReviewRequest {
    SourceReviewRequest {
        target: DeliveryTarget::new(REPOSITORY, "main", BASE).unwrap(),
        head_branch: "zeroshot/test".into(),
        head_revision: HEAD.into(),
        title: "Fix the behavior".into(),
        description: "Preserve behavior.".into(),
        source_issue: None,
    }
}

fn receipt() -> SourceReviewReceipt {
    SourceReviewReceipt::for_request(&request(), "17")
}

fn destination() -> SourceCommentDestination {
    comment_destination_at(
        &format!("{ORIGIN}/{REPOSITORY}/-/merge_requests/17"),
        &SourceForgeBaseUrl::new(ORIGIN).unwrap(),
    )
    .unwrap()
}

fn credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token("test-private-token")
}

fn overridden_response(state: &StateData, method: &Method, path: &str) -> Option<Response> {
    state
        .responses
        .iter()
        .find(|((verb, suffix), _)| verb == method.as_str() && path.ends_with(suffix))
        .map(|(_, (status, body))| (*status, Json(body.clone())).into_response())
}

fn handler(state: &Arc<Mutex<StateData>>, request: http_fixture::Request) -> Response {
    let http_fixture::Request {
        method,
        uri,
        headers,
        body,
    } = request;
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer test-private-token")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let path = uri.path();
    let mut state = state.lock().unwrap();
    state
        .calls
        .push((method.to_string(), path.to_owned(), body.clone()));
    let query = uri.query().unwrap_or("");
    if let Some(response) =
        overridden_response(&state, &method, path).or_else(|| simple_response(&state, path))
    {
        return response;
    }
    if path.ends_with("/merge_requests") {
        return list_or_create(&mut state, &method, body);
    }
    if path.ends_with("/merge_requests/17/merge") {
        return merge_response(&mut state, body);
    }
    if path.ends_with("/merge_requests/17/discussions") {
        return discussions_response(&mut state);
    }
    if path.contains("/notes") {
        return notes_response(&mut state, &method, (path, query), body);
    }
    if path.ends_with("/merge_requests/17") {
        return review_response(&mut state, &method, body);
    }
    if path.ends_with("/projects/22") {
        let mut fork = project();
        fork["id"] = json!(22);
        fork["path_with_namespace"] = json!("contributor/fork");
        fork["web_url"] = json!(format!("{ORIGIN}/contributor/fork"));
        return Json(fork).into_response();
    }
    Json(state.project.clone()).into_response()
}

fn simple_response(state: &StateData, path: &str) -> Option<Response> {
    let value = if path == "/api/v4/version" {
        json!({"version":state.version})
    } else if path.ends_with("/repository/branches/main") {
        json!({"name":"main","commit":{"id":BASE}})
    } else if path.contains("/repository/branches/") {
        json!({"name":"zeroshot/test","commit":{"id":HEAD}})
    } else if path == "/api/v4/user" {
        json!({"id":31})
    } else {
        return None;
    };
    Some(Json(value).into_response())
}

fn list_or_create(state: &mut StateData, method: &Method, body: Value) -> Response {
    if method == Method::GET {
        let values = state.review.clone().into_iter().collect::<Vec<_>>();
        return Json(values).into_response();
    }
    let mut created = merge_request();
    created["title"] = body["title"].clone();
    created["description"] = body["description"].clone();
    state.review = Some(created.clone());
    if state.lose_create {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (StatusCode::CREATED, Json(created)).into_response()
}

fn review_response(state: &mut StateData, method: &Method, body: Value) -> Response {
    let Some(review) = &mut state.review else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if method == Method::PUT {
        review["title"] = body["title"].clone();
        review["description"] = body["description"].clone();
    }
    Json(review.clone()).into_response()
}

fn merge_response(state: &mut StateData, body: Value) -> Response {
    let Some(review) = &mut state.review else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if body["sha"] != review["sha"] {
        return StatusCode::CONFLICT.into_response();
    }
    assert_eq!(body["auto_merge"], false);
    review["state"] = json!("merged");
    review["merge_commit_sha"] = json!(MERGED);
    if state.lose_merge {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(review.clone()).into_response()
}

fn discussions_response(state: &mut StateData) -> Response {
    let result = json!([{"id":"discussion-a","notes":[state.discussion_note.clone()]}]);
    if state.move_head_during_feedback {
        state.review.as_mut().unwrap()["sha"] = json!(MERGED);
    }
    Json(result).into_response()
}

fn notes_response(
    state: &mut StateData,
    method: &Method,
    location: (&str, &str),
    body: Value,
) -> Response {
    let (path, query) = location;
    if state.oversized_notes {
        return Json(json!({"body":"x".repeat(1024 * 1024 + 1)}))
            .into_response()
            .header("X-Next-Page", "");
    }
    if method == Method::POST {
        let created = note(501, body["body"].as_str().unwrap());
        state.notes.push(created.clone());
        return (StatusCode::CREATED, Json(created)).into_response();
    }
    if path.ends_with("/notes/501") {
        return match state.notes.first() {
            Some(value) => Json(value.clone()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    listed_notes_response(state, query)
}

#[tokio::test]
async fn creates_nested_project_review_and_renders_instance_links() {
    let fixture = Fixture::new().await;
    let result = fixture
        .authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap();
    assert_eq!(result.review_id, "17");
    let links = fixture.authority.review_links(&result).unwrap().unwrap();
    assert_eq!(
        links.url,
        format!("{ORIGIN}/{REPOSITORY}/-/merge_requests/17")
    );
    assert_eq!(fixture.writes("/merge_requests"), 1);
    let calls = &fixture.state.lock().unwrap().calls;
    assert!(
        calls
            .iter()
            .any(|(_, path, _)| path.contains("group%2Fsub%2Fproject"))
    );
}

#[tokio::test]
async fn lost_create_response_reconciles_the_exact_review_without_duplicate_post() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().lose_create = true;
    fixture
        .authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap();
    fixture
        .authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap();
    assert_eq!(fixture.writes("/merge_requests"), 1);
}

#[tokio::test]
async fn merges_only_the_exact_reviewed_head_and_confirms_integrated_revision() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Accepted
    );
    let result = fixture
        .authority
        .inspect_review(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(
        result.state,
        SourceReviewState::Merged {
            merge_revision: MERGED.into()
        }
    );
    assert_eq!(fixture.writes("/merge"), 1);
}

#[tokio::test]
async fn changed_head_refuses_merge_before_any_effect() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().review.as_mut().unwrap()["sha"] = json!(MERGED);
    assert!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn lost_merge_response_is_reconciled_during_safe_handoff() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().lose_merge = true;
    assert!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .authority
            .withdraw_merge_request(&receipt(), credential())
            .await
            .unwrap(),
        SourceMergeWithdrawal::Merged {
            merge_revision: MERGED.into()
        }
    );
}

#[tokio::test]
async fn unknown_policy_and_manual_pipeline_never_authorize_merge() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    for status in [
        "checking",
        "security_policy_violations",
        "status_checks_must_pass",
        "future_status",
    ] {
        fixture.state.lock().unwrap().review.as_mut().unwrap()["detailed_merge_status"] =
            json!(status);
        let result = fixture
            .authority
            .inspect_review(&receipt(), credential())
            .await
            .unwrap();
        assert_eq!(
            result.state,
            SourceReviewState::Open {
                checks: SourceChecks::Pending
            }
        );
        assert_eq!(
            fixture
                .authority
                .request_merge(&receipt(), credential())
                .await
                .unwrap(),
            SourceMergeRequestOutcome::Pending
        );
    }
    {
        let mut state = fixture.state.lock().unwrap();
        let mr = state.review.as_mut().unwrap();
        mr["detailed_merge_status"] = json!("mergeable");
        mr["head_pipeline"]["status"] = json!("manual");
    }
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Pending
    );
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn approval_handoff_requires_independently_green_ci_and_resolved_discussions() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().review.as_mut().unwrap()["detailed_merge_status"] =
        json!("not_approved");
    let ready = fixture
        .authority
        .inspect_review(&receipt(), credential())
        .await
        .unwrap();
    assert!(ready.pull_request_ready);
    fixture.state.lock().unwrap().review.as_mut().unwrap()["blocking_discussions_resolved"] =
        json!(false);
    let waiting = fixture
        .authority
        .inspect_review(&receipt(), credential())
        .await
        .unwrap();
    assert!(!waiting.pull_request_ready);
    assert_eq!(
        waiting.state,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
}

#[tokio::test]
async fn feedback_is_fenced_against_mid_read_head_changes() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(feedback.items.len(), 1);
    fixture.state.lock().unwrap().move_head_during_feedback = true;
    assert!(
        fixture
            .authority
            .inspect_review_feedback(&receipt(), credential())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn comment_post_get_and_pagination_use_validated_note_identity() {
    let fixture = Fixture::new().await;
    let destination = destination();
    assert_eq!(fixture.authority.actor(credential()).await.unwrap(), "31");
    let posted = fixture
        .authority
        .create_comment(&destination, "Report.", credential())
        .await
        .unwrap();
    assert_eq!(posted.url, format!("{}#note_501", destination.url));
    fixture
        .authority
        .validate_comment(&destination, &posted)
        .unwrap();
    assert_eq!(
        fixture
            .authority
            .get_comment(&destination, "501", credential())
            .await
            .unwrap(),
        posted
    );
    let page = fixture
        .authority
        .list_comments(&destination, 1, credential())
        .await
        .unwrap();
    assert_eq!(page.comments, vec![posted]);
    assert!(!page.has_more);
}

#[tokio::test]
async fn slash_commands_and_cross_instance_comments_fail_before_post() {
    let fixture = Fixture::new().await;
    let destination = destination();
    assert!(
        fixture
            .authority
            .create_comment(&destination, "Report\n/close", credential())
            .await
            .is_err()
    );
    let mut foreign = destination.clone();
    foreign.url = foreign.url.replace("gitlab.example", "other.example");
    assert!(
        fixture
            .authority
            .create_comment(&foreign, "Report", credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/notes"), 0);
}

#[tokio::test]
async fn old_server_and_oversized_response_fail_closed() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().version = "19.4.9-ee".into();
    assert!(fixture.authority.actor(credential()).await.is_err());
    assert_eq!(fixture.state.lock().unwrap().calls.len(), 1);
    fixture.state.lock().unwrap().version = "19.5.1-ee".into();
    fixture.state.lock().unwrap().oversized_notes = true;
    assert!(
        fixture
            .authority
            .list_comments(&destination(), 1, credential())
            .await
            .is_err()
    );
}

#[test]
fn pure_destination_and_locator_helpers_keep_nested_paths_and_base_prefix() {
    let base = SourceForgeBaseUrl::new(ORIGIN).unwrap();
    assert!(comment_destination_at(&format!("{ORIGIN}/{REPOSITORY}/-/issues/12"), &base).is_some());
    assert!(
        comment_destination_at(
            &format!("{ORIGIN}/{REPOSITORY}/-/issues/12?redirect=yes"),
            &base
        )
        .is_none()
    );
    assert_eq!(
        repository_reference(&format!("{ORIGIN}/{REPOSITORY}.git"), &base).as_deref(),
        Some(REPOSITORY)
    );
    assert!(
        comment_destination("https://gitlab.com/group/sub/project/-/merge_requests/4").is_some()
    );
    assert!(Api::new("http://127.0.0.1/api/v4/").is_err());
}

#[tokio::test]
async fn required_merge_trains_never_use_a_synchronous_merge_bypass() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    let initially_observed = fixture
        .authority
        .project(REPOSITORY, credential())
        .await
        .unwrap();
    assert_eq!(initially_observed.merge_trains_enabled, Some(false));
    fixture.state.lock().unwrap().project["merge_trains_enabled"] = json!(true);
    let observed = fixture
        .authority
        .inspect_review(&receipt(), credential())
        .await
        .unwrap();
    assert!(!observed.pull_request_ready);
    assert_eq!(
        observed.state,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Pending
    );
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn unknown_merge_settlement_refuses_a_worker_handoff() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().lose_merge = true;
    assert!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .is_err()
    );
    // Simulate a subsequent observation that cannot prove whether the in-flight merge settled.
    {
        let mut state = fixture.state.lock().unwrap();
        let review = state.review.as_mut().unwrap();
        review["state"] = json!("opened");
        review["merge_commit_sha"] = Value::Null;
    }
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Pending,
    );
    assert!(
        fixture
            .authority
            .withdraw_merge_request(&receipt(), credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge"), 1);
}

fn captured_scope() -> Value {
    json!({
        "baseRepository":REPOSITORY, "baseBranch":"main", "baseRevision":BASE,
        "headRepository":REPOSITORY, "headBranch":"zeroshot/test", "headRevision":HEAD,
    })
}

#[tokio::test]
async fn captured_comment_scope_verifies_server_fields_and_exact_types() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture
        .authority
        .verify_review_scope(&destination(), &captured_scope(), credential())
        .await
        .unwrap();
    let mut wrong = captured_scope();
    wrong["headRevision"] = json!(123);
    assert!(
        fixture
            .authority
            .verify_review_scope(&destination(), &wrong, credential())
            .await
            .is_err()
    );
    wrong = captured_scope();
    wrong["baseRevision"] = json!(MERGED);
    assert!(
        fixture
            .authority
            .verify_review_scope(&destination(), &wrong, credential())
            .await
            .is_err()
    );
    wrong = captured_scope();
    wrong["headRepository"] = json!("somebody/else");
    assert!(
        fixture
            .authority
            .verify_review_scope(&destination(), &wrong, credential())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn captured_comment_scope_uses_fork_project_identity_instead_of_iid_as_global_id() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().review.as_mut().unwrap()["source_project_id"] = json!(22);
    let mut scope = captured_scope();
    scope["headRepository"] = json!("contributor/fork");
    fixture
        .authority
        .verify_review_scope(&destination(), &scope, credential())
        .await
        .unwrap();
    assert!(
        fixture
            .authority
            .verify_review_scope(&destination(), &captured_scope(), credential())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn metadata_refresh_preserves_human_text_and_refuses_reserved_marker_ambiguity() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    let existing = fixture.state.lock().unwrap().review.as_ref().unwrap()["description"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.state.lock().unwrap().review.as_mut().unwrap()["description"] =
        json!(format!("Human preface.\n{existing}\nHuman follow-up."));
    let mut next = request();
    next.description = "Updated report.".into();
    fixture
        .authority
        .open_or_update_review(&next, credential())
        .await
        .unwrap();
    let description = fixture.state.lock().unwrap().review.as_ref().unwrap()["description"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(description.starts_with("Human preface."));
    assert!(description.ends_with("Human follow-up."));
    assert!(description.contains("Updated report."));
    fixture.state.lock().unwrap().review.as_mut().unwrap()["description"] =
        json!(format!("{description}\n{DELIVERY_GENERATED_BODY_START}"));
    assert!(
        fixture
            .authority
            .open_or_update_review(&next, credential())
            .await
            .is_err()
    );
}

#[test]
fn pure_note_permalink_validation_rejects_wrong_resources_and_noncanonical_ids() {
    let bound = destination();
    assert!(valid_comment_permalink(
        &bound,
        "12",
        &format!("{}#note_12", bound.url)
    ));
    assert!(!valid_comment_permalink(
        &bound,
        "12",
        &format!("{}#note_13", bound.url)
    ));
    assert!(!valid_comment_permalink(
        &bound,
        "12",
        &format!("{}#note_12?other=1", bound.url)
    ));
    assert!(!valid_comment_permalink(
        &bound,
        "012",
        &format!("{}#note_012", bound.url)
    ));
    let mut wrong = bound.clone();
    wrong.discussion_id = "99".into();
    assert!(!valid_comment_permalink(
        &wrong,
        "12",
        &format!("{}#note_12", bound.url)
    ));
}

#[tokio::test]
async fn unknown_integrated_revision_never_falls_back_to_the_candidate_head() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().review.as_mut().unwrap()["state"] = json!("merged");
    fixture
        .authority
        .project(REPOSITORY, credential())
        .await
        .unwrap();
    fixture
        .authority
        .projects
        .lock()
        .unwrap()
        .get_mut(REPOSITORY)
        .unwrap()
        .merge_method = "ff".into();
    assert!(
        fixture
            .authority
            .inspect_review(&receipt(), credential())
            .await
            .is_err()
    );
    assert!(
        fixture
            .authority
            .withdraw_merge_request(&receipt(), credential())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn known_review_cannot_replace_admitted_target_identity() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    let request = request();
    let mut known = receipt();
    known.target_branch = "other".into();
    assert!(
        fixture
            .authority
            .observe_delivery(
                SourceDeliveryRead {
                    target: &request.target,
                    head_branch: &request.head_branch,
                    known_review: Some(&known),
                    include_review: true,
                },
                credential()
            )
            .await
            .is_err()
    );
    assert_eq!(fixture.state.lock().unwrap().calls.len(), 0);
}

#[tokio::test]
async fn asynchronously_prepared_review_heads_are_retryable_without_another_post() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().review.as_mut().unwrap()["sha"] = Value::Null;
    let error = fixture
        .authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap_err();
    assert!(error.retryable_review_sync());
    assert_eq!(fixture.writes("/merge_requests"), 0);
}

#[tokio::test]
async fn synthetic_pipeline_success_does_not_prove_ci_for_the_captured_head() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    {
        let mut state = fixture.state.lock().unwrap();
        let review = state.review.as_mut().unwrap();
        review["head_pipeline"]["sha"] = json!(MERGED);
        review["head_pipeline"]["source"] = json!("merge_request_event");
    }
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Pending
    );
    fixture.state.lock().unwrap().review.as_mut().unwrap()["detailed_merge_status"] =
        json!("not_approved");
    let observed = fixture
        .authority
        .inspect_review(&receipt(), credential())
        .await
        .unwrap();
    assert!(!observed.pull_request_ready);
    assert_eq!(
        observed.state,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn root_notes_require_the_exact_discussion_iid() {
    let fixture = Fixture::new().await;
    let destination = destination();
    fixture
        .authority
        .create_comment(&destination, "Report.", credential())
        .await
        .unwrap();
    for iid in [Value::Null, json!(18)] {
        {
            let mut state = fixture.state.lock().unwrap();
            state.notes[0]["noteable_iid"] = iid;
            state.notes[0]["noteable_id"] = json!(999);
        }
        assert!(
            fixture
                .authority
                .get_comment(&destination, "501", credential())
                .await
                .is_err()
        );
        assert!(
            fixture
                .authority
                .list_comments(&destination, 1, credential())
                .await
                .is_err()
        );
    }
    assert_eq!(fixture.writes("/notes"), 1);
}

#[tokio::test]
async fn null_discussion_iid_requires_the_fetched_global_merge_request_id() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().discussion_note["noteable_iid"] = Value::Null;
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(feedback.items.len(), 1);
    fixture.state.lock().unwrap().discussion_note["noteable_id"] = json!(999);
    assert!(
        fixture
            .authority
            .inspect_review_feedback(&receipt(), credential())
            .await
            .is_err()
    );
    {
        let mut state = fixture.state.lock().unwrap();
        state.discussion_note["noteable_id"] = json!(123);
        state.discussion_note["noteable_iid"] = json!(18);
    }
    assert!(
        fixture
            .authority
            .inspect_review_feedback(&receipt(), credential())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn comments_use_canonical_stored_text_and_refuse_quick_actions_after_cr_removal() {
    let fixture = Fixture::new().await;
    let input = "A finding.\r\nAnother finding.\r\n \t";
    assert_eq!(
        fixture.authority.prepare_comment_body(input).unwrap(),
        "A finding.\nAnother finding."
    );
    let posted = fixture
        .authority
        .create_comment(&destination(), input, credential())
        .await
        .unwrap();
    assert_eq!(posted.body, "A finding.\nAnother finding.");
    assert_eq!(
        fixture
            .authority
            .prepare_comment_body(&posted.body)
            .unwrap(),
        posted.body
    );
    assert!(
        fixture
            .authority
            .prepare_comment_body("Report.\n/clo\rse")
            .is_err()
    );
    assert!(
        fixture
            .authority
            .create_comment(&destination(), "Report.\n/clo\rse", credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/notes"), 1);
}

#[tokio::test]
async fn review_metadata_normalizes_cr_before_marker_and_quick_action_validation() {
    let fixture = Fixture::new().await;
    let mut request = request();
    request.description = "First finding.\r\nSecond finding.  \r\n".into();
    fixture
        .authority
        .open_or_update_review(&request, credential())
        .await
        .unwrap();
    let description = fixture.state.lock().unwrap().review.as_ref().unwrap()["description"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        description,
        format!(
            "{DELIVERY_GENERATED_BODY_START}\nFirst finding.\nSecond finding.\n{DELIVERY_GENERATED_BODY_END}"
        )
    );
    for invalid in [
        DELIVERY_GENERATED_BODY_START.replace("zeroshot", "zero\rshot"),
        "Report.\n/clo\rse".into(),
    ] {
        request.description = invalid;
        assert!(
            fixture
                .authority
                .open_or_update_review(&request, credential())
                .await
                .is_err()
        );
    }
    assert_eq!(fixture.writes("/merge_requests"), 1);
    assert_eq!(fixture.writes("/merge_requests/17"), 0);
}

#[test]
fn current_work_item_issue_urls_preserve_the_exact_destination() {
    let url = format!("{ORIGIN}/{REPOSITORY}/-/work_items/17");
    let base = SourceForgeBaseUrl::new(ORIGIN).unwrap();
    let destination = comment_destination_at(&url, &base).unwrap();
    assert_eq!(destination.kind, SourceDiscussionKind::Issue);
    assert_eq!(destination.repository, REPOSITORY);
    assert_eq!(destination.discussion_id, "17");
    assert!(valid_comment_permalink(
        &destination,
        "23",
        &format!("{url}#note_23")
    ));
    assert!(!valid_comment_permalink(
        &destination,
        "23",
        &format!("{ORIGIN}/{REPOSITORY}/-/work_items/18#note_23")
    ));
    assert!(!valid_comment_permalink(
        &destination,
        "23",
        &format!("{ORIGIN}/{REPOSITORY}/-/issues/17#note_23")
    ));
}

fn listed_notes_response(state: &StateData, query: &str) -> Response {
    let page = url::form_urlencoded::parse(query.as_bytes())
        .find_map(|(key, value)| (key == "page").then(|| value.parse::<u32>().unwrap()))
        .unwrap();
    let notes = if page == 1 {
        state.notes.clone()
    } else {
        state.note_pages.get(&page).cloned().unwrap_or_default()
    };
    let headers = state
        .note_headers
        .get(&page)
        .cloned()
        .unwrap_or_else(|| vec![String::new()]);
    let mut response = Json(notes).into_response();
    for next in headers {
        response = response.header("X-Next-Page", &next);
    }
    response
}

#[tokio::test]
async fn short_note_pages_continue_from_numeric_server_metadata() {
    let fixture = Fixture::new().await;
    {
        let mut state = fixture.state.lock().unwrap();
        state.notes = vec![note(1, "First page.")];
        state.note_pages.insert(2, vec![note(2, "Continued page.")]);
        state.note_headers.insert(1, vec!["2".into()]);
    }
    let first = fixture
        .authority
        .list_comments(&destination(), 1, credential())
        .await
        .unwrap();
    assert_eq!(first.comments.len(), 1);
    assert!(first.has_more);
    let second = fixture
        .authority
        .list_comments(&destination(), 2, credential())
        .await
        .unwrap();
    assert_eq!(second.comments[0].id, "2");
    assert!(!second.has_more);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(method, path, _)| method == "GET" && path.ends_with("/notes"))
            .count(),
        2
    );
    assert_eq!(fixture.writes("/notes"), 0);
}

#[tokio::test]
async fn filtered_empty_page_can_continue_and_full_last_page_can_finish() {
    let fixture = Fixture::new().await;
    fixture
        .state
        .lock()
        .unwrap()
        .note_headers
        .insert(1, vec!["2".into()]);
    let first = fixture
        .authority
        .list_comments(&destination(), 1, credential())
        .await
        .unwrap();
    assert!(first.comments.is_empty());
    assert!(first.has_more);
    fixture
        .state
        .lock()
        .unwrap()
        .note_pages
        .insert(2, (1..=100).map(|id| note(id, "Last page.")).collect());
    let last = fixture
        .authority
        .list_comments(&destination(), 2, credential())
        .await
        .unwrap();
    assert_eq!(last.comments.len(), 100);
    assert!(!last.has_more);
}

#[tokio::test]
async fn missing_ambiguous_or_nonconsecutive_notes_pagination_never_proves_completion() {
    let fixture = Fixture::new().await;
    for headers in [
        Vec::new(),
        vec!["2".into(), "2".into()],
        vec!["0".into()],
        vec!["1".into()],
        vec!["3".into()],
        vec!["02".into()],
        vec!["+2".into()],
        vec!["two".into()],
        vec!["4294967296".into()],
        vec!["https://foreign.example/notes?page=2".into()],
    ] {
        fixture
            .state
            .lock()
            .unwrap()
            .note_headers
            .insert(1, headers);
        assert!(
            fixture
                .authority
                .list_comments(&destination(), 1, credential())
                .await
                .is_err()
        );
    }
    fixture
        .state
        .lock()
        .unwrap()
        .note_headers
        .insert(api::PAGE_LIMIT, vec![(api::PAGE_LIMIT + 1).to_string()]);
    assert!(
        fixture
            .authority
            .list_comments(&destination(), api::PAGE_LIMIT, credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/notes"), 0);
}

#[path = "policy_tests.rs"]
mod policy_tests;

#[path = "effect_tests.rs"]
mod effect_tests;
