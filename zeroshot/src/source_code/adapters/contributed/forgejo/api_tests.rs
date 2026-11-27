//! Bounded client acceptance for the contributed Forgejo adapter.
use std::collections::BTreeMap;

use openengine_cluster_testkit::assertions::{AssertAt, AssertError, AssertValue};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use url::Position;

use crate::source_code_provider::adapters::core::network::ForgeNetwork;
use crate::source_code_provider::SourceDeliveryCredential;

use super::api::{Api, ApiRequest, PAGE_LIMIT, PAGE_SIZE, RESPONSE_LIMIT};
use super::http_fixture::{Fixture, Response};
use super::wire::ServerVersion;

const TOKEN: &str = "forgejo-test-token";

fn credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token(TOKEN)
}

async fn local_api(fixture: &Fixture) -> Api {
    Api::loopback(&fixture.endpoint()).assert_value()
}

#[tokio::test]
async fn a1_admits_only_https_and_the_loopback_fixture() {
    let admitted = Api::new("https://forge.example/git").assert_value();
    assert_eq!(
        &admitted.base()[..Position::AfterPath],
        "https://forge.example/git"
    );

    for base in ["http://forge.example", "ftp://forge.example/git"] {
        let error = Api::new(base).assert_error();
        assert!(format!("{error}").contains("Forgejo"), "{base}: {error}");
        assert!(!format!("{error}").contains("GitLab"), "{base}: {error}");
    }

    let fixture = Fixture::new().await;
    for base in [
        "http://localhost:1",
        "http://forge.example",
        "https://127.0.0.1:1",
    ] {
        let error = Api::loopback(base).assert_error();
        assert!(format!("{error}").contains("Forgejo"), "{base}: {error}");
    }

    let local = local_api(&fixture).await;
    assert!(Api::loopback("http://[::1]:1").is_ok());
    let networked = local
        .clone()
        .with_network(ForgeNetwork::from_snapshot(&BTreeMap::new()))
        .assert_value();
    fixture.queue(Response::json(json!({"version": "16.0.0"})));
    networked
        .require_supported(credential())
        .await
        .assert_value();
    assert_eq!(fixture.requests().assert_at(0).uri, "/api/v1/version");
}

#[tokio::test]
async fn a2_exact_authorization_and_api_path() {
    let fixture = Fixture::new().await;
    let api = local_api(&fixture).await;

    fixture.queue(Response::json(json!({"id": 1})));
    let observed: Value = api
        .json(ApiRequest::get(&["repos", "owner", "name"]), credential())
        .await
        .assert_value();
    assert_eq!(observed, json!({"id": 1}));

    let request = fixture.requests().assert_at(0).clone();
    assert_eq!(request.method, Method::GET);
    assert_eq!(request.uri, "/api/v1/repos/owner/name");
    assert_eq!(
        request
            .headers
            .get("authorization")
            .assert_value()
            .to_str()
            .assert_value(),
        "token forgejo-test-token"
    );
    assert_eq!(
        request
            .headers
            .get("accept")
            .assert_value()
            .to_str()
            .assert_value(),
        "application/json"
    );

    fixture.queue(Response::json(json!({"published": true})));
    let written: Value = api
        .json(
            ApiRequest::write(
                &["repos", "owner", "name"],
                Method::POST,
                json!({"body": "Comment"}),
            ),
            credential(),
        )
        .await
        .assert_value();
    assert_eq!(written, json!({"published": true}));

    let write = fixture.requests().assert_at(1).clone();
    assert_eq!(write.method, Method::POST);
    assert_eq!(write.uri, "/api/v1/repos/owner/name");
    assert_eq!(
        String::from_utf8_lossy(&write.body),
        r#"{"body":"Comment"}"#
    );
}

#[tokio::test]
async fn a3_keeps_the_installation_prefix_before_the_api_path() {
    let fixture = Fixture::new().await;
    let api = Api::loopback(&format!("{}/git", fixture.endpoint())).assert_value();
    fixture.queue(Response::json(json!({"version": "16.0.0"})));
    api.require_supported(credential()).await.assert_value();
    assert_eq!(fixture.requests().assert_at(0).uri, "/git/api/v1/version");
}

