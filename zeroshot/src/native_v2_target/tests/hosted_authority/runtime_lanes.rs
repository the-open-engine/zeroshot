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

#[derive(Clone, Copy)]
enum HostedCapability {
    Lanes(bool),
    Git(Option<&'static str>),
}

async fn hosted_authority_with(
    capability: HostedCapability,
) -> (
    super::super::fixtures::TempRoot,
    RecordingTarget,
    crate::native_v2_target::TargetRecord,
    crate::native_v2_target::TargetHttpControlAuthority,
) {
    let root = temp_root();
    let target = RecordingTarget::spawn(move |request, origin, tokens| match capability {
        HostedCapability::Lanes(advertised) => hosted_response(request, origin, tokens, advertised),
        HostedCapability::Git(marker) => git_hosted_response(request, origin, tokens, marker),
    })
    .await;
    let (record, authority) = authenticated_authority(&root, &target.origin).await;
    (root, target, record, authority)
}

async fn authenticated_authority(
    root: &super::super::fixtures::TempRoot,
    origin: &str,
) -> (
    crate::native_v2_target::TargetRecord,
    crate::native_v2_target::TargetHttpControlAuthority,
) {
    let (credentials, authority) = test_authority(root);
    let record = hosted_target("prod", origin.to_owned());
    credentials
        .set(&record.id, "refresh-0")
        .await
        .assert_value();
    (record, authority)
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
    let (_root, target, record, authority) =
        hosted_authority_with(HostedCapability::Lanes(false)).await;
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
    let (_root, target, record, authority) =
        hosted_authority_with(HostedCapability::Lanes(false)).await;
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
    let (_root, target, record, authority) =
        hosted_authority_with(HostedCapability::Lanes(true)).await;
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

fn generic_git_request(remote: bool, credentials: bool) -> TargetRunRequest {
    let mut request = run_request(lane_free_binding());
    if remote {
        request.submission.source = Some(
            serde_json::from_value(json!({
                "repository":"nested/team/project:42", "branch":"main", "revision":"a".repeat(64),
                "remote":{"url":"git@git.example:nested/team/project.git"}
            }))
            .assert_value(),
        );
    }
    if credentials {
        request.git_credentials = Some(
            openengine_cluster_protocol::GitTransportCredentials::Https {
                username: "private-user".to_owned(),
                password: "transport-secret".to_owned(),
            },
        );
    }
    request
}

fn git_hosted_response(
    request: &CapturedHttpRequest,
    origin: &str,
    tokens: &mut u8,
    marker: Option<&str>,
) -> (&'static str, String) {
    if (request.method.as_str(), request.path.as_str()) == DISCOVERY {
        let mut wire: Value =
            serde_json::from_str(&hosted_lane_discovery(origin, true)).assert_value();
        if let Some(kind) = marker {
            wire["extensions"]["git_transport"] = json!({"kind":kind});
        }
        return ("200 OK", wire.to_string());
    }
    hosted_response(request, origin, tokens, true)
}

#[tokio::test]
async fn old_direct_target_refuses_generic_source_or_credentials_after_only_discovery() {
    for (remote, credentials) in [(true, false), (false, true)] {
        let root = temp_root();
        let target = RecordingTarget::spawn(direct_without_marker).await;
        let authority = test_http_authority(root.path("refresh-locks"));
        let error = authority
            .submit(
                &direct_target(target.origin.clone()),
                &generic_git_request(remote, credentials),
            )
            .await
            .assert_error();
        assert_eq!(
            error.to_string(),
            "target does not support generic Git transport (openengine.git-transport/v1)"
        );
        let requests = target.stop();
        assert_eq!(routes(&requests), [DISCOVERY]);
        assert!(requests.iter().all(|request| request.body.is_empty()));
    }
}

#[tokio::test]
async fn new_direct_target_transmits_authored_ssh_remote_and_exact_sha256_source() {
    let root = temp_root();
    let target = RecordingTarget::spawn(|request, _, _| {
        match (request.method.as_str(), request.path.as_str()) {
            DISCOVERY => (
                "200 OK",
                serde_json::to_string(
                    &TargetDiscoveryDocument::direct(TargetAuthentication::None)
                        .with_git_transport(),
                )
                .assert_value(),
            ),
            RUN => ("200 OK", RUN_RECEIPT.to_owned()),
            _ => unexpected(),
        }
    })
    .await;
    let authority = test_http_authority(root.path("refresh-locks"));
    let request = generic_git_request(true, false);
    authority
        .submit(&direct_target(target.origin.clone()), &request)
        .await
        .assert_value();
    let requests = target.stop();
    assert_eq!(routes(&requests), [DISCOVERY, RUN]);
    let body: Value = serde_json::from_str(&requests.last().assert_value().body).assert_value();
    assert_eq!(
        body["submission"]["source"]["remote"]["url"],
        "git@git.example:nested/team/project.git"
    );
    assert_eq!(body["submission"]["source"]["revision"], "a".repeat(64));
    assert!(body.get("gitCredentials").is_none());
}

#[tokio::test]
async fn old_hosted_target_rejects_new_git_forms_before_access_token_or_body() {
    for (remote, credentials) in [(true, false), (false, true)] {
        let (_root, target, record, authority) =
            hosted_authority_with(HostedCapability::Git(None)).await;
        let error = authority
            .submit(&record, &generic_git_request(remote, credentials))
            .await
            .assert_error();
        assert_eq!(
            error.to_string(),
            "target does not support generic Git transport (openengine.git-transport/v1)"
        );
        assert_eq!(routes(&target.stop()), [DISCOVERY, OAUTH_METADATA]);
    }
}

#[tokio::test]
async fn another_git_marker_kind_does_not_authorize_the_new_wire_shape() {
    let (_root, target, record, authority) =
        hosted_authority_with(HostedCapability::Git(Some("openengine.git-transport/v2"))).await;
    authority
        .submit(&record, &generic_git_request(true, false))
        .await
        .assert_error();
    assert_eq!(routes(&target.stop()), [DISCOVERY, OAUTH_METADATA]);
}

#[tokio::test]
async fn old_hosted_target_refuses_profile_git_source_and_resume_credentials_before_auth() {
    let (_root, target, record, authority) =
        hosted_authority_with(HostedCapability::Git(None)).await;
    let request = generic_git_request(true, false);
    let profile = openengine_cluster_protocol::RunProfileRunRequest {
        run_id: request.run_id,
        profile: openengine_cluster_protocol::RunProfileSelector {
            scope: RunProfileScope::User,
            name: RunProfileName::new("generic").assert_value(),
        },
        title: request.submission.title,
        initial_input: request.submission.initial_input,
        source: request.submission.source.assert_value(),
        submission_key: request.submission.submission_key,
        environment: None,
        connections: request.connections,
        github_token: None,
        git_credentials: None,
    };
    authority
        .profile_run(&record, &profile)
        .await
        .assert_error();
    let resume: openengine_cluster_protocol::RunResumeParams = serde_json::from_value(json!({
        "runId":"018f5e78-7f95-7c22-8d98-3f15af20c991",
        "successorRunId":"018f5e78-7f95-7c22-8d98-3f15af20c992",
        "gitCredentials":{"kind":"ssh","privateKey":"private-key-secret","knownHosts":"known-host-secret"}
    })).assert_value();
    authority
        .hosted_run_resume(&record, resume)
        .await
        .assert_error();
    assert_eq!(
        routes(&target.stop()),
        [DISCOVERY, OAUTH_METADATA, DISCOVERY, OAUTH_METADATA]
    );
}

#[tokio::test]
async fn supported_hosted_target_transmits_private_credentials_only_in_the_envelope() {
    let (_root, target, record, authority) = hosted_authority_with(HostedCapability::Git(Some(
        openengine_cluster_protocol::GIT_TRANSPORT_KIND,
    )))
    .await;
    let mut request = generic_git_request(true, true);
    request
        .submission
        .source
        .as_mut()
        .assert_value()
        .remote
        .as_mut()
        .assert_value()
        .url = openengine_cluster_protocol::GitRemoteUrl::new(
        "https://git.example/nested/team/project.git",
    )
    .assert_value();
    authority.submit(&record, &request).await.assert_value();
    let requests = target.stop();
    assert_eq!(
        routes(&requests),
        [DISCOVERY, OAUTH_METADATA, TOKEN, SESSION, RUN]
    );
    let body: Value = serde_json::from_str(&requests.last().assert_value().body).assert_value();
    assert_eq!(body["gitCredentials"]["password"], "transport-secret");
    assert!(!body["submission"].to_string().contains("transport-secret"));
    assert!(!body["submission"].to_string().contains("private-user"));
}

#[path = "runtime_lanes/review.rs"]
mod review;
