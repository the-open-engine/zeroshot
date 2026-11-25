use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::*;
use crate::source_code_provider::adapters::core::http_fixture::{
    self, IntoResponse, Json, Request as FixtureRequest, Response,
};

const API_VERSION: &str = "2026-03-10";
const TOKEN: SourceDeliveryCredential<'_> = SourceDeliveryCredential::token("test-private-token");
const MAX: usize = 1024;

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

struct Server {
    url: Url,
    task: JoinHandle<()>,
}

impl Server {
    async fn start(f: impl Fn(FixtureRequest) -> Response + Send + Sync + 'static) -> Self {
        let (listener, url) = bind("http").await;
        let task = tokio::spawn(http_fixture::serve(listener, f));
        Self { url, task }
    }

    fn client(&self) -> Http {
        Http::loopback(self.url.as_str(), API_VERSION).unwrap()
    }
}

async fn bind(scheme: &str) -> (TcpListener, Url) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("{scheme}://{}/", listener.local_addr().unwrap())).unwrap();
    (listener, url)
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[test]
fn routes_rest_and_graphql_on_the_selected_instance() {
    for (base, rest, graphql) in [
        (
            "https://github.com",
            "https://api.github.com/",
            "https://api.github.com/graphql",
        ),
        (
            "https://team.ghe.com",
            "https://api.team.ghe.com/",
            "https://api.team.ghe.com/graphql",
        ),
        (
            "https://enterprise.example:8443",
            "https://enterprise.example:8443/api/v3/",
            "https://enterprise.example:8443/api/graphql",
        ),
    ] {
        let (actual_rest, actual_graphql) =
            api_urls(&SourceForgeBaseUrl::new(base).unwrap()).unwrap();
        assert_eq!(actual_rest.as_str(), rest);
        assert_eq!(actual_graphql.as_str(), graphql);
    }
    assert!(
        api_urls(&SourceForgeBaseUrl::new("https://enterprise.example/install").unwrap()).is_err()
    );
    assert!(Http::loopback("http://example.com/", API_VERSION).is_err());
}

#[tokio::test]
async fn rest_encodes_paths_query_and_json_and_preserves_response_metadata() {
    let server = Server::start(|request| {
        assert_eq!(request.method, Method::PATCH);
        assert_eq!(
            request.uri.path(),
            "/repos/acme/project/rules/branches/topic%2F%23%25"
        );
        assert_eq!(request.uri.query_pairs().next().unwrap().1, "head:ü/ +");
        assert_eq!(
            request.headers["authorization"],
            "Bearer test-private-token"
        );
        assert_eq!(request.headers[API_VERSION_HEADER], API_VERSION);
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap(),
            json!({"body":"preserve newline\nand = signs"})
        );
        (
            StatusCode::CONFLICT,
            Json(json!({"message":"a real HTTP response"})),
        )
            .into_response()
            .header("X-GitHub-Request-Id", "request-1")
            .header("Retry-After", "7")
    })
    .await;
    let client = server.client();
    let reply = client
        .send(
            Request::Rest {
                method: Method::PATCH,
                path: &["repos", "acme", "project", "rules", "branches", "topic/#%"],
                query: &[("head", "head:ü/ +")],
                body: Some(&json!({"body":"preserve newline\nand = signs"})),
            },
            TOKEN,
            (deadline(), MAX),
        )
        .await
        .unwrap();
    assert_eq!(reply.head.status, StatusCode::CONFLICT);
    assert_eq!(reply.head.headers["retry-after"], "7");
    assert_eq!(reply.head.headers["x-github-request-id"], "request-1");
    assert_eq!(
        serde_json::from_slice::<Value>(&reply.body).unwrap(),
        json!({"message":"a real HTTP response"})
    );
    let failure = client
        .send(
            Request::get(&["repos", "..", "project"]),
            TOKEN,
            (deadline(), MAX),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::InvalidRequest);
}

#[tokio::test]
async fn graphql_uses_json_variables_and_keeps_mixed_response_as_provided() {
    let server = Server::start(|request| {
        assert_eq!(request.method, Method::POST);
        assert_eq!(request.uri.path(), "/graphql");
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap(),
            json!({
                "query":"query Test { viewer { id } }",
                "variables":{"number":17,"cursor":null,"disabled":false,"text":"field=α"}
            })
        );
        Json(json!({"data":{"viewer":{"id":"actor"}},"errors":[{"message":"partial"}]}))
            .into_response()
    })
    .await;
    let reply = server
        .client()
        .send(
            Request::Graphql {
                document: "query Test { viewer { id } }",
                variables: &json!({"number":17,"cursor":null,"disabled":false,"text":"field=α"}),
            },
            TOKEN,
            (deadline(), MAX),
        )
        .await
        .unwrap();
    assert_eq!(reply.head.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&reply.body).unwrap(),
        json!({"data":{"viewer":{"id":"actor"}},"errors":[{"message":"partial"}]})
    );
}

