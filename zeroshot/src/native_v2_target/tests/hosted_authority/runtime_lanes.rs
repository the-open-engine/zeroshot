use std::sync::{Arc, Mutex};

use openengine_cluster_protocol::{
    NODE_RUNTIME_LANES_KIND, NodeName, RunProfileName, RunProfileScope, RunProfileSetRequest,
    TargetAuthentication, TargetDiscoveryDocument, TargetRunRequest,
};
use openengine_cluster_testkit::admission::graph_fixture;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::super::fixtures::{
    direct_target, exact_run_request, hosted_target, temp_root, test_http_authority,
};
use super::super::super::TargetControlAuthority;
use super::super::super::controller_authority::TargetCredentialStore;
use super::{
    CapturedHttpRequest, bind_target_authority, hosted_discovery, oauth_metadata,
    read_http_request, spawn_direct_target_authority, test_authority, token_response,
    write_http_response_with_status,
};

const REFUSAL: &str =
    "target does not support per-node runtime lanes (openengine.node-runtime-lanes/v1)";
const DISCOVERY: (&str, &str) = ("GET", "/.well-known/zeroshot-native-v2");
const OAUTH_METADATA: (&str, &str) = ("GET", "/oauth/metadata");
const TOKEN: (&str, &str) = ("POST", "/oauth/token");
const SESSION: (&str, &str) = ("GET", "/session");
const RUN: (&str, &str) = ("POST", "/native-v2/run");
const PROFILE_SET: (&str, &str) = ("POST", "/native-v2/profiles/set");
const RUN_RECEIPT: &str = r#"{"runId":"018f5e78-7f95-7c22-8d98-3f15af20c991"}"#;

struct RecordingTarget {
    origin: String,
    requests: Arc<Mutex<Vec<CapturedHttpRequest>>>,
    server: tokio::task::JoinHandle<()>,
}

