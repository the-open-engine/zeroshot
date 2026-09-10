use openengine_cluster_protocol::MergePlanId;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::{
    CapturedHttpRequest, bind_target_authority, hosted_discovery, oauth_metadata,
    read_http_request, test_authority, token_response, write_http_response_with_status,
};
use super::super::fixtures::{TempRoot, hosted_target, temp_root};
use super::super::super::controller_authority::{TargetCredentialStore, TargetHttpControlAuthority};
use super::super::super::{TargetControlAuthority, TargetRecord};

const DEFAULT_RESPONSE_LIMIT: usize = 64 * 1024;
const MERGE_PLAN_RESPONSE_LIMIT: usize = 1024 * 1024;

#[tokio::test]
async fn merge_plan_polling_reuses_discovery_routes_and_access_token() {
    let body = merge_plan_body(dense_runs());
    let harness = merge_plan_harness(vec![body.clone(), body]).await;

    let plan_id = MergePlanId::new("plan-1");
    harness
        .authority
        .merge_plan_status(&harness.target, &plan_id)
        .await
        .assert_value();
    harness
        .authority
        .merge_plan_status(&harness.target, &plan_id)
        .await
        .assert_value();

    let requests = harness.server.await.assert_value();
    assert_eq!(
        request_count(&requests, "GET", "/.well-known/zeroshot-native-v2"),
        1
    );
    assert_eq!(request_count(&requests, "GET", "/oauth/metadata"), 1);
    assert_eq!(request_count(&requests, "POST", "/oauth/token"), 1);
    assert_eq!(request_count(&requests, "GET", "/session"), 1);
    assert_eq!(
        request_count(&requests, "GET", "/native-v2/merge-plans/plan-1"),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/native-v2/merge-plans/plan-1")
            .map(|request| request.authorization.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("Bearer access-1"), Some("Bearer access-1")]
    );
}

#[tokio::test]
async fn merge_plan_auth_rejection_refreshes_and_retries_once() {
    let body = merge_plan_body(vec![merge_plan_run("build", Vec::new(), None)]);
    for rejection in ["401 Unauthorized", "403 Forbidden"] {
        let harness = merge_plan_harness_with_responses(
            vec![(rejection, auth_rejection_body()), ("200 OK", body.clone())],
            2,
        )
        .await;

        harness
            .authority
            .merge_plan_status(&harness.target, &MergePlanId::new("plan-1"))
            .await
            .assert_value();

        let requests = harness.server.await.assert_value();
        assert_eq!(
            request_count(&requests, "GET", "/.well-known/zeroshot-native-v2"),
            2
        );
        assert_eq!(request_count(&requests, "POST", "/oauth/token"), 2);
        assert_eq!(
            merge_plan_authorizations(&requests),
            ["Bearer access-1", "Bearer access-2"]
        );
        let refreshes = refresh_token_bodies(&requests);
        assert!(refreshes[0].contains("refresh_token=refresh-0"));
        assert!(refreshes[1].contains("refresh_token=refresh-1"));
    }
}

#[tokio::test]
async fn merge_plan_auth_retry_is_bounded_and_leaves_rejected_retry_uncached() {
    let body = merge_plan_body(vec![merge_plan_run("build", Vec::new(), None)]);
    let harness = merge_plan_harness_with_responses(
        vec![
            ("401 Unauthorized", auth_rejection_body()),
            ("403 Forbidden", auth_rejection_body()),
            ("200 OK", body),
        ],
        3,
    )
    .await;
    let plan_id = MergePlanId::new("plan-1");

    let error = harness
        .authority
        .merge_plan_status(&harness.target, &plan_id)
        .await
        .assert_error();
    assert_eq!(
        error.to_string(),
        "merge-plan status request was rejected: access token rejected"
    );
    harness
        .authority
        .merge_plan_status(&harness.target, &plan_id)
        .await
        .assert_value();

    let requests = harness.server.await.assert_value();
    assert_eq!(request_count(&requests, "POST", "/oauth/token"), 3);
    assert_eq!(
        merge_plan_authorizations(&requests),
        ["Bearer access-1", "Bearer access-2", "Bearer access-3"]
    );
}

#[tokio::test]
async fn merge_plan_status_accepts_a_maximal_dense_dag_above_the_default_limit() {
    let body = merge_plan_body(dense_runs());
    assert!(body.len() > DEFAULT_RESPONSE_LIMIT);
    assert!(body.len() <= MERGE_PLAN_RESPONSE_LIMIT);
    let harness = merge_plan_harness(vec![body]).await;

    let plan = harness
        .authority
        .merge_plan_status(&harness.target, &MergePlanId::new("plan-1"))
        .await
        .assert_value();

    assert_eq!(plan.runs.len(), 64);
    harness.server.await.assert_value();
}

