use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::source_code_provider::adapters::core::http_fixture::{
    self, IntoResponse, Json, Request, Response,
};
use reqwest::{Method, StatusCode};

use super::*;
use super::super::queue::merge;
use super::super::http::Http;

const TOKEN: SourceDeliveryCredential<'_> = SourceDeliveryCredential::token("test-token");
const MERGED_SHA: &str = "dddddddddddddddddddddddddddddddddddddddd";

struct Server {
    authority: GhCliDeliveryAuthority,
    task: JoinHandle<()>,
}

impl Server {
    async fn start(
        root: &std::path::Path,
        handler: impl Fn(Request) -> Response + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
        let base = format!("http://{}/", listener.local_addr().assert_value());
        let mut config = GhCliAuthorityConfig::hosted(root.to_owned());
        config.api_deadline = Duration::from_secs(5);
        let authority = GhCliDeliveryAuthority::new(config);
        authority
            .http
            .set(Http::loopback(&base, "2026-03-10").assert_value())
            .assert_value();
        let task = tokio::spawn(http_fixture::serve(listener, handler));
        Self { authority, task }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn graph(request: &Request) -> Value {
    assert_eq!(request.uri.path(), "/graphql");
    assert_eq!(request.method, Method::POST);
    assert_eq!(request.headers["authorization"], "Bearer test-token");
    serde_json::from_slice(&request.body).assert_value()
}

fn merged_policy() -> Value {
    let mut policy = policy_page_with_merge_capabilities(true, true, true);
    set_review_state(&mut policy, "MERGED", true);
    policy["data"]["repository"]["pullRequest"]["mergeCommit"] = json!({"oid": MERGED_SHA});
    policy
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn cancelled_direct_merge_drains_its_submission_before_withdrawal() {
    let root = tempfile::tempdir().assert_value();
    let submitted = Arc::new(Notify::new());
    let observed = Arc::new(Notify::new());
    let (release, waiting) = mpsc::channel();
    let waiting = Mutex::new(waiting);
    let server = Server::start(root.path(), {
        let submitted = submitted.clone();
        let observed = observed.clone();
        move |request| {
            let body = graph(&request);
            let query = body["query"].as_str().assert_value();
            if query.contains("mergePullRequest") {
                assert_eq!(
                    body["variables"],
                    json!({
                        "pullRequestId": "PR_node_17",
                        "expectedHeadOid": review().head_revision,
                        "method": "SQUASH",
                    })
                );
                submitted.notify_one();
                waiting
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3))
                    .assert_value();
                return Json(direct_merge_response(MERGED_SHA)).into_response();
            }
            observed.notify_one();
            Json(merged_policy()).into_response()
        }
    })
    .await;
    let merge = tokio::spawn({
        let authority = server.authority.clone();
        async move {
            merge(
                &authority,
                &review(),
                (MergeMethod::Squash, "PR_node_17"),
                TOKEN,
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), submitted.notified())
        .await
        .assert_value();
    merge.abort();
    assert!(merge.await.assert_error().is_cancelled());
    let withdrawal = tokio::spawn({
        let authority = server.authority.clone();
        async move { authority.withdraw_merge_request(&review(), TOKEN).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), observed.notified())
            .await
            .is_err()
    );
    release.send(()).assert_value();
    assert_eq!(
        withdrawal.await.assert_value().assert_value(),
        SourceMergeWithdrawal::Merged {
            merge_revision: MERGED_SHA.to_owned()
        },
    );
}

#[tokio::test]
async fn incomplete_mutation_status_cannot_authorize_replay_or_withdrawal() {
    for method in [MergeMethod::Queue, MergeMethod::Merge] {
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert_unconfirmed_submission(
                method,
                move || {
                    (status, Json(json!({"message":"dropped response"})))
                        .into_response()
                        .content_length(1024)
                },
                &status.as_u16().to_string(),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn only_a_definite_queue_refusal_allows_a_different_merge_method() {
    for unresolved in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let calls = Arc::new(AtomicUsize::new(0));
        let server = Server::start(root.path(), {
            let calls = calls.clone();
            move |request| {
                observe_or_submit(request, || match calls.fetch_add(1, Ordering::Relaxed) {
                    attempt if attempt == usize::from(unresolved) => Json(json!({
                        "data":{"enqueuePullRequest":null},
                        "errors":[{"type":"UNPROCESSABLE","path":["enqueuePullRequest"],
                            "message":"Merge queue policy rejected enqueue"}],
                    }))
                    .into_response(),
                    0 => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                    _ => Json(direct_merge_response(MERGED_SHA)).into_response(),
                })
            }
        })
        .await;
        let review = review();
        let request = |method| merge(&server.authority, &review, (method, "PR_node_17"), TOKEN);
        request(MergeMethod::Queue).await.assert_error();
        assert_eq!(
            request(MergeMethod::Queue).await.assert_error().rejection,
            Some(api::MutationRejection::Policy),
        );
        let direct = request(MergeMethod::Merge).await;
        if unresolved {
            assert_eq!(
                direct.assert_error().rejection,
                Some(api::MutationRejection::Policy)
            );
        } else {
            direct.assert_value();
        }
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }
}

#[tokio::test]
async fn graph_mutation_redirect_keeps_the_submission_owned() {
    assert_unconfirmed_submission(
        MergeMethod::Merge,
        || {
            StatusCode::SEE_OTHER
                .into_response()
                .header("location", "/redirected-mutation")
        },
        "303",
    )
    .await;
}

#[tokio::test]
async fn a_pre_send_connection_refusal_allows_withdrawal_and_retry() {
    let root = tempfile::tempdir().assert_value();
    let socket = TcpSocket::new_v4().assert_value();
    socket
        .bind("127.0.0.1:0".parse().assert_value())
        .assert_value();
    let base = format!("http://{}/", socket.local_addr().assert_value());
    let authority =
        GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted(root.path().to_owned()));
    authority
        .http
        .set(Http::loopback(&base, "2026-03-10").assert_value())
        .assert_value();
    let failed = merge(
        &authority,
        &review(),
        (MergeMethod::Queue, "PR_node_17"),
        TOKEN,
    )
    .await
    .assert_error();
    assert_eq!(failed.rejection, Some(api::MutationRejection::NotSent));
    assert!(failed.error.retryable_operation());

    let received = Arc::new(AtomicUsize::new(0));
    let listener = socket.listen(128).assert_value();
    let task = tokio::spawn(http_fixture::serve(listener, {
        let received = received.clone();
        move |request| {
            observe_or_submit(request, || {
                received.fetch_add(1, Ordering::Relaxed);
                Json(enqueue_response()).into_response()
            })
        }
    }));
    let server = Server { authority, task };
    assert_eq!(
        server
            .authority
            .withdraw_merge_request(&review(), TOKEN)
            .await
            .assert_value(),
        SourceMergeWithdrawal::Withdrawn,
    );
    assert_eq!(received.load(Ordering::Relaxed), 0);
    merge(
        &server.authority,
        &review(),
        (MergeMethod::Queue, "PR_node_17"),
        TOKEN,
    )
    .await
    .assert_value();
    assert_eq!(received.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn malformed_or_truncated_queue_output_is_redacted_and_never_resubmitted() {
    assert_unconfirmed_submission(
        MergeMethod::Queue,
        || Response::raw(b"not-json test-token".to_vec()),
        "invalid JSON",
    )
    .await;
    assert_unconfirmed_submission(
        MergeMethod::Queue,
        || Response::raw(b"test-token".to_vec()).content_length(2 * 1024 * 1024),
        "byte bound",
    )
    .await;
}

#[tokio::test]
async fn invalid_direct_merge_receipts_keep_the_submission_owned() {
    let receipt = direct_merge_response(MERGED_SHA);
    let mut changed_head = receipt.clone();
    changed_head["data"]["mergePullRequest"]["pullRequest"]["headRefOid"] = json!(MERGED_SHA);
    let mut missing_commit = receipt.clone();
    missing_commit["data"]["mergePullRequest"]["pullRequest"]["mergeCommit"] = Value::Null;
    let mut mixed = receipt;
    mixed["errors"] = json!([{
        "path":["mergePullRequest","pullRequest"],
        "type":"UNPROCESSABLE",
        "message":"inconsistent reply"
    }]);
    for response in [changed_head, missing_commit, mixed] {
        assert_unconfirmed_submission(
            MergeMethod::Merge,
            move || Json(&response).into_response(),
            "GitHub",
        )
        .await;
    }
}

async fn assert_unconfirmed_submission(
    method: MergeMethod,
    response: impl Fn() -> Response + Send + Sync + 'static,
    diagnostic: &str,
) {
    let root = tempfile::tempdir().assert_value();
    let mutations = Arc::new(AtomicUsize::new(0));
    let server = Server::start(root.path(), {
        let mutations = mutations.clone();
        move |request| {
            observe_or_submit(request, || {
                mutations.fetch_add(1, Ordering::Relaxed);
                response()
            })
        }
    })
    .await;
    for _ in 0..2 {
        let failed = merge(&server.authority, &review(), (method, "PR_node_17"), TOKEN)
            .await
            .assert_error();
        assert_eq!(failed.error.api_status(), None);
        assert!(!failed.error.authentication_failed());
        assert!(!failed.error.retryable_operation());
        let rendered = failed.error.to_string();
        assert!(rendered.contains(diagnostic), "{rendered}");
        assert!(!rendered.contains("test-token"), "{rendered}");
    }
    let error = server
        .authority
        .withdraw_merge_request(&review(), TOKEN)
        .await
        .assert_error();
    assert!(
        error.to_string().contains("submission remains unconfirmed"),
        "{error}"
    );
    assert_eq!(mutations.load(Ordering::Relaxed), 1);
}

fn observe_or_submit(request: Request, submit: impl FnOnce() -> Response) -> Response {
    if request.method == Method::GET {
        return Json(json!([])).into_response();
    }
    if graph(&request)["query"]
        .as_str()
        .assert_value()
        .trim_start()
        .starts_with("mutation")
    {
        return submit();
    }
    Json(queued(false, false)).into_response()
}
