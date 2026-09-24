use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::http::HeaderName;
use openengine_cluster_testkit::assertions::AssertValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::native_v2_observability::history::RunHistoryList;

fn accepted_headers(origin: &BrowserOrigin) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::HOST,
        HeaderValue::from_str(&origin.authority).assert_value(),
    );
    headers
}

fn embedded_asset_with_extension(directory: &Dir<'_>, extension: &str) -> Option<String> {
    directory
        .files()
        .find(|file| {
            file.path()
                .extension()
                .is_some_and(|value| value == extension)
        })
        .map(|file| file.path().to_string_lossy().into_owned())
        .or_else(|| {
            directory
                .dirs()
                .find_map(|child| embedded_asset_with_extension(child, extension))
        })
}

fn target_service(root: &openengine_cluster_testkit::TemporaryDirectory) -> UiService {
    let observations = crate::native_v2_observability::NativeV2Observability::new(Arc::new(
        crate::v2_run_ledger::fake::FakeRunLedger::new(),
    ));
    UiService::for_target(
        root.as_path().to_owned(),
        "https://target.example",
        observations,
    )
    .assert_value()
}

#[test]
fn browser_origins_are_exact_http_authorities_without_url_components() {
    for (value, authority, serialized) in [
        (
            "http://127.0.0.1:8080",
            "127.0.0.1:8080",
            "http://127.0.0.1:8080",
        ),
        (
            "https://target.example",
            "target.example",
            "https://target.example",
        ),
        ("https://[::1]:9443", "[::1]:9443", "https://[::1]:9443"),
    ] {
        let origin = BrowserOrigin::new(value).assert_value();
        assert_eq!(origin.authority, authority);
        assert_eq!(origin.origin, serialized);
    }

    for invalid in [
        "not a URL",
        "ftp://target.example",
        "https://user@target.example",
        "https://target.example/path",
        "https://target.example?query=1",
        "https://target.example#fragment",
    ] {
        assert!(BrowserOrigin::new(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn browser_header_fence_accepts_only_one_exact_host_origin_and_site() {
    let origin = BrowserOrigin::new("https://target.example").assert_value();
    let mut headers = accepted_headers(&origin);
    assert!(origin.accepts(&headers));

    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://target.example"),
    );
    headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    assert!(origin.accepts(&headers));
    headers.insert("sec-fetch-site", HeaderValue::from_static("none"));
    assert!(origin.accepts(&headers));

    for (name, value) in [
        (header::HOST, "other.example"),
        (header::ORIGIN, "https://other.example"),
        (HeaderName::from_static("sec-fetch-site"), "cross-site"),
    ] {
        let mut rejected = accepted_headers(&origin);
        rejected.insert(name.clone(), HeaderValue::from_str(value).assert_value());
        assert!(
            !origin.accepts(&rejected),
            "accepted {}: {value}",
            name.as_str()
        );
    }

    let mut missing_host = HeaderMap::new();
    missing_host.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://target.example"),
    );
    assert!(!origin.accepts(&missing_host));
    for name in [
        header::HOST,
        header::ORIGIN,
        HeaderName::from_static("sec-fetch-site"),
    ] {
        let mut duplicate = accepted_headers(&origin);
        duplicate.append(name.clone(), HeaderValue::from_static("duplicate"));
        assert!(
            !origin.accepts(&duplicate),
            "accepted duplicate {}",
            name.as_str()
        );
    }
    let mut non_text = accepted_headers(&origin);
    non_text.insert(
        header::ORIGIN,
        HeaderValue::from_bytes(b"\xff").assert_value(),
    );
    assert!(!origin.accepts(&non_text));
}

#[tokio::test]
async fn embedded_assets_have_explicit_types_and_missing_paths_are_not_fallbacks() {
    for (path, expected) in [
        ("index.html", "text/html; charset=utf-8"),
        ("history-examples.json", "application/json; charset=utf-8"),
        ("oec-mascot.webp", "image/webp"),
    ] {
        let response = file_response(path);
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()[header::CONTENT_TYPE], expected, "{path}");
    }
    for (extension, expected) in [
        ("js", "text/javascript; charset=utf-8"),
        ("css", "text/css; charset=utf-8"),
        ("woff2", "font/woff2"),
    ] {
        let path = embedded_asset_with_extension(&ASSETS, extension).assert_value();
        let response = file_response(&path);
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()[header::CONTENT_TYPE], expected, "{path}");
    }
    assert_eq!(file_response("missing.js").status(), StatusCode::NOT_FOUND);
    assert_eq!(index().await.status(), StatusCode::OK);
    assert_eq!(
        asset(Path("missing.js".to_owned())).await.status(),
        StatusCode::NOT_FOUND
    );
}