#[tokio::test]
async fn merge_plan_status_rejects_a_response_above_its_dedicated_limit() {
    let body = merge_plan_body(vec![merge_plan_run(
        "oversized",
        Vec::new(),
        Some("x".repeat(MERGE_PLAN_RESPONSE_LIMIT)),
    )]);
    assert!(body.len() > MERGE_PLAN_RESPONSE_LIMIT);
    let harness = merge_plan_harness(vec![body]).await;

    let error = harness
        .authority
        .merge_plan_status(&harness.target, &MergePlanId::new("plan-1"))
        .await
        .assert_error();

    assert_eq!(error.to_string(), "merge-plan status response is too large");
    harness.server.await.assert_value();
}

struct MergePlanHarness {
    _root: TempRoot,
    authority: TargetHttpControlAuthority,
    target: TargetRecord,
    server: tokio::task::JoinHandle<Vec<CapturedHttpRequest>>,
}

async fn merge_plan_harness(responses: Vec<String>) -> MergePlanHarness {
    merge_plan_harness_with_responses(
        responses.into_iter().map(|body| ("200 OK", body)).collect(),
        1,
    )
    .await
}

async fn merge_plan_harness_with_responses(
    responses: Vec<(&'static str, String)>,
    access_rounds: usize,
) -> MergePlanHarness {
    let (origin, server) = spawn_merge_plan_authority(responses, access_rounds).await;
    let root = temp_root();
    let (credentials, authority) = test_authority(&root);
    let target = hosted_target("prod", origin);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    MergePlanHarness {
        _root: root,
        authority,
        target,
        server,
    }
}

async fn spawn_merge_plan_authority(
    responses: Vec<(&'static str, String)>,
    access_rounds: usize,
) -> (String, tokio::task::JoinHandle<Vec<CapturedHttpRequest>>) {
    let (listener, _, origin) = bind_target_authority().await;
    let server_origin = origin.clone();
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        let mut token_index = 0_u8;
        let request_count = responses.len() + access_rounds * 4;
        let mut responses = responses.into_iter();
        for _ in 0..request_count {
            let (mut stream, request) = accept_merge_plan_request(&listener).await;
            let (status, body) = match (request.method.as_str(), request.path.as_str()) {
                ("GET", "/.well-known/zeroshot-native-v2") => {
                    ("200 OK", hosted_discovery(&server_origin))
                }
                ("GET", "/oauth/metadata") => ("200 OK", oauth_metadata(&server_origin)),
                ("POST", "/oauth/token") => ("200 OK", token_response(&mut token_index)),
                ("GET", "/session") => (
                    "200 OK",
                    json!({
                        "kind": "openengine.target-session/v1",
                        "organization_id": "organization-1",
                    })
                    .to_string(),
                ),
                ("GET", path) if path.starts_with("/native-v2/merge-plans/") => {
                    responses.next().assert_value()
                }
                unexpected => None::<(&str, String)>
                    .assert_value_with(&format!("unexpected authority request: {unexpected:?}")),
            };
            write_http_response_with_status(&mut stream, status, &body).await;
            captured.push(request);
        }
        captured
    });
    (origin, server)
}

async fn accept_merge_plan_request(
    listener: &tokio::net::TcpListener,
) -> (tokio::net::TcpStream, CapturedHttpRequest) {
    let (mut stream, _) = listener.accept().await.assert_value();
    let request = read_http_request(&mut stream).await;
    (stream, request)
}

fn dense_runs() -> Vec<Value> {
    let names = (0..64)
        .map(|index| format!("n{index:02}-{}", "x".repeat(60)))
        .collect::<Vec<_>>();
    names
        .iter()
        .enumerate()
        .map(|(index, name)| merge_plan_run(name, names.get(..index).assert_value().to_vec(), None))
        .collect()
}

fn merge_plan_run(name: &str, needs: Vec<String>, waiting_reason: Option<String>) -> Value {
    json!({
        "name": name,
        "runId": format!("run-{name}"),
        "state": "blocked",
        "needs": needs,
        "sourceRevision": null,
        "readyAt": null,
        "queueExpiresAt": null,
        "terminalAt": null,
        "waitingReason": waiting_reason,
        "errorCode": null
    })
}

fn merge_plan_body(runs: Vec<Value>) -> String {
    json!({
        "planId": "plan-1",
        "title": "Dense merge plan",
        "state": "queued",
        "repository": "open-engine/zeroshot",
        "branch": "main",
        "submittedAt": "2026-09-10T00:00:00Z",
        "expiresAt": "2026-09-11T00:00:00Z",
        "runs": runs
    })
    .to_string()
}

fn auth_rejection_body() -> String {
    json!({
        "code": "unauthorized",
        "message": "access token rejected"
    })
    .to_string()
}

fn merge_plan_authorizations(requests: &[CapturedHttpRequest]) -> Vec<&str> {
    requests
        .iter()
        .filter(|request| request.path == "/native-v2/merge-plans/plan-1")
        .filter_map(|request| request.authorization.as_deref())
        .collect()
}

fn refresh_token_bodies(requests: &[CapturedHttpRequest]) -> Vec<&str> {
    requests
        .iter()
        .filter(|request| request.path == "/oauth/token")
        .map(|request| request.body.as_str())
        .collect()
}

fn request_count(requests: &[CapturedHttpRequest], method: &str, path: &str) -> usize {
    requests
        .iter()
        .filter(|request| request.method == method && request.path == path)
        .count()
}
