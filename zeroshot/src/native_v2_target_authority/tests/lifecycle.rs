use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::*;

#[tokio::test]
async fn shutdown_closes_idle_and_websocket_connections_and_releases_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let endpoint = format!("ws://{address}{OECP_PATH}");
    let factory = Arc::new(FakeFactory::default());
    let server = Arc::new(
        NativeV2TargetServer::new_direct(
            Arc::new(NativeV2TargetAuthority::new(factory.clone())),
            identity(),
            endpoint.clone(),
        )
        .assert_value(),
    );
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve_until(listener, async {
        let _ = shutdown.await;
    }));
    let mut idle = TcpStream::connect(address).await.assert_value();
    idle.write_all(b"GET /native-v2/").await.assert_value();
    let (mut websocket, _) = tokio_tungstenite::connect_async(endpoint)
        .await
        .assert_value();

    stop.send(()).assert_value();
    tokio::time::timeout(Duration::from_secs(7), task)
        .await
        .assert_value_with("server shutdown is bounded even with active observers")
        .assert_value()
        .assert_value();
    let mut byte = [0];
    let closed = tokio::time::timeout(Duration::from_secs(1), idle.read(&mut byte))
        .await
        .assert_value_with("idle connection is owned by the server");
    assert!(matches!(closed, Ok(0) | Err(_)));
    let frame = tokio::time::timeout(Duration::from_secs(1), websocket.next())
        .await
        .assert_value_with("OECP connection is owned by the server");
    assert!(frame.is_none_or(|frame| frame.is_err() || frame.is_ok_and(|frame| frame.is_close())));
    TcpListener::bind(address).await.assert_value();
    assert_eq!(factory.controllers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dropping_listener_task_also_drops_accepted_connections() {
    let (address, endpoint, task) = direct_test_server(Arc::new(FakeFactory::default())).await;
    let (mut websocket, _) = tokio_tungstenite::connect_async(endpoint)
        .await
        .assert_value();
    task.abort();
    assert!(task.await.is_err_and(|error| error.is_cancelled()));
    let frame = tokio::time::timeout(Duration::from_secs(1), websocket.next())
        .await
        .assert_value_with("aborted server cannot leave a detached connection task");
    assert!(frame.is_none_or(|frame| frame.is_err() || frame.is_ok_and(|frame| frame.is_close())));
    TcpListener::bind(address).await.assert_value();
}

#[tokio::test]
async fn native_routes_still_reject_queries() {
    let (address, _, task) = direct_test_server(Arc::new(FakeFactory::default())).await;
    let client = reqwest::Client::new();
    for path in [
        format!("{DISCOVERY_PATH}?query=1"),
        format!("{SESSION_PATH}?query=1"),
        format!("{OECP_PATH}?query=1"),
        "/uix/?query=1".to_owned(),
    ] {
        let response = client
            .get(format!("http://{address}{path}"))
            .send()
            .await
            .assert_value();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let problem = response.json::<TargetHttpProblem>().await.assert_value();
        assert_eq!(problem.code(), "request.invalid");
        assert_eq!(problem.message(), "target HTTP request is malformed");
    }
    task.abort();
}

#[cfg(feature = "ui")]
mod ui {
    use super::*;
    use crate::profile_ui::UiService;

    struct Directory(std::path::PathBuf);

    impl Directory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("zeroshot-target-ui-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir(&path).assert_value();
            Self(path)
        }

        fn service(&self, address: std::net::SocketAddr) -> UiService {
            UiService::for_target(
                self.0.clone(),
                &format!("http://{address}"),
                crate::native_v2_observability::NativeV2Observability::new(Arc::new(
                    crate::v2_run_ledger::fake::FakeRunLedger::new(),
                )),
            )
            .assert_value()
        }

        fn key(&self) -> TargetBootstrapKey {
            let path = self.0.join("key");
            std::fs::write(&path, "07".repeat(32)).assert_value();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .assert_value();
            }
            TargetBootstrapKey::load_and_unlink(&path).assert_value()
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn direct_ui_shares_listener_without_activating_another_controller() {
        let directory = Directory::new();
        let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
        let address = listener.local_addr().assert_value();
        let factory = Arc::new(FakeFactory::default());
        let server = Arc::new(
            NativeV2TargetServer::new_direct(
                Arc::new(NativeV2TargetAuthority::new(factory.clone())),
                identity(),
                format!("ws://{address}{OECP_PATH}"),
            )
            .assert_value()
            .with_ui(directory.service(address))
            .assert_value(),
        );
        let task = tokio::spawn(server.serve(listener));
        let client = reqwest::Client::new();
        let page = client
            .get(format!("http://{address}/ui/?embed=1"))
            .send()
            .await
            .assert_value();
        assert_eq!(page.status(), reqwest::StatusCode::OK);
        assert!(page.text().await.assert_value().contains("id=\"root\""));
        let bootstrap = client
            .get(format!("http://{address}/ui/api/bootstrap"))
            .send()
            .await
            .assert_value();
        assert_eq!(bootstrap.status(), reqwest::StatusCode::OK);
        assert_eq!(bootstrap.json::<Value>().await.assert_value()["version"], 1);
        assert_eq!(factory.controllers.load(Ordering::SeqCst), 0);

        assert_eq!(
            http(address, TestHttpRequest::empty("GET", DISCOVERY_PATH, None))
                .await
                .status,
            200
        );
        let mut stream = TcpStream::connect(address).await.assert_value();
        let pipelined = format!(
            "GET /ui/api/bootstrap HTTP/1.1\r\nHost: {address}\r\n\r\nPOST {SESSION_PATH} HTTP/1.1\r\nHost: {address}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        );
        stream.write_all(pipelined.as_bytes()).await.assert_value();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
            .await
            .assert_value()
            .assert_value();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        assert_eq!(
            factory.controllers.load(Ordering::SeqCst),
            0,
            "UI keep-alive cannot route into native session creation"
        );
        task.abort();
        let _ = task.await;
    }

    #[tokio::test]
    async fn private_and_hosted_targets_cannot_mount_or_expose_standalone_ui() {
        let directory = Directory::new();
        for private in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
            let address = listener.local_addr().assert_value();
            let factory = Arc::new(FakeFactory::default());
            let target = Arc::new(NativeV2TargetAuthority::new(factory.clone()));
            let endpoint = format!("ws://{address}{OECP_PATH}");
            let server = if private {
                NativeV2TargetServer::new_private(
                    target.clone(),
                    identity(),
                    &endpoint,
                    directory.key(),
                )
            } else {
                NativeV2TargetServer::new_hosted(target.clone(), Arc::new(FakeSessions), &endpoint)
            }
            .assert_value();
            assert!(
                server
                    .with_ui(directory.service(address))
                    .is_err_and(|error| error.kind() == TargetAuthorityErrorKind::Invalid)
            );

            let server = if private {
                NativeV2TargetServer::new_private(target, identity(), &endpoint, directory.key())
            } else {
                NativeV2TargetServer::new_hosted(target, Arc::new(FakeSessions), &endpoint)
            }
            .assert_value();
            let task = tokio::spawn(Arc::new(server).serve(listener));
            for path in ["/ui/", "/ui/api/profiles", "/ui/api/runs?after=v2:0"] {
                http(
                    address,
                    TestHttpRequest::empty("GET", path, Some("control-token")),
                )
                .await
                .assert_problem(
                    404,
                    "request.not_found",
                    "target route was not found",
                );
            }
            assert_eq!(factory.controllers.load(Ordering::SeqCst), 0);
            task.abort();
            let _ = task.await;
        }
    }
}
