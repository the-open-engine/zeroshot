//! A named-target run submission whose reply is lost must not start a second run.
//!
//! The stub target follows the shared target idempotency contract. The first request with a
//! submission key admits a run. A later request with the same key and submission gets that run
//! back, and a new key admits another run. Like a proxy or VPN that drops the response, the stub
//! discards a configured number of replies after it has admitted the submission.
//! The CLI's replay budget is bounded, so the tests also count every submission it sends.

use std::sync::{Arc, Mutex};

use openengine_cluster_protocol::{
    RunProfileName, RunProfileScope, RunProfileSelector, TargetAuthentication,
    TargetDiscoveryDocument,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use zeroshot_engine::native_v2_cli::oecp::NamedTargetCliBackend;
use zeroshot_engine::native_v2_cli::{NativeV2CliBackend, PreparedRunRequest};

use super::super::super::controller_authority::TargetCredentialStore;
use super::super::super::{
    NativeV2TargetConnector, TargetHttpControlAuthority, TargetRecord, TargetRegistry,
};
use super::super::fixtures::{
    FakeDialer, MemoryRegistry, TempRoot, direct_target, hosted_target, run_request, temp_root,
};
use super::{
    CapturedHttpRequest, hosted_discovery, oauth_metadata, read_http_request, test_authority,
    token_response, write_http_response_with_status,
};

/// The submission key carried by the shared `run_request` fixture.
const SUBMISSION_KEY: &str = "target-test";
const FORGE_SECRET: &str = "ghs_lost_reply_forge_secret";
const SERVER_DOWN_HINT: &str = "that the server is running";
/// The first submission and its two identical replays.
const SUBMISSION_ATTEMPTS: usize = 3;

type Backend = NamedTargetCliBackend<
    NativeV2TargetConnector<MemoryRegistry, TargetHttpControlAuthority, FakeDialer>,
>;

#[derive(Clone, Copy, Debug)]
enum Submission {
    Direct,
    Hosted,
    HostedProfile,
}

impl Submission {
    fn target(self, origin: &str) -> TargetRecord {
        match self {
            Self::Direct => direct_target(origin),
            Self::Hosted | Self::HostedProfile => hosted_target("prod", origin),
        }
    }

    fn request(self) -> PreparedRunRequest {
        let mut request = run_request();
        if matches!(self, Self::HostedProfile) {
            request.profile = Some(RunProfileSelector {
                scope: RunProfileScope::User,
                name: RunProfileName::new("contract").assert_value(),
            });
        }
        request
    }

    const fn route(self) -> &'static str {
        match self {
            Self::Direct | Self::Hosted => "/native-v2/run",
            Self::HostedProfile => "/native-v2/profiles/run",
        }
    }
}

/// What the stub answers once it stops discarding replies.
#[derive(Clone, Copy)]
enum Answer {
    /// The receipt the idempotency contract selects.
    Receipt,
    /// A temporary refusal, as Cloud answers a replay while the first submission is in progress.
    Unavailable,
}

struct Admission {
    key: String,
    identity: Value,
    run_id: String,
}

#[derive(Default)]
struct TargetState {
    admissions: Vec<Admission>,
    submissions: usize,
    unexpected: Vec<String>,
    tokens: u8,
}

impl TargetState {
    /// Applies the target idempotency contract. The caller-assigned run ID is not identity.
    fn submit(&mut self, body: &str) -> (&'static str, String) {
        let Some((key, identity, run_id)) = submission_identity(body) else {
            self.unexpected
                .push(format!("malformed submission: {body}"));
            return problem("400 Bad Request", "invalid_request");
        };
        match self
            .admissions
            .iter()
            .find(|admission| admission.key == key)
        {
            Some(admission) if admission.identity == identity => {
                ("200 OK", json!({"runId": admission.run_id}).to_string())
            }
            Some(_) => problem("409 Conflict", "IDEMPOTENCY_REUSE"),
            None => {
                let receipt = json!({"runId": run_id}).to_string();
                self.admissions.push(Admission {
                    key,
                    identity,
                    run_id,
                });
                ("200 OK", receipt)
            }
        }
    }
}