struct EmptyRemoteHistory {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl RunHistoryTransport for EmptyRemoteHistory {
    async fn get(
        &self,
        request: super::super::RunHistoryRequest,
    ) -> Result<super::super::RunHistoryResponse, super::super::RunHistoryTransportError> {
        assert!(matches!(
            request,
            super::super::RunHistoryRequest::List { after: None }
        ));
        self.calls.fetch_add(1, Ordering::SeqCst);
        super::super::RunHistoryResponse::new(
            StatusCode::OK.as_u16(),
            serde_json::to_vec(&RunHistoryList {
                runs: Vec::new(),
                next_cursor: None,
            })
            .assert_value(),
        )
    }
}

#[tokio::test]
async fn configured_remote_history_stays_behind_the_local_browser_boundary() {
    let calls = Arc::new(AtomicUsize::new(0));
    let target = RunHistoryTarget::remote(EmptyRemoteHistory {
        calls: calls.clone(),
    });
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "profile-ui-remote-history-target",
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let origin = format!("http://{address}");
    let state = UiState::new(
        LocalRunProfileStore::new(root.path("profiles")),
        target.runs,
        &origin,
        "local",
    )
    .assert_value();
    let shutdown = state.shutdown.clone();
    let app = router(state, false);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });

    let response = reqwest::get(format!("{origin}/ui/api/runs"))
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.assert_value(),
        serde_json::json!({"runs": [], "nextCursor": null})
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    shutdown.cancel();
    let stopped = reqwest::get(format!("{origin}/ui/api/runs"))
        .await
        .assert_value();
    assert_eq!(stopped.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        stopped.json::<serde_json::Value>().await.assert_value()["code"],
        "server_stopping"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[test]
fn security_headers_are_applied_to_success_and_error_responses() {
    for mut response in [
        secured(StatusCode::NO_CONTENT.into_response()),
        secured(problem(StatusCode::FORBIDDEN, "denied", "denied")),
    ] {
        let headers = response.headers_mut();
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        assert!(
            headers["content-security-policy"]
                .to_str()
                .assert_value()
                .contains("frame-ancestors 'self'")
        );
    }
}

#[tokio::test]
async fn shutdown_and_target_service_boundaries_are_immediate_and_shared() {
    let shutdown = Shutdown::default();
    let observer = shutdown.clone();
    shutdown.cancel();
    observer.cancelled().await;
    assert!(*shutdown.0.borrow());

    let observations = crate::native_v2_observability::NativeV2Observability::new(Arc::new(
        crate::v2_run_ledger::fake::FakeRunLedger::new(),
    ));
    assert!(
        UiService::for_target(
            PathBuf::from("relative"),
            "https://target.example",
            observations.clone()
        )
        .is_err()
    );
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("profile-ui-target");
    assert!(
        UiService::for_target(root.as_path().to_owned(), "invalid", observations.clone()).is_err()
    );
    let service = UiService::for_target(
        root.as_path().to_owned(),
        "https://target.example",
        observations,
    )
    .assert_value();
    let discovery = service.run_history_discovery().assert_value();
    assert_eq!(discovery.kind, RUN_HISTORY_KIND);
    assert_eq!(discovery.base_url, "https://target.example");
    assert_eq!(
        discovery.route_templates.list,
        "/native-v2/run-history{?after}"
    );
    assert_eq!(
        discovery.route_templates.detail,
        "/native-v2/run-history/{run_id}"
    );
    assert_eq!(
        discovery.route_templates.page,
        "/native-v2/run-history/{run_id}/page{?after}"
    );
    service.shutdown();
    assert!(*service.shutdown.0.borrow());
}

#[tokio::test]
async fn target_connection_serves_one_http_lifetime_and_drains_on_shutdown() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("profile-ui-target-connection");
    let service = target_service(&root);

    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let client = TcpStream::connect(address).await.assert_value();
    let (server_stream, _) = listener.accept().await.assert_value();
    let served = tokio::spawn({
        let service = service.clone();
        async move { service.serve_connection(server_stream).await }
    });
    let (mut reader, mut writer) = client.into_split();
    writer
        .write_all(b"GET /ui/ HTTP/1.1\r\nHost: target.example\r\nConnection: close\r\n\r\n")
        .await
        .assert_value();
    let mut response = Vec::new();
    reader.read_to_end(&mut response).await.assert_value();
    let response = String::from_utf8(response).assert_value();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    served.await.assert_value().assert_value();

    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let idle_client = TcpStream::connect(address).await.assert_value();
    let (server_stream, _) = listener.accept().await.assert_value();
    let draining = tokio::spawn({
        let service = service.clone();
        async move { service.serve_connection(server_stream).await }
    });
    service.shutdown();
    draining.await.assert_value().assert_value();
    drop(idle_client);
}