impl RecordingTarget {
    async fn spawn<F>(respond: F) -> Self
    where
        F: Fn(&CapturedHttpRequest, &str, &mut u8) -> (&'static str, String) + Send + 'static,
    {
        let (listener, _, origin) = bind_target_authority().await;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let served_origin = origin.clone();
        let server = tokio::spawn(async move {
            let mut token_index = 0_u8;
            loop {
                let (mut stream, _) = listener.accept().await.assert_value();
                let request = read_http_request(&mut stream).await;
                let (status, body) = respond(&request, &served_origin, &mut token_index);
                recorded.lock().assert_value().push(request);
                write_http_response_with_status(&mut stream, status, &body).await;
            }
        });
        Self {
            origin,
            requests,
            server,
        }
    }

    fn stop(self) -> Vec<CapturedHttpRequest> {
        self.server.abort();
        std::mem::take(&mut *self.requests.lock().assert_value())
    }
}

fn routes(requests: &[CapturedHttpRequest]) -> Vec<(&str, &str)> {
    requests
        .iter()
        .map(|request| (request.method.as_str(), request.path.as_str()))
        .collect()
}

fn unexpected() -> (&'static str, String) {
    (
        "404 Not Found",
        json!({"code": "not_found", "message": "unexpected request"}).to_string(),
    )
}

fn direct_without_marker(
    request: &CapturedHttpRequest,
    _origin: &str,
    _token_index: &mut u8,
) -> (&'static str, String) {
    match (request.method.as_str(), request.path.as_str()) {
        DISCOVERY => (
            "200 OK",
            serde_json::to_string(&TargetDiscoveryDocument::direct(TargetAuthentication::None))
                .assert_value(),
        ),
        RUN => ("200 OK", RUN_RECEIPT.to_owned()),
        _ => unexpected(),
    }
}

fn hosted_response(
    request: &CapturedHttpRequest,
    origin: &str,
    token_index: &mut u8,
    advertises_lanes: bool,
) -> (&'static str, String) {
    match (request.method.as_str(), request.path.as_str()) {
        DISCOVERY => ("200 OK", hosted_lane_discovery(origin, advertises_lanes)),
        OAUTH_METADATA => ("200 OK", oauth_metadata(origin)),
        TOKEN => ("200 OK", token_response(token_index)),
        SESSION => (
            "200 OK",
            json!({
                "kind": "openengine.target-session/v1",
                "organization_id": "organization-1",
            })
            .to_string(),
        ),
        RUN => ("200 OK", RUN_RECEIPT.to_owned()),
        PROFILE_SET => ("200 OK", stored_profile(&request.body)),
        _ => unexpected(),
    }
}

fn hosted_lane_discovery(origin: &str, advertises_lanes: bool) -> String {
    let mut document: Value = serde_json::from_str(&hosted_discovery(origin)).assert_value();
    if advertises_lanes {
        document["extensions"]["node_runtime_lanes"] = json!({"kind": NODE_RUNTIME_LANES_KIND});
    }
    document.to_string()
}

fn stored_profile(body: &str) -> String {
    let request: Value = serde_json::from_str(body).assert_value();
    json!({
        "profile": {
            "id": "profile-lanes",
            "name": request["name"],
            "scope": request["scope"],
            "graph": request["graph"],
            "runtime": request["runtime"],
            "isDefault": false
        }
    })
    .to_string()
}

fn lane_binding() -> Value {
    json!({
        "kind": "agent",
        "lane": {"harness": "claude", "provider": "anthropic"},
        "model": "opaque-model"
    })
}

fn lane_free_binding() -> Value {
    json!({"kind": "agent", "model": "opaque-model"})
}

fn run_request(worker: Value) -> TargetRunRequest {
    let mut request = exact_run_request();
    request.submission.runtime.nodes_mut().insert(
        NodeName::new("worker").assert_value(),
        serde_json::from_value(worker).assert_value(),
    );
    request
}

fn profile_request(worker: Value) -> RunProfileSetRequest {
    RunProfileSetRequest {
        name: RunProfileName::new("lanes").assert_value(),
        scope: RunProfileScope::User,
        graph: graph_fixture("worker", json!({"kind": "null"})),
        runtime: run_request(worker).submission.runtime,
        set_default: false,
    }
}

fn sent_lane(body: &str, pointer: &str) -> Value {
    let body: Value = serde_json::from_str(body).assert_value();
    body.pointer(pointer).cloned().assert_value()
}

#[tokio::test]
async fn direct_target_without_the_marker_refuses_a_lane_after_only_discovery() {
    let root = temp_root();
    let target = RecordingTarget::spawn(direct_without_marker).await;
    let authority = test_http_authority(root.path("refresh-locks"));
    let authority: &dyn TargetControlAuthority = &authority;

    let error = authority
        .submit(
            &direct_target(target.origin.clone()),
            &run_request(lane_binding()),
        )
        .await
        .assert_error();

    assert_eq!(error.to_string(), REFUSAL);
    assert_eq!(routes(&target.stop()), [DISCOVERY]);
}

#[tokio::test]
async fn direct_target_with_the_marker_accepts_a_lane() {
    let root = temp_root();
    let (origin, server) = spawn_direct_target_authority(2).await;
    let authority = test_http_authority(root.path("refresh-locks"));
    let authority: &dyn TargetControlAuthority = &authority;
    let request = run_request(lane_binding());

    let receipt = authority
        .submit(&direct_target(origin), &request)
        .await
        .assert_value();

    assert_eq!(receipt.run_id, request.run_id);
    let requests = server.await.assert_value();
    assert_eq!(routes(&requests), [DISCOVERY, RUN]);
    let run = requests.last().assert_value();
    assert_eq!(
        sent_lane(&run.body, "/submission/runtime/nodes/worker/lane"),
        json!({"harness": "claude", "provider": "anthropic"})
    );
}

#[tokio::test]
async fn direct_target_without_the_marker_accepts_a_plan_without_lanes() {
    let root = temp_root();
    let target = RecordingTarget::spawn(direct_without_marker).await;
    let authority = test_http_authority(root.path("refresh-locks"));
    let authority: &dyn TargetControlAuthority = &authority;

    authority
        .submit(
            &direct_target(target.origin.clone()),
            &run_request(lane_free_binding()),
        )
        .await
        .assert_value();

    assert_eq!(routes(&target.stop()), [DISCOVERY, RUN]);
}

#[tokio::test]
async fn hosted_target_without_the_marker_refuses_a_lane_submission_before_the_token() {
    let root = temp_root();
    let target = RecordingTarget::spawn(|request, origin, tokens| {
        hosted_response(request, origin, tokens, false)
    })
    .await;
    let (credentials, authority) = test_authority(&root);
    let record = hosted_target("prod", target.origin.clone());
    credentials
        .set(&record.id, "refresh-0")
        .await
        .assert_value();
    let authority: &dyn TargetControlAuthority = &authority;

    let error = authority
        .submit(&record, &run_request(lane_binding()))
        .await
        .assert_error();

    assert_eq!(error.to_string(), REFUSAL);
    assert_eq!(routes(&target.stop()), [DISCOVERY, OAUTH_METADATA]);
}

#[tokio::test]
async fn hosted_target_without_the_marker_refuses_a_lane_profile_before_the_token() {
    let root = temp_root();
    let target = RecordingTarget::spawn(|request, origin, tokens| {
        hosted_response(request, origin, tokens, false)
    })
    .await;
    let (credentials, authority) = test_authority(&root);
    let record = hosted_target("prod", target.origin.clone());
    credentials
        .set(&record.id, "refresh-0")
        .await
        .assert_value();
    let authority: &dyn TargetControlAuthority = &authority;

    let error = authority
        .profile_set(&record, profile_request(lane_binding()))
        .await
        .assert_error();

    assert_eq!(error.to_string(), REFUSAL);
    assert_eq!(routes(&target.stop()), [DISCOVERY, OAUTH_METADATA]);
}

#[tokio::test]
async fn hosted_target_with_the_marker_accepts_a_lane_submission_and_profile() {
    let root = temp_root();
    let target = RecordingTarget::spawn(|request, origin, tokens| {
        hosted_response(request, origin, tokens, true)
    })
    .await;
    let (credentials, authority) = test_authority(&root);
    let record = hosted_target("prod", target.origin.clone());
    credentials
        .set(&record.id, "refresh-0")
        .await
        .assert_value();
    let authority: &dyn TargetControlAuthority = &authority;

    authority
        .submit(&record, &run_request(lane_binding()))
        .await
        .assert_value();
    let stored = authority
        .profile_set(&record, profile_request(lane_binding()))
        .await
        .assert_value();

    assert!(stored.profile.runtime.has_lane_overrides());
    let requests = target.stop();
    assert_eq!(
        routes(&requests),
        [
            DISCOVERY,
            OAUTH_METADATA,
            TOKEN,
            SESSION,
            RUN,
            DISCOVERY,
            OAUTH_METADATA,
            PROFILE_SET
        ]
    );
    let lane = json!({"harness": "claude", "provider": "anthropic"});
    let run = requests.get(4).assert_value();
    assert_eq!(
        sent_lane(&run.body, "/submission/runtime/nodes/worker/lane"),
        lane
    );
    let profile = requests.last().assert_value();
    assert_eq!(sent_lane(&profile.body, "/runtime/nodes/worker/lane"), lane);
}
