use super::*;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

#[cfg(unix)]
use futures_util::FutureExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

struct Storage(std::path::PathBuf);

#[cfg(unix)]
const DIRECT_SERVE_CHILD: &str = "ZEROSHOT_TEST_DIRECT_SERVE_CHILD";

impl Drop for Storage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn relative_storage_has_an_initialized_ledger_before_accepting_connections() {
    let storage = Storage(
        std::path::PathBuf::from("target").join(format!("target-serve-{}", uuid::Uuid::now_v7())),
    );
    let config = TargetServe {
        listen: "127.0.0.1:0".parse().assert_value(),
        public_origin: "http://127.0.0.1:8080".to_owned(),
        storage: storage.0.clone(),
        bootstrap_key_file: None,
    };
    let (server, listener) = prepare_server(&config, &config.public_origin)
        .await
        .assert_value();
    assert!(storage.0.join("runs.sqlite3").is_file());
    let ledger = zeroshot_engine::v2_run_ledger::sqlite::SqliteRunLedger::open_read_only(
        storage.0.join("runs.sqlite3"),
    )
    .assert_value();
    use zeroshot_engine::v2_run_ledger::RunLedger;
    assert!(ledger.list().await.assert_value().is_empty());
    drop(ledger);
    drop(listener);
    drop(server);
}

#[test]
fn target_serve_derives_only_same_authority_websocket_endpoints() {
    assert_eq!(
        oecp_endpoint("http://127.0.0.1:8080").assert_value(),
        "ws://127.0.0.1:8080/native-v2/oecp"
    );
    assert_eq!(
        oecp_endpoint("https://target.example").assert_value(),
        "wss://target.example/native-v2/oecp"
    );
    assert_eq!(
        oecp_endpoint("http://[::1]:8080").assert_value(),
        "ws://[::1]:8080/native-v2/oecp"
    );
    for invalid in [
        "not a URL",
        "ftp://target.example",
        "ws://target.example",
        "file:///target",
    ] {
        assert_eq!(
            oecp_endpoint(invalid).assert_error().to_string(),
            "public target origin is invalid"
        );
    }

    let identity = direct_identity();
    assert_eq!(identity.principal().as_str(), "direct-target");
    assert_eq!(identity.tenant().as_str(), "direct-target");
    assert_eq!(identity.issued_at_ms(), None);
    assert_eq!(identity.expires_at_ms(), u64::MAX);
    assert!(identity.binding_attributes().iter().next().is_none());
}

