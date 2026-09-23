use super::*;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

struct Storage(std::path::PathBuf);

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
