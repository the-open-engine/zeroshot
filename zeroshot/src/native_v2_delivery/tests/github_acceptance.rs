use super::*;
use crate::source_code_provider::{
    TestReviewCheckpoint,
    adapters::core::github::test_policy_page,
    adapters::core::http_fixture::{self, IntoResponse, Json, Request, Response},
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use reqwest::{Method, StatusCode};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MERGED: &str = "cccccccccccccccccccccccccccccccccccccccc";
const REVIEW_BODY: &str = concat!(
    "<!-- zeroshot-delivery:generated:v1:start -->\n",
    "Repair the checkout flow.\n\nCloses #208\n",
    "<!-- zeroshot-delivery:generated:v1:end -->"
);

fn review_request(source_issue: Option<u64>) -> SourceReviewRequest {
    SourceReviewRequest {
        include_terminal_reviews: true,
        known_review: None,
        target: DeliveryTarget::new("acme/project", "main", BASE).unwrap(),
        head_branch: "zeroshot/v2-test".into(),
        head_revision: HEAD.to_owned(),
        title: "fix: repair checkout".into(),
        description: "Repair the checkout flow.".into(),
        source_issue: source_issue.map(|number| SourceDeliveryIssue { number }),
    }
}

struct Server {
    task: tokio::task::JoinHandle<()>,
    address: String,
}

impl Server {
    async fn start(handler: impl Fn(Request) -> Response + Send + Sync + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/", listener.local_addr().unwrap());
        Self {
            task: tokio::spawn(http_fixture::serve(listener, handler)),
            address,
        }
    }

    fn authority(&self, home: &Path, git_program: PathBuf) -> GhCliDeliveryAuthority {
        GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            git_program,
            ..GhCliAuthorityConfig::hosted(home.to_owned())
        })
        .with_test_api(&self.address)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn review_wire() -> Value {
    json!({
        "number":17,"title":"fix: repair checkout","body":REVIEW_BODY,
        "state":"open","merged":false,"merge_commit_sha":null,
        "base":{"ref":"main","sha":BASE,"repo":{"full_name":"acme/project"}},
        "head":{"ref":"zeroshot/v2-test","sha":HEAD,"repo":{"full_name":"acme/project"}}
    })
}

fn policy(merged: bool) -> Value {
    let mut page = test_policy_page("MERGEABLE", "CLEAN", None, (false, None));
    let pull = &mut page["data"]["repository"]["pullRequest"];
    pull["headRefName"] = json!("zeroshot/v2-test");
    if merged {
        pull["state"] = json!("MERGED");
        pull["merged"] = json!(true);
        pull["mergeCommit"] = json!({ "oid": MERGED });
    }
    page
}

fn forge_reply(request: Request, plan_unavailable: bool, merged: &AtomicBool) -> Response {
    let path = request.uri.path();
    assert_eq!(request.headers["authorization"], "Bearer test-token");
    assert_eq!(request.headers["x-github-api-version"], "2026-03-10");
    assert!(!request.uri.as_str().contains("test-token"));
    if request.method == Method::GET {
        return read_forge(&request, plan_unavailable);
    }
    assert_eq!(request.method, Method::POST);
    let body = serde_json::from_slice::<Value>(&request.body).unwrap();
    let value = match path {
        "/repos/acme/project/pulls" => {
            assert_eq!(
                body,
                json!({"title":"fix: repair checkout","body":REVIEW_BODY,"head":"zeroshot/v2-test","base":"main"})
            );
            review_wire()
        }
        "/repos/acme/project/issues/208/comments" => {
            assert_eq!(
                body,
                json!({"body":"Zeroshot opened pull request #17 for this issue.\n\n<!-- zeroshot-delivery:zeroshot/v2-test -->"})
            );
            json!({"id":71})
        }
        "/graphql" => graphql_reply(&body, merged),
        _ => panic!("unexpected POST {path}"),
    };
    Json(value).into_response()
}

fn read_forge(request: &Request, plan_unavailable: bool) -> Response {
    let value = match request.uri.path() {
        "/repos/acme/project/git/ref/heads/zeroshot/v2-test" => json!({
            "ref":"refs/heads/zeroshot/v2-test","object":{"sha":HEAD,"type":"commit"}
        }),
        "/repos/acme/project/pulls" => {
            assert!(
                request
                    .uri
                    .query_pairs()
                    .any(|(key, value)| key == "state" && value == "all")
            );
            assert!(
                request
                    .uri
                    .query_pairs()
                    .any(|(key, value)| key == "head" && value == "acme:zeroshot/v2-test")
            );
            json!([])
        }
        "/repos/acme/project/pulls/17" => review_wire(),
        "/repos/acme/project/issues/208" => json!({"comments":0}),
        "/repos/acme/project/issues/208/comments" => {
            assert!(
                request
                    .uri
                    .query_pairs()
                    .any(|(key, value)| key == "page" && value == "1")
            );
            json!([])
        }
        "/repos/acme/project/rules/branches/main" if plan_unavailable => {
            return (StatusCode::FORBIDDEN, Json(json!({
                "message":"Upgrade to GitHub Pro or make this repository public to enable this feature."
            }))).into_response();
        }
        "/repos/acme/project/rules/branches/main" => json!([]),
        path => panic!("unexpected GET {path}"),
    };
    Json(value).into_response()
}

fn graphql_reply(body: &Value, merged: &AtomicBool) -> Value {
    let query = body["query"].as_str().unwrap();
    if query.contains("mergePullRequest") {
        assert_eq!(
            body["variables"],
            json!({
                "pullRequestId":"PR_node_17","expectedHeadOid":HEAD,"method":"MERGE"
            })
        );
        assert!(!merged.swap(true, Ordering::SeqCst));
        json!({"data":{"mergePullRequest":{"pullRequest":{
            "id":"PR_node_17","number":17,"repository":{"nameWithOwner":"acme/project"},
            "baseRefName":"main","headRefName":"zeroshot/v2-test","headRefOid":HEAD,
            "merged":true,"mergeCommit":{"oid":MERGED}
        }}}})
    } else {
        assert_eq!(body["variables"]["number"], 17);
        if query.contains("isRequired(pullRequestNumber: $number)") {
            assert_eq!(body["variables"]["head"], HEAD);
            assert_eq!(body["variables"]["branch"], "main");
        } else {
            assert!(query.contains("query MergePolicy"));
        }
        policy(merged.load(Ordering::SeqCst))
    }
}

#[tokio::test]
async fn native_forge_routes_and_git_transport_preserve_exact_authority() {
    for plan_unavailable in [false, true] {
        let repo = TempRepo::delivery();
        let git = write_executable(repo.root.path(), "git-script", GIT_SCRIPT);
        let reads = Arc::new(Mutex::new(Vec::new()));
        let received = reads.clone();
        let merged = AtomicBool::new(false);
        let server = Server::start(move |request| {
            received.lock().unwrap().push(request.uri.path().to_owned());
            forge_reply(request, plan_unavailable, &merged)
        })
        .await;
        let authority = server.authority(repo.root.path(), git.clone());
        let request = review_request(Some(208));
        let credential = SourceDeliveryCredential::token("test-token");
        authority
            .push_branch(
                &SourcePushRequest {
                    workspace: repo.workspace.clone(),
                    target: request.target.clone(),
                    head_branch: request.head_branch.clone(),
                    head_revision: request.head_revision.clone(),
                },
                credential,
            )
            .await
            .assert_value();
        let review = authority
            .open_or_update_review(&request, credential)
            .await
            .assert_value();
        let observation = authority
            .inspect_review(&review, credential)
            .await
            .assert_value();
        assert_eq!(
            observation.state,
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired
            }
        );
        assert_eq!(
            authority
                .request_merge(&review, credential, &TestReviewCheckpoint)
                .await
                .assert_value(),
            SourceMergeRequestOutcome::Accepted,
        );
        assert_eq!(
            authority
                .inspect_review(&review, credential)
                .await
                .assert_value()
                .state,
            SourceReviewState::Merged {
                merge_revision: MERGED.to_owned(),
            }
        );

        let captured = fs::read_to_string(format!("{}.capture", git.display())).assert_value();
        assert!(captured.contains(&format!("home={}", repo.root.path().display())));
        assert!(captured.contains("config_count=4"));
        assert!(
            captured.contains("config_key_1=http.https://github.com/acme/project.git.extraheader")
        );
        assert!(captured.contains("config_value_1=\n"));
        assert!(
            captured.contains("config_key_2=http.https://github.com/acme/project.git.extraheader")
        );
        assert!(captured.contains(&format!(
            "config_value_2=AUTHORIZATION: basic {}",
            crate::native_v2_delivery::git_auth::encode_basic_credential("test-token")
        )));
        assert!(captured.contains("arg=https://github.com/acme/project.git"));
        assert!(captured.contains(&format!("arg={HEAD}:refs/heads/zeroshot/v2-test")));
        assert!(!argument_lines(&captured).contains("test-token"));
        assert!(
            reads
                .lock()
                .unwrap()
                .iter()
                .any(|path| path == "/repos/acme/project/issues/208/comments")
        );
    }
}