#[tokio::test]
async fn direct_serve_rejects_an_invalid_public_origin_before_preparing_storage() {
    let storage =
        openengine_cluster_testkit::TemporaryDirectory::for_test("target-serve-invalid-origin");
    for (case, origin) in [
        ("non-loopback-http", "http://target.example"),
        ("credentials", "https://user@target.example"),
        ("path", "https://target.example/private"),
        ("query", "https://target.example?token=private"),
    ] {
        let path = storage.path(case);
        let error = serve_direct_target(TargetServe {
            listen: "127.0.0.1:0".parse().assert_value(),
            public_origin: origin.to_owned(),
            storage: path.clone(),
            bootstrap_key_file: None,
        })
        .await
        .assert_error();
        assert!(matches!(error, TargetServeError::InvalidOrigin(_)));
        assert!(!path.exists(), "invalid origin prepared storage for {case}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn direct_serve_shuts_down_cleanly_on_process_signal() {
    if let Some(storage) = std::env::var_os(DIRECT_SERVE_CHILD) {
        serve_direct_target(TargetServe {
            listen: "127.0.0.1:0".parse().assert_value(),
            public_origin: "http://127.0.0.1:8080".to_owned(),
            storage: storage.into(),
            bootstrap_key_file: None,
        })
        .await
        .assert_value();
        return;
    }

    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("target-serve-signal-shutdown");
    let storage = root.path("storage");
    let mut command = tokio::process::Command::new(std::env::current_exe().assert_value());
    let null_stdio = std::process::Stdio::null;
    command
        .arg("direct_serve_shuts_down_cleanly_on_process_signal")
        .env(DIRECT_SERVE_CHILD, &storage)
        .stdin(null_stdio())
        .stdout(null_stdio())
        .stderr(null_stdio())
        .kill_on_drop(true);
    let mut child = command.spawn().assert_value();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !storage.join("runs.sqlite3").is_file() {
            assert!(
                child.try_wait().assert_value().is_none(),
                "direct target stopped during preparation"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value();

    let pid = i32::try_from(child.id().assert_value()).assert_value();
    // SAFETY: pid identifies the live child owned by this test; SIGTERM is handled by the server.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
            .await
            .assert_value()
            .assert_value()
            .success()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_waits_for_an_explicit_process_signal() {
    assert!(shutdown_signal().assert_value().now_or_never().is_none());
}

#[tokio::test]
async fn direct_serve_surfaces_hosting_preparation_failures() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("target-serve-hosting-refusal");
    let storage = root.path("storage-blocker");
    std::fs::write(&storage, b"not a storage directory").assert_value();

    let error = serve_direct_target(TargetServe {
        listen: "127.0.0.1:0".parse().assert_value(),
        public_origin: "http://127.0.0.1:8080".to_owned(),
        storage,
        bootstrap_key_file: None,
    })
    .await
    .assert_error();
    assert!(matches!(error, TargetServeError::Hosting(_)));
}

#[tokio::test]
async fn preparation_fails_closed_before_hosting_for_invalid_private_inputs() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "target-serve-preparation-refusal",
    );
    let occupied = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let cases = [
        (
            TargetServe {
                listen: "127.0.0.1:0".parse().assert_value(),
                public_origin: "http://127.0.0.1:8080".to_owned(),
                storage: root.path("missing-key-storage"),
                bootstrap_key_file: Some(root.path("missing-bootstrap-key")),
            },
            "authority",
        ),
        (
            TargetServe {
                listen: occupied.local_addr().assert_value(),
                public_origin: "http://127.0.0.1:8080".to_owned(),
                storage: root.path("occupied-listener-storage"),
                bootstrap_key_file: None,
            },
            "io",
        ),
        (
            TargetServe {
                listen: "127.0.0.1:0".parse().assert_value(),
                public_origin: "ftp://127.0.0.1:8080".to_owned(),
                storage: root.path("invalid-endpoint-storage"),
                bootstrap_key_file: None,
            },
            "authority",
        ),
    ];
    for (config, expected) in cases {
        let error = prepare_server(&config, &config.public_origin)
            .await
            .assert_error();
        assert!(
            matches!(
                (&error, expected),
                (TargetServeError::Authority(_), "authority") | (TargetServeError::Io(_), "io")
            ),
            "unexpected {expected} error: {error}"
        );
        assert!(!config.storage.exists());
    }
}

#[tokio::test]
async fn private_preparation_consumes_the_bootstrap_key_before_serving() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "target-serve-private-preparation",
    );
    let bootstrap_key = root.path("bootstrap-key");
    std::fs::write(&bootstrap_key, "07".repeat(32)).assert_value();
    #[cfg(unix)]
    std::fs::set_permissions(&bootstrap_key, std::fs::Permissions::from_mode(0o600)).assert_value();
    let config = TargetServe {
        listen: "127.0.0.1:0".parse().assert_value(),
        public_origin: "http://127.0.0.1:8080".to_owned(),
        storage: root.path("storage"),
        bootstrap_key_file: Some(bootstrap_key.clone()),
    };

    let (server, listener) = prepare_server(&config, &config.public_origin)
        .await
        .assert_value();
    assert!(!bootstrap_key.exists());
    assert!(config.storage.join("runs.sqlite3").is_file());
    drop(listener);
    drop(server);
}

#[tokio::test]
async fn prepared_target_reaches_a_live_listener_and_remains_active_until_cancelled() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("target-serve-live-listener");
    let storage = root.path("storage");
    let config = TargetServe {
        listen: "127.0.0.1:0".parse().assert_value(),
        public_origin: "http://127.0.0.1:8080".to_owned(),
        storage: storage.clone(),
        bootstrap_key_file: None,
    };
    let (server, listener) = prepare_server(&config, &config.public_origin)
        .await
        .assert_value();
    let listen = listener.local_addr().assert_value();
    let task = tokio::spawn(serve_prepared(server, listener, std::future::pending()));

    let stream = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match tokio::net::TcpStream::connect(listen).await {
                Ok(stream) => return stream,
                Err(_) if !task.is_finished() => tokio::task::yield_now().await,
                Err(error) => panic!("direct target stopped before listening: {error}"),
            }
        }
    })
    .await
    .assert_value();
    assert!(storage.join("runs.sqlite3").is_file());
    assert!(!task.is_finished());
    drop(stream);

    task.abort();
    assert!(task.await.assert_error().is_cancelled());
}
