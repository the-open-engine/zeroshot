use std::sync::atomic::{AtomicUsize, Ordering};
use reqwest::StatusCode;
use tokio::sync::Notify;

use crate::source_code_provider::adapters::core::{
    http_fixture::{self, IntoResponse, Json, Response},
    network::ForgeNetwork,
};
use super::*;

const TOKEN: &str = "test-api-token";

async fn endpoint() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/", listener.local_addr().unwrap());
    (listener, address)
}

#[tokio::test]
async fn access_checks_repository_and_enterprise_version_in_one_http_read() {
    for (base, version, name, accepted) in [
        ("https://github.com", "", "acme/project", true),
        (
            "https://ghe.example",
            "enterprise-server@3.22.2",
            "acme/project",
            true,
        ),
        ("https://ghe.example", "3.22.1", "acme/project", false),
        ("https://ghe.example", "", "acme/project", false),
        ("https://github.com", "", "other/project", false),
    ] {
        let (listener, address) = endpoint().await;
        let count = Arc::new(AtomicUsize::new(0));
        let received = count.clone();
        let http = tokio::spawn(http_fixture::serve(listener, move |request| {
            assert_eq!(request.method, Method::GET);
            assert_eq!(request.uri.path(), "/repos/acme/project");
            assert_eq!(request.headers["authorization"], "Bearer test-api-token");
            received.fetch_add(1, Ordering::SeqCst);
            Json(json!({"full_name":name}))
                .into_response()
                .header("x-github-enterprise-version", version)
        }));
        let authority =
            GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted("/fixture".into()))
                .with_base_url(openengine_cluster_protocol::SourceForgeBaseUrl::new(base).unwrap())
                .unwrap()
                .with_test_api(&address);
        assert_eq!(
            authority
                .check_access("acme/project", SourceDeliveryCredential::token(TOKEN), None)
                .await
                .is_ok(),
            accepted
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        http.abort();
    }
}

#[tokio::test]
async fn github_access_redacts_proxy_and_token_echoes_in_http_errors() {
    let (listener, address) = endpoint().await;
    let http = tokio::spawn(http_fixture::serve(listener, |_| {
        Json(json!({"message":"proxy-user:proxy-password test-api-token"}))
            .into_response()
            .status(StatusCode::UNAUTHORIZED)
    }));
    let network = ForgeNetwork::from_snapshot(&BTreeMap::from([(
        "HTTPS_PROXY".into(),
        "http://proxy-user:proxy-password@127.0.0.1:1".into(),
    )]));
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted("/fixture".into()))
        .with_network(network)
        .with_test_api(&address);
    let error = authority
        .check_access("acme/project", SourceDeliveryCredential::token(TOKEN), None)
        .await
        .unwrap_err();
    assert!(error.authentication_failed());
    for secret in ["proxy-user", "proxy-password", TOKEN] {
        assert!(!error.to_string().contains(secret), "{error}");
    }
    http.abort();
}

#[tokio::test]
async fn stop_request_cancels_a_stalled_project_read() {
    let (listener, address) = endpoint().await;
    let started = Arc::new(Notify::new());
    let disconnected = Arc::new(Notify::new());
    let arrived = started.clone();
    let connection_ended = disconnected.clone();
    let http = tokio::spawn(http_fixture::serve(listener, move |_| {
        arrived.notify_one();
        Response::stall_until_disconnect(connection_ended.clone())
    }));
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted("/fixture".into()))
        .with_test_api(&address);
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let check = authority.check_access(
        "acme/project",
        SourceDeliveryCredential::token(TOKEN),
        Some(stopping),
    );
    tokio::pin!(check);
    tokio::select! {
        () = started.notified() => {}
        _ = &mut check => panic!("project check settled before stop"),
    }
    stop.send(true).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), check)
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("stopped"));
    assert!(!error.cleanup_unconfirmed(), "{error}");
    tokio::time::timeout(Duration::from_secs(1), disconnected.notified())
        .await
        .expect("stopped preflight should close its HTTP connection");
    http.abort();
}