#[tokio::test]
async fn native_http_preserves_safe_validation_details_and_request_id() {
    let server = Server::start(|_| {
        (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({
            "message":"Validation Failed", "errors":[{"field":"head","message":"Head sha can't be blank test-token"}]
        }))).into_response().header("x-github-request-id","safe-request-id")
    }).await;
    let error = refused_review(&server).await;
    assert_eq!(error.api_status(), Some(422));
    let message = error.to_string();
    assert!(message.contains("Head sha can't be blank"));
    assert!(message.contains("HTTP 422") && message.contains("safe-request-id"));
    assert!(error.retryable_review_sync());
    assert!(!message.contains("test-token"));
}

#[tokio::test]
async fn native_http_refuses_wrong_pr_identity_or_invalid_json() {
    for invalid_json in [true, false] {
        let server = Server::start(move |_| {
            if invalid_json {
                Response::raw(b"{invalid}".to_vec())
            } else {
                let mut review = review_wire();
                review["base"]["ref"] = json!("other");
                Json(json!([review])).into_response()
            }
        })
        .await;
        let error = refused_review(&server).await;
        assert!(matches!(
            error,
            SourceDeliveryError::Api(_) | SourceDeliveryError::Identity(_)
        ));
        assert!(!error.retryable_operation());
    }
}

async fn refused_review(server: &Server) -> SourceDeliveryError {
    server
        .authority(Path::new("/fixture"), "/usr/bin/git".into())
        .open_or_update_review(
            &review_request(None),
            SourceDeliveryCredential::token("test-token"),
        )
        .await
        .assert_error()
}