fn submission_identity(body: &str) -> Option<(String, Value, String)> {
    let mut identity: Value = serde_json::from_str(body).ok()?;
    let run_id = identity
        .as_object_mut()?
        .remove("runId")?
        .as_str()?
        .to_owned();
    let key = identity
        .pointer("/submission/submissionKey")
        .or_else(|| identity.get("submissionKey"))?
        .as_str()?
        .to_owned();
    Some((key, identity, run_id))
}

fn problem(status: &'static str, code: &str) -> (&'static str, String) {
    (
        status,
        json!({"code": code, "message": "stub target refused the request"}).to_string(),
    )
}

struct StubTarget {
    origin: String,
    submission: Submission,
    lost_replies: usize,
    answer: Answer,
    state: Mutex<TargetState>,
}

impl StubTarget {
    async fn serve(self: Arc<Self>, mut stream: TcpStream) {
        let request = read_http_request(&mut stream).await;
        if let Some((status, body)) = self.respond(&request) {
            write_http_response_with_status(&mut stream, status, &body).await;
        }
        // Without a reply the stream closes here: the run exists, but the caller never learns it.
    }

    fn respond(&self, request: &CapturedHttpRequest) -> Option<(&'static str, String)> {
        let mut state = self.state.lock().assert_value();
        let response = match (request.method.as_str(), request.path.as_str()) {
            ("POST", path) if path == self.submission.route() => {
                let receipt = state.submit(&request.body);
                let response = match self.answer {
                    Answer::Receipt => receipt,
                    Answer::Unavailable => {
                        problem("503 Service Unavailable", "temporarily_unavailable")
                    }
                };
                state.submissions += 1;
                return (state.submissions > self.lost_replies).then_some(response);
            }
            ("GET", "/.well-known/zeroshot-native-v2") => match self.submission {
                Submission::Direct => serde_json::to_string(&TargetDiscoveryDocument::direct(
                    TargetAuthentication::None,
                ))
                .assert_value(),
                Submission::Hosted | Submission::HostedProfile => hosted_discovery(&self.origin),
            },
            ("GET", "/oauth/metadata") => oauth_metadata(&self.origin),
            ("POST", "/oauth/token") => token_response(&mut state.tokens),
            ("GET", "/session") => json!({
                "kind": "openengine.target-session/v1",
                "organization_id": "organization-1",
            })
            .to_string(),
            (method, path) => {
                state.unexpected.push(format!("{method} {path}"));
                return Some(problem("404 Not Found", "not_found"));
            }
        };
        Some(("200 OK", response))
    }
}

struct LossyTarget {
    stub: Arc<StubTarget>,
    server: tokio::task::JoinHandle<()>,
}

impl LossyTarget {
    async fn spawn(submission: Submission, lost_replies: usize, answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
        let stub = Arc::new(StubTarget {
            origin: format!("http://{}", listener.local_addr().assert_value()),
            submission,
            lost_replies,
            answer,
            state: Mutex::default(),
        });
        let served = stub.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(served.clone().serve(stream));
            }
        });
        Self { stub, server }
    }

    fn origin(&self) -> &str {
        &self.stub.origin
    }

    fn stop(self) -> TargetState {
        self.server.abort();
        std::mem::take(&mut *self.stub.state.lock().assert_value())
    }
}

async fn backend(root: &TempRoot, target: TargetRecord) -> Backend {
    let (credentials, authority) = test_authority(root);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    let registry = MemoryRegistry::default();
    registry.insert(target).assert_value();
    NamedTargetCliBackend::new(NativeV2TargetConnector::new(
        registry,
        authority,
        FakeDialer::default(),
    ))
}

async fn assert_lost_reply_returns_the_admitted_run(submission: Submission, lost_replies: usize) {
    let root = temp_root();
    let stub = LossyTarget::spawn(submission, lost_replies, Answer::Receipt).await;
    let target = submission.target(stub.origin());
    let name = target.name.clone();
    let backend = backend(&root, target).await;
    let request = submission.request();
    let run_id = request.run_id.clone();

    let receipt = backend.run_submit(Some(&name), request).await;
    let state = stub.stop();

    let context = format!(
        "{submission:?}: the target admitted run {run_id:?}, but the CLI did not get it back: {:?}",
        receipt.as_ref().map_err(ToString::to_string)
    );
    let receipt = receipt.assert_value_with(&context);
    assert_eq!(receipt.run_id, run_id, "{submission:?}");
    assert_eq!(state.admissions.len(), 1, "{submission:?}: duplicate run");
    assert_eq!(state.submissions, lost_replies + 1, "{submission:?}");
    assert!(
        state.unexpected.is_empty(),
        "{submission:?}: {:?}",
        state.unexpected
    );
}