#[tokio::test]
async fn a4_percent_encodes_url_path_segments() {
    let fixture = Fixture::new().await;
    let api = local_api(&fixture).await;
    fixture.queue(Response::json(json!([])));
    let observed: Vec<Value> = api
        .json(ApiRequest::get(&["repos", "owner/name"]), credential())
        .await
        .assert_value();
    assert!(observed.is_empty());
    assert_eq!(
        fixture.requests().assert_at(0).uri,
        "/api/v1/repos/owner%2Fname"
    );
}

#[test]
fn a5_admits_only_a_complete_three_part_version_at_the_floor() {
    for version in [
        "16.0.0",
        "16.0.1",
        "17.0.0",
        "120.5.2",
        "16.0.0+testing",
        "16.0.0-rc1",
    ] {
        assert!(super::api::require_version(version).is_ok(), "{version}");
    }
    for version in [
        "", "16", "16.0", "15.9.9", "16.0.0.1", "16.0.x", "16.0.0 ", "v16.0.0",
    ] {
        let error = super::api::require_version(version).assert_error();
        assert_eq!(
            format!("{error}"),
            "source delivery identity mismatch: Forgejo server 16.0.0 or newer is required for delivery",
            "{version}"
        );
    }
}

#[tokio::test]
async fn a6_reads_the_server_version_once_and_caches_it() {
    let fixture = Fixture::new().await;
    let api = local_api(&fixture).await;
    fixture.queue(Response::json(json!({"version": "16.0.0"})));
    api.require_supported(credential()).await.assert_value();
    api.require_supported(credential()).await.assert_value();
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn a7_a_redirect_is_a_failure_and_is_never_followed() {
    let fixture = Fixture::new().await;
    let api = local_api(&fixture).await;
    fixture.queue(
        Response::status(StatusCode::FOUND)
            .header("location", "https://other.example/api/v1/version"),
    );
    let error = api
        .json::<ServerVersion>(ApiRequest::get(&["version"]), credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(302));
    assert!(format!("{error}").contains("Forgejo"), "{error}");
    assert_eq!(fixture.requests().len(), 1);
    assert_eq!(fixture.requests().assert_at(0).uri, "/api/v1/version");
}

#[tokio::test]
async fn a8_pages_join_at_the_reported_total_and_stop_at_the_page_bound() {
    let fixture = Fixture::new().await;
    let api = local_api(&fixture).await;
    let path = ["repos", "owner", "name"];

    for page in 1..=3 {
        let items = (1..=if page == 3 { 20 } else { PAGE_SIZE })
            .map(|item| json!(item))
            .collect::<Vec<Value>>();
        fixture.queue(Response::json(items).header("x-total-count", "120"));
    }
    let collected: Vec<Value> = api
        .pages(ApiRequest::get(&path), credential())
        .await
        .assert_value();
    assert_eq!(collected.len(), 120);
    for (index, request) in fixture.requests().iter().enumerate() {
        assert_eq!(
            request.uri,
            format!(
                "/api/v1/repos/owner/name?limit={PAGE_SIZE}&page={}",
                index + 1
            )
        );
    }

    fixture.queue(Response::json(vec![json!(1)]));
    let error = api
        .pages::<Value>(ApiRequest::get(&path), credential())
        .await
        .assert_error();
    assert!(format!("{error}").contains("Forgejo"), "{error}");

    fixture.queue(Response::json(vec![json!(1), json!(2), json!(3)]).header("x-total-count", "2"));
    let error = api
        .pages::<Value>(ApiRequest::get(&path), credential())
        .await
        .assert_error();
    assert!(
        format!("{error}").contains("more items than its reported total of 2"),
        "{error}"
    );

    for _ in 0..PAGE_LIMIT {
        let items = (0..PAGE_SIZE)
            .map(|item| json!(item))
            .collect::<Vec<Value>>();
        fixture.queue(Response::json(items).header("x-total-count", "3201"));
    }
    let error = api
        .pages::<Value>(ApiRequest::get(&path), credential())
        .await
        .assert_error();
    let bound = format!("consecutive page bound of {PAGE_LIMIT}");
    assert!(format!("{error}").contains(bound.as_str()), "{error}");
}

#[tokio::test]
async fn a9_statuses_and_missing_resources_keep_their_meaning() {
    let fixture = Fixture::new().await;
    let api = local_api(&fixture).await;
    let path = ["repos", "owner", "name"];

    fixture.queue(Response::status(StatusCode::UNAUTHORIZED));
    let error = api
        .json::<Value>(ApiRequest::get(&path), credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(401));
    assert!(format!("{error}").contains("401"), "{error}");
    assert!(format!("{error}").contains("Forgejo"), "{error}");

    fixture.queue(Response::status(StatusCode::NOT_FOUND));
    let missing = api
        .optional_json::<Value>(ApiRequest::get(&path), credential())
        .await
        .assert_value();
    assert_eq!(missing, None);

    fixture.queue(Response::status(StatusCode::NOT_FOUND));
    let error = api
        .json::<Value>(ApiRequest::get(&path), credential())
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(404));

    fixture.queue(Response::status(StatusCode::NO_CONTENT));
    api.empty(
        ApiRequest::write(&path, Method::DELETE, json!({"resolution": "declined"})),
        credential(),
    )
    .await
    .assert_value();

    fixture.queue(Response::status(StatusCode::FORBIDDEN));
    let error = api
        .empty(
            ApiRequest::write(&path, Method::PATCH, json!({"body": "Edited"})),
            credential(),
        )
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(403));
    assert!(format!("{error}").contains("Forgejo"), "{error}");
}