#[tokio::test]
async fn interrupted_response_preserves_header_evidence_and_redacts_the_partial_body() {
    let server = Server::start(|request| {
        let status = request
            .uri
            .path()
            .trim_start_matches('/')
            .parse::<u16>()
            .unwrap();
        (
            StatusCode::from_u16(status).unwrap(),
            Json(json!({"message":"test-private-token"})),
        )
            .into_response()
            .content_length(256)
            .header("X-GitHub-Request-Id", "broken-body")
            .header("Retry-After", "3")
    })
    .await;
    let client = server.client();
    for status in ["401", "403", "503"] {
        let failure = client
            .send(Request::get(&[status]), TOKEN, (deadline(), MAX))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::InterruptedBody);
        let head = failure.head.as_ref().unwrap();
        assert_eq!(head.status.as_str(), status);
        assert_eq!(head.headers["retry-after"], "3");
        assert_eq!(head.headers["x-github-request-id"], "broken-body");
        assert!(failure.diagnostic.contains("GitHub request broken-body"));
        assert!(failure.diagnostic.contains("partial response"));
        assert!(!failure.diagnostic.contains(TOKEN.expose()));
    }
}

#[tokio::test]
async fn one_mutation_transmission_when_the_server_drops_its_response() {
    let mutations = Arc::new(AtomicUsize::new(0));
    let writes = mutations.clone();
    let server = Server::start(move |request| {
        assert_eq!(request.method, Method::POST);
        writes.fetch_add(1, Ordering::SeqCst);
        Response::disconnected()
    })
    .await;
    let failure = server
        .client()
        .send(
            Request::Graphql {
                document: "mutation Test",
                variables: &json!({}),
            },
            TOKEN,
            (deadline(), MAX),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::Send);
    assert!(failure.head.is_none());
    assert_eq!(mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn expired_deadline_does_not_send_a_request() {
    let writes = Arc::new(AtomicUsize::new(0));
    let seen = writes.clone();
    let server = Server::start(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        StatusCode::NO_CONTENT.into_response()
    })
    .await;
    let failure = server
        .client()
        .send(
            Request::Graphql {
                document: "mutation Test",
                variables: &json!({}),
            },
            TOKEN,
            (Instant::now(), MAX),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::Timeout);
    assert!(failure.head.is_none());
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn checks_complete_reply_and_bounds_error_body_without_reinterpreting_status() {
    const LARGE_BYTES: usize = 512 * 1024;
    let mut expected = b"test-private-token\0\xff".to_vec();
    expected.resize(LARGE_BYTES, b'x');
    let payload = expected.clone();
    let server = Server::start(move |request| {
        if request.uri.path() == "/large" {
            return Response::raw(payload.clone());
        }
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"too-large":"x".repeat(MAX_ERROR_BYTES)})),
        )
            .into_response()
    })
    .await;
    let client = server.client();
    let reply = client
        .send(Request::get(&["large"]), TOKEN, (deadline(), LARGE_BYTES))
        .await
        .unwrap();
    assert_eq!(reply.body, expected);
    let failure = client
        .send(
            Request::get(&["oversize"]),
            TOKEN,
            (deadline(), MAX_ERROR_BYTES * 2),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::Oversize);
    assert_eq!(
        failure.head.unwrap().status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(failure.diagnostic.contains("65536-byte bound"));
}

#[tokio::test]
async fn deadline_applies_after_headers_and_retains_partial_body_evidence() {
    let (listener, url) = bind("http").await;
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 503 Fixture\r\nContent-Length: 40\r\n\r\npartial test-private-to")
            .await
            .unwrap();
        std::future::pending::<()>().await;
    });
    let server = Server { url, task };
    let failure = server
        .client()
        .send(
            Request::get(&["deadline"]),
            TOKEN,
            (Instant::now() + Duration::from_secs(1), MAX),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::Timeout);
    assert_eq!(
        failure.head.unwrap().status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(failure.diagnostic.contains("partial response: partial"));
    assert!(!failure.diagnostic.contains("test-private-to"));
}