async fn assert_unknown_outcome_names_the_retry_key(submission: Submission) {
    let root = temp_root();
    let stub = LossyTarget::spawn(submission, usize::MAX, Answer::Receipt).await;
    let target = submission.target(stub.origin());
    let name = target.name.clone();
    let backend = backend(&root, target).await;
    let mut request = submission.request();
    request.github_token = Some(FORGE_SECRET.to_owned());

    let error = backend.run_submit(Some(&name), request).await;
    let state = stub.stop();

    let error = error.assert_error_with(&format!("{submission:?}: every reply was lost"));
    let message = error.to_string();
    let context = format!("{submission:?}: {message}");
    assert!(message.contains(SUBMISSION_KEY), "{context}");
    assert!(message.contains("--submission-key"), "{context}");
    assert!(
        message.contains(&format!("zeroshot list --target {name}")),
        "{context}"
    );
    assert!(message.to_lowercase().contains("unknown"), "{context}");
    assert!(!message.contains(SERVER_DOWN_HINT), "{context}");
    let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
    assert_eq!(diagnostic["kind"], "target", "{context}");
    assert!(
        diagnostic["message"]
            .as_str()
            .is_some_and(|text| text.contains(SUBMISSION_KEY)),
        "{context}"
    );
    assert!(!message.contains(FORGE_SECRET), "{context}");
    assert!(!diagnostic.to_string().contains(FORGE_SECRET), "{context}");
    assert_eq!(state.admissions.len(), 1, "{context}");
    assert_eq!(state.submissions, SUBMISSION_ATTEMPTS, "{context}");
    assert!(
        state.unexpected.is_empty(),
        "{context}: {:?}",
        state.unexpected
    );
}

#[tokio::test]
async fn direct_lost_reply_returns_the_admitted_run() {
    assert_lost_reply_returns_the_admitted_run(Submission::Direct, 1).await;
}

#[tokio::test]
async fn hosted_lost_reply_returns_the_admitted_run() {
    assert_lost_reply_returns_the_admitted_run(Submission::Hosted, 1).await;
}

#[tokio::test]
async fn hosted_profile_lost_reply_returns_the_admitted_run() {
    assert_lost_reply_returns_the_admitted_run(Submission::HostedProfile, 1).await;
}

#[tokio::test]
async fn direct_final_replay_returns_the_admitted_run() {
    assert_lost_reply_returns_the_admitted_run(Submission::Direct, SUBMISSION_ATTEMPTS - 1).await;
}

#[tokio::test]
async fn a_target_answer_to_a_replay_ends_the_replays_with_an_unknown_outcome() {
    let root = temp_root();
    let stub = LossyTarget::spawn(Submission::Direct, 1, Answer::Unavailable).await;
    let target = Submission::Direct.target(stub.origin());
    let name = target.name.clone();
    let backend = backend(&root, target).await;

    let error = backend
        .run_submit(Some(&name), Submission::Direct.request())
        .await;
    let state = stub.stop();

    let message = error
        .assert_error_with("the replay was answered with a refusal")
        .to_string();
    // The refusal does not settle whether the lost first submission was admitted.
    assert!(message.contains("submission outcome unknown"), "{message}");
    assert!(
        message.contains("target run request failed: connection interrupted"),
        "{message}"
    );
    assert!(message.contains(SUBMISSION_KEY), "{message}");
    assert_eq!(state.submissions, 2, "{message}");
    assert_eq!(state.admissions.len(), 1, "{message}");
}

#[tokio::test]
async fn direct_unrecovered_reply_reports_an_unknown_outcome_with_the_key() {
    assert_unknown_outcome_names_the_retry_key(Submission::Direct).await;
}

#[tokio::test]
async fn hosted_unrecovered_reply_reports_an_unknown_outcome_with_the_key() {
    assert_unknown_outcome_names_the_retry_key(Submission::Hosted).await;
}

#[tokio::test]
async fn hosted_profile_unrecovered_reply_reports_an_unknown_outcome_with_the_key() {
    assert_unknown_outcome_names_the_retry_key(Submission::HostedProfile).await;
}