#[tokio::test]
async fn a10_credentials_never_enter_a_diagnostic_or_a_debug_value() {
    let fixture = Fixture::new().await;
    let api = local_api(&fixture).await;

    fixture.queue(
        Response::status(StatusCode::BAD_GATEWAY)
            .with_body(format!("upstream accepted token {TOKEN}")),
    );
    let error = api
        .json::<Value>(ApiRequest::get(&["repos", "owner", "name"]), credential())
        .await
        .assert_error();
    let text = format!("{error}");
    assert!(!text.contains(TOKEN), "{text}");
    assert!(text.contains("[REDACTED]"), "{text}");
    assert!(!format!("{api:?}").contains(TOKEN));

    fixture.queue(Response::status(StatusCode::OK).with_body(vec![b'x'; RESPONSE_LIMIT + 1]));
    let error = api
        .json::<Vec<String>>(ApiRequest::get(&["repos", "owner", "name"]), credential())
        .await
        .assert_error();
    assert!(format!("{error}").contains("byte bound"), "{error}");
    assert!(!format!("{error}").contains(TOKEN));
}

#[test]
fn a11_timestamp_prefixes_require_the_complete_utc_shape() {
    // The rule is the shape, not calendar values: the digit positions are what the prefix
    // requires, so an out-of-range month is still a complete timestamp prefix. A line that has
    // one loses exactly the stamp and the one space after it, so a second space stays.
    for (printable, kept) in [
        ("2024-01-02T03:04:05Z failure", "failure"),
        ("2024-01-02T03:04:05.123456Z failure", "failure"),
        ("2024-13-02T03:04:05Z failure", "failure"),
        ("2024-01-02T03:04:05.123456Z  failure", " failure"),
    ] {
        assert_eq!(
            super::api::strip_log_timestamps(printable),
            kept,
            "{printable}"
        );
    }

    for printable in [
        "2024-01-02 03:04:05Z failure",
        "2024-01-02T03:04:05 failure",
        "2024-01-02T03:04:05z failure",
        "2024-01-02T03:04:05.Z failure",
        "2024-01-02T03:04:05.123456Zfailure",
    ] {
        assert_eq!(
            super::api::strip_log_timestamps(printable),
            printable,
            "{printable}"
        );
    }
}

#[test]
fn a12_diagnostic_secrets_include_the_network_credentials() {
    let snapshot = BTreeMap::from([(
        "HTTPS_PROXY".to_string(),
        "https://user:proxy-secret@proxy.example:8080".to_string(),
    )]);
    let network = ForgeNetwork::from_snapshot(&snapshot);
    let api = Api::new("https://forge.example")
        .assert_value()
        .with_network(network.clone())
        .assert_value();

    // No request is made here, so no source identity is part of the diagnostic set.
    let secrets = api.diagnostic_secrets(credential());
    let proxy = network.secrets();
    assert!(
        proxy.iter().any(|secret| secret.contains("proxy-secret")),
        "the snapshot must yield the proxy credential"
    );
    for secret in proxy {
        assert!(secrets.contains(&secret), "missing a network secret");
    }
    assert!(secrets.contains(&TOKEN.to_string()));
}