#[tokio::test]
async fn standalone_ui_rejects_non_loopback_before_binding() {
    let error = serve("0.0.0.0:0".parse().assert_value())
        .await
        .err()
        .assert_value();
    assert!(error.to_string().contains("loopback"));
}

#[tokio::test]
async fn standalone_ui_reports_an_occupied_loopback_listener_without_starting() {
    let occupied = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = occupied.local_addr().assert_value();
    let error = serve(address).await.err().assert_value();
    assert!(matches!(
        error,
        NativeV2CliError::Local(message) if !message.is_empty()
    ));
}

#[tokio::test]
async fn malformed_http_is_rejected_by_the_target_connection_boundary() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "profile-ui-target-malformed-connection",
    );
    let service = target_service(&root);
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let mut client = TcpStream::connect(address).await.assert_value();
    let (server_stream, _) = listener.accept().await.assert_value();

    client.write_all(b"not HTTP\r\n\r\n").await.assert_value();
    client.shutdown().await.assert_value();
    let error = service
        .serve_connection(server_stream)
        .await
        .err()
        .assert_value();
    assert_eq!(error.kind(), io::ErrorKind::Other);
}

#[tokio::test]
async fn standalone_listener_drains_immediately_after_an_owned_shutdown() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("profile-ui-owned-shutdown");
    let service = target_service(&root);
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let origin = format!("http://{}", listener.local_addr().assert_value());

    serve_listener_until(
        listener,
        service.clone(),
        origin,
        std::future::ready(Ok(())),
    )
    .await
    .assert_value();
    assert!(*service.shutdown.0.borrow());
}

#[tokio::test]
async fn local_service_preparation_selects_remote_or_native_history_without_serving() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let calls = Arc::new(AtomicUsize::new(0));
    let target = RunHistoryTarget::remote(EmptyRemoteHistory {
        calls: calls.clone(),
    });
    let (remote, origin) = prepare_local_service(&listener, Some(target)).assert_value();
    assert_eq!(
        origin,
        format!("http://{}", listener.local_addr().assert_value())
    );
    assert!(remote.run_history_discovery().is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let (native, native_origin) = prepare_local_service(&listener, None).assert_value();
    assert_eq!(native_origin, origin);
    assert!(native.run_history_discovery().is_none());
}