#[test]
fn validates_job_log_redirect_location_with_https_and_a_single_header() {
    let api = Url::parse("https://api.github.com/repos/acme/project/actions/jobs/3/logs").unwrap();
    for (location, valid) in [
        ("/signed?key=123", true),
        ("https://artifacts.example/signed", true),
        ("http://artifacts.example/signed", false),
        ("https://user:private@artifacts.example/signed", false),
        ("https://artifacts.example/signed#fragment", false),
        ("", false),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("location", location.parse().unwrap());
        assert_eq!(log_location(&api, &headers).is_some(), valid, "{location}");
        headers.append("location", "/another".parse().unwrap());
        assert!(log_location(&api, &headers).is_none());
    }
}

async fn tls_archive(
    ca_path: &std::path::Path,
    handler: impl Fn(FixtureRequest) -> Response + Send + Sync + 'static,
) -> Server {
    let (listener, url) = bind("https").await;
    let task = tokio::spawn(http_fixture::serve_tls(
        listener,
        http_fixture::loopback_tls(ca_path),
        handler,
    ));
    Server { url, task }
}

async fn invalid_log_redirect(client: &Http, job: u64) -> Failure {
    let failure = client
        .job_log(("acme/project", job), TOKEN, (deadline(), MAX))
        .await
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::InvalidRedirect);
    failure
}

#[tokio::test]
async fn actions_log_download_uses_custom_ca_without_forwarding_the_bearer_or_redirecting_again() {
    let root = tempfile::tempdir().unwrap();
    let ca = root.path().join("ca.pem");
    let archive = tls_archive(&ca, |request| {
        assert_eq!(request.method, Method::GET);
        assert_eq!(request.uri.query(), Some("sig=example%2Ftoken"));
        assert!(!request.headers.contains_key("authorization"));
        assert!(!request.headers.contains_key(API_VERSION_HEADER));
        match request.uri.path() {
            "/signed" => Response::raw(b"job log".to_vec()),
            "/chain" => StatusCode::FOUND
                .into_response()
                .header("Location", "/unfollowed?token=another-secret"),
            other => panic!("unexpected archive path {other}"),
        }
    })
    .await;
    let location = archive.url.join("signed?sig=example%2Ftoken").unwrap();
    let raw_location = location.to_string();
    let further_redirect = archive.url.join("chain?sig=example%2Ftoken").unwrap();
    let server = Server::start(move |request| {
        assert_eq!(
            request.headers["authorization"],
            "Bearer test-private-token"
        );
        let destination = match request.uri.path() {
            "/repos/acme/project/actions/jobs/3/logs" => &raw_location,
            "/repos/acme/project/actions/jobs/4/logs" => further_redirect.as_str(),
            other => panic!("unexpected API path {other}"),
        };
        StatusCode::FOUND
            .into_response()
            .header("Location", destination)
    })
    .await;
    let network = ForgeNetwork::from_snapshot(&BTreeMap::from([(
        "SSL_CERT_FILE".to_owned(),
        ca.to_string_lossy().into_owned(),
    )]));
    let client = Http::build(
        (server.url.clone(), server.url.join("graphql").unwrap()),
        network,
        API_VERSION,
    )
    .unwrap();
    let (reply, secrets) = client
        .job_log(("acme/project", 3), TOKEN, (deadline(), MAX))
        .await
        .unwrap();
    assert_eq!(reply.head.status, StatusCode::OK);
    assert_eq!(reply.body, b"job log");
    for value in [
        location.as_str(),
        "sig=example%2Ftoken",
        "example%2Ftoken",
        "example/token",
    ] {
        assert!(secrets.iter().any(|secret| secret == value));
    }
    let failure = invalid_log_redirect(&client, 4).await;
    assert!(!failure.diagnostic.contains("another-secret"));
}

#[tokio::test]
async fn actions_log_can_return_directly_and_refuses_insecure_redirects() {
    let server = Server::start(|request| {
        if request.uri.path().contains("/jobs/3/") {
            return Response::raw(b"\x1b[31mfailed\x1b[0m".to_vec());
        }
        StatusCode::FOUND
            .into_response()
            .header("Location", "http://artifact.example/private")
    })
    .await;
    let client = server.client();
    let (reply, secrets) = client
        .job_log(("acme/project", 3), TOKEN, (deadline(), MAX))
        .await
        .unwrap();
    assert_eq!(reply.body, b"\x1b[31mfailed\x1b[0m");
    assert!(secrets.is_empty());
    let failure = invalid_log_redirect(&client, 4).await;
    assert!(!failure.diagnostic.contains("artifact.example"));
}
