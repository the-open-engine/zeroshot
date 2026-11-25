use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

use crate::source_code_provider::adapters::core::http_fixture::{self, IntoResponse, Json};

#[tokio::test]
async fn ghe_version_verification_precedes_requests_and_is_cached() {
    for (version, accepted) in [
        ("3.22.2", true),
        ("3.22.1", false),
        ("3.21.9", false),
        ("unknown", false),
    ] {
        let (listener, address) = http_fixture::loopback().await;
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = calls.clone();
        let http = tokio::spawn(http_fixture::serve(listener, move |request| {
            assert_eq!(
                request.headers["authorization"],
                "Bearer instance-test-token"
            );
            received.lock().unwrap().push(request.uri.path().to_owned());
            Json(match request.uri.path() {
                "/meta" => json!({"installed_version": version}),
                "/user" => json!({"id": 42}),
                _ => panic!("unexpected Enterprise API request"),
            })
            .into_response()
        }));
        let authority =
            GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted("/fixture".into()))
                .with_base_url(SourceForgeBaseUrl::new("https://ghe.example").unwrap())
                .unwrap()
                .with_test_api(&address);
        for _ in 0..if accepted { 2 } else { 1 } {
            let result = authority
                .rest(
                    Request::get(&["user"]),
                    SourceDeliveryCredential::token("instance-test-token"),
                )
                .await;
            if accepted {
                assert_eq!(result.unwrap()["id"], 42);
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("3.22.2"));
                assert!(!error.contains("instance-test-token"));
            }
        }
        let observed = calls.lock().unwrap();
        let expected: &[&str] = if accepted {
            &["/meta", "/user", "/user"]
        } else {
            &["/meta"]
        };
        assert_eq!(&*observed, expected);
        http.abort();
    }
}

#[test]
fn ghe_receipt_links_preserve_the_admitted_instance() {
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted("/fixture".into()))
        .with_base_url(SourceForgeBaseUrl::new("https://ghe.example:8443").assert_value())
        .assert_value();
    let receipt = SourceReviewReceipt {
        review_id: "17".to_owned(),
        repository: "acme/project".to_owned(),
        target_branch: "main".to_owned(),
        head_branch: "zeroshot/v2-run".to_owned(),
        head_revision: "b".repeat(40),
    };
    let links = authority.canonical_review_links(&receipt).assert_value();
    assert_eq!(links.url, "https://ghe.example:8443/acme/project/pull/17");
    assert_eq!(
        links.commit_url,
        format!(
            "https://ghe.example:8443/acme/project/commit/{}",
            receipt.head_revision
        )
    );
    assert!(
        authority
            .with_base_url(SourceForgeBaseUrl::new("https://ghe.example/path").assert_value())
            .is_err()
    );
}
