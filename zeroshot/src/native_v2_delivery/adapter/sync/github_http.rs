use std::sync::Mutex;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::native_v2_runner::test_support;
use crate::source_code_provider::{
    SourceReviewReceipt,
    adapters::core::{
        GhCliAuthorityConfig, GhCliDeliveryAuthority,
        http_fixture::{self, IntoResponse, Json, Request, Response},
    },
};

use super::*;

const REPO: &str = "acme/project";
const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Clone, Copy)]
enum Fault {
    Lookup,
    AppliedPatch,
}

struct RemoteReview {
    wire: Value,
    requests: Vec<Method>,
    fault: Option<Fault>,
}

impl RemoteReview {
    fn receive(&mut self, request: Request) -> Response {
        assert_eq!(request.uri.path(), "/repos/acme/project/pulls/17");
        assert_eq!(request.headers["authorization"], "Bearer test-github-token");
        self.requests.push(request.method.clone());
        let fail = match request.method {
            Method::GET => matches!(self.fault, Some(Fault::Lookup)),
            Method::PATCH => {
                let patch: Value = serde_json::from_slice(&request.body).unwrap();
                self.wire["title"] = patch["title"].clone();
                self.wire["body"] = patch["body"].clone();
                matches!(self.fault, Some(Fault::AppliedPatch))
            }
            _ => panic!("synchronization issued an unexpected write"),
        };
        if fail {
            self.fault = None;
            return Response::raw(b"{".to_vec())
                .status(StatusCode::OK)
                .content_length(500);
        }
        Json(&self.wire).into_response()
    }
}

async fn sync_after_broken_reply(fault: Fault) -> Vec<Method> {
    let review = Arc::new(Mutex::new(RemoteReview {
        wire: json!({
            "number":17, "title":"Original", "body":"Human note",
            "head":{"ref":"zeroshot/test","sha":HEAD,"repo":{"full_name":REPO}},
            "base":{"ref":"main","sha":BASE,"repo":{"full_name":REPO}},
        }),
        requests: Vec::new(),
        fault: Some(fault),
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/", listener.local_addr().unwrap());
    let remote = review.clone();
    let server = tokio::spawn(http_fixture::serve(listener, move |request| {
        remote.lock().unwrap().receive(request)
    }));
    let workspace = tempfile::tempdir().unwrap();
    let authority =
        GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted(workspace.path().to_owned()))
            .with_test_api(&address);
    let target = DeliveryTarget::new(REPO, "main", BASE).unwrap();
    let mut request = SourceReviewRequest {
        target: target.clone(),
        head_branch: "zeroshot/test".into(),
        head_revision: HEAD.into(),
        title: "fix: confirm delivered metadata".into(),
        description: "Review this change.".into(),
        source_issue: None,
        known_review: None,
        include_terminal_reviews: true,
    };
    request.known_review = Some(SourceReviewReceipt::for_request(&request, "17"));
    let adapter = NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            local_branch: None,
            delivery_run_id: openengine_cluster_protocol::RunId::new("native-github-sync"),
            adopt_existing_delivery: false,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: workspace.path().to_owned(),
            git_program: PathBuf::from("unused-git"),
            target,
            poll: DeliveryPollPolicy::new(1, Duration::ZERO).unwrap(),
        },
        Arc::new(authority),
    );
    let mut credentials = DeliveryCredentials {
        environment: None,
        token_file: None,
        token: "test-github-token".to_owned(),
        git: None,
        forge_field: Some("GH_TOKEN"),
        github_git_token: String::new(),
        token_from_environment: false,
        diagnostic_secrets: Vec::new(),
    };
    let (_cancel, control, _output) = test_support::control_with_capacity(16);
    match adapter
        .synchronize_review(&request, &mut credentials, &control)
        .await
    {
        Ok(receipt) => assert_eq!(receipt, request.known_review.unwrap()),
        Err(failure) => panic!("sync failed: {:?}", failure.diagnostic()),
    }
    server.abort();
    let review = review.lock().unwrap();
    assert_eq!(review.wire["title"], request.title);
    assert!(review.wire["body"].as_str().unwrap().contains("Human note"));
    review.requests.clone()
}

#[tokio::test]
async fn interrupted_lookup_restarts_sync_before_edit() {
    assert_eq!(
        sync_after_broken_reply(Fault::Lookup).await,
        [Method::GET, Method::GET, Method::GET, Method::PATCH],
    );
}

#[tokio::test]
async fn applied_patch_with_lost_reply_is_observed_without_a_second_patch() {
    assert_eq!(
        sync_after_broken_reply(Fault::AppliedPatch).await,
        [
            Method::GET,
            Method::GET,
            Method::PATCH,
            Method::GET,
            Method::GET
        ],
    );
}
