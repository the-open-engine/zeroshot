//! Real loopback TLS and CONNECT proxy coverage; no process environment is mutated.
use super::*;
use crate::source_code_provider::adapters::core::network::ForgeNetwork;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "network_tests/tests/support.rs"]
mod support;
use support::{TlsFixture, ProxyFixture};
#[path = "network_tests/tests/socks.rs"]
mod socks;

#[tokio::test]
async fn local_pr_and_ship_preflight_use_captured_settings_and_explain_refusals() {
    use crate::native_v2_delivery::DeliveryMode;
    for mode in [DeliveryMode::PullRequest, DeliveryMode::Merge] {
        let fixture = TlsFixture::new().await;
        let proxy = ProxyFixture::new(fixture.address).await;
        let mut prepared = fixture.prepared(mode, "test-private-token");
        prepared
            .native_environment
            .insert("HTTPS_PROXY".into(), proxy.url());
        let state = tempfile::tempdir().unwrap();
        crate::native_v2_local::check_forge_access(&prepared, state.path())
            .await
            .unwrap();
        assert_eq!(proxy.count(), 1);
        assert_eq!(fixture.calls().len(), 1);
        let rejected = fixture.prepared(mode, "rejected-token");
        let error = crate::native_v2_local::check_forge_access(&rejected, state.path())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("source.gitlab")
                && error.contains("authenticated project/version read")
                && error.contains("HTTP 401"),
            "{error}"
        );
        assert!(!error.contains("rejected-token"));
        assert!(!state.path().join("runs").exists());
    }
}

#[tokio::test]
async fn preflight_requests_and_diagnostics_use_the_selected_forge_repository() {
    use crate::native_v2_delivery::{DeliveryMode, source_target};
    for mode in [DeliveryMode::PullRequest, DeliveryMode::Merge] {
        let fixture = TlsFixture::new().await;
        let state = tempfile::tempdir().unwrap();
        for token in ["test-private-token", "rejected-token"] {
            let mut prepared = fixture.prepared(mode, token);
            let source = prepared.submission.source.as_mut().unwrap();
            source.repository =
                openengine_cluster_protocol::SourceRepositoryId::new("caller/opaque/source-label")
                    .unwrap();
            let target = source_target(source, mode).unwrap();
            assert_eq!(target.repository, REPOSITORY);
            let result = crate::native_v2_local::check_forge_access(&prepared, state.path()).await;
            if token == "test-private-token" {
                result.unwrap();
            } else {
                let error = result.unwrap_err().to_string();
                assert!(
                    error.contains(&format!("for project {}:", target.repository)),
                    "{error}"
                );
                assert!(!error.contains("caller/opaque/source-label"), "{error}");
            }
        }
        let calls = fixture.calls();
        assert_eq!(
            calls.len(),
            2,
            "each preflight remains one read-only request"
        );
        assert!(
            calls
                .iter()
                .all(|(_, _, body)| body["variables"]["project"] == REPOSITORY)
        );
        assert!(!state.path().join("runs").exists());
    }
}

#[tokio::test]
async fn captured_socks_proxy_supports_preflight_delivery_and_no_proxy() {
    for scheme in ["socks5", "socks5h"] {
        for bypass in [false, true] {
            let fixture = TlsFixture::new().await;
            let proxy = socks::SocksFixture::new(fixture.address).await;
            let mut prepared = fixture.prepared(
                crate::native_v2_delivery::DeliveryMode::Merge,
                "test-private-token",
            );
            prepared
                .native_environment
                .insert("ALL_PROXY".into(), proxy.url(scheme));
            if bypass {
                prepared
                    .native_environment
                    .insert("NO_PROXY".into(), "127.0.0.1".into());
            }
            let state = tempfile::tempdir().unwrap();
            crate::native_v2_local::check_forge_access(&prepared, state.path())
                .await
                .unwrap();
            assert_eq!(fixture.calls().len(), 1);
            assert_eq!(proxy.count(), usize::from(!bypass));
            let authority = fixture.authority(&prepared.native_environment);
            let receipt = authority
                .open_or_update_review(&request(), credential())
                .await
                .unwrap();
            assert_eq!(
                authority
                    .request_merge(&receipt, credential(), &TestReviewCheckpoint)
                    .await
                    .unwrap(),
                SourceMergeRequestOutcome::Accepted,
            );
            assert_eq!(
                proxy.count() > 1,
                !bypass,
                "delivery must also use the captured proxy"
            );
            assert!(
                fixture
                    .calls()
                    .iter()
                    .any(|(method, path, _)| method == "PUT" && path.ends_with("/merge"))
            );
        }
    }
}

#[tokio::test]
async fn native_github_cli_certificate_trust_matches_the_platform() {
    use crate::source_code_provider::adapters::core::{GhCliAuthorityConfig, GhCliDeliveryAuthority};
    let fixture = TlsFixture::new().await;
    let program = std::env::var_os("ZEROSHOT_GH_TEST")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            crate::execution::platform::find_executable(
                "gh",
                &std::env::vars().collect(),
                &std::env::current_dir().unwrap(),
            )
        })
        .expect("native gh is a repository test prerequisite");
    let root = tempfile::tempdir().unwrap();
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program,
        ..GhCliAuthorityConfig::hosted(root.path().into())
    })
    .with_base_url(SourceForgeBaseUrl::new(format!("https://{}", fixture.address)).unwrap())
    .unwrap()
    .with_network(ForgeNetwork::from_snapshot(&fixture.ca_environment()));
    let result = authority
        .check_access("acme/project", credential(), None)
        .await;
    // Go reads SSL_CERT_FILE on Linux. On macOS and Windows it does so from Go 1.27; a `gh` built
    // with an older Go trusts only the system certificate store and refuses the private CA.
    match result {
        Ok(()) => {}
        Err(error) if cfg!(any(target_os = "macos", windows)) => assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("certificate"),
            "an older gh may refuse only the untrusted certificate: {error}"
        ),
        Err(error) => panic!("gh must trust the captured CA file on this platform: {error}"),
    }
}

#[tokio::test]
async fn captured_ca_and_proxy_support_access_and_merge_delivery() {
    for bypass in [false, true] {
        let fixture = TlsFixture::new().await;
        let proxy = ProxyFixture::new(fixture.address).await;
        let mut environment = BTreeMap::from([
            ("HTTPS_PROXY".into(), proxy.url()),
            (
                "SSL_CERT_FILE".into(),
                fixture.ca_file.to_string_lossy().into_owned(),
            ),
        ]);
        if bypass {
            environment.insert("NO_PROXY".into(), "127.0.0.1".into());
        }
        // Build independent authorities, just as preflight and a detached/resumed controller do.
        fixture
            .authority(&environment)
            .check_access(REPOSITORY, credential(), None)
            .await
            .unwrap();
        assert_eq!(
            fixture.calls().len(),
            1,
            "preflight adds one read-only request"
        );
        let authority = fixture.authority(&environment);
        let receipt = authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap();
        let result = authority
            .request_merge(&receipt, credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        assert_eq!(result, SourceMergeRequestOutcome::Accepted);
        assert!(
            fixture
                .calls()
                .iter()
                .any(|(method, path, _)| method == "PUT" && path.ends_with("/merge"))
        );
        assert_eq!(proxy.count() > 0, !bypass);
    }
}

#[tokio::test]
async fn private_ca_is_required_and_certificate_directory_is_honored() {
    let fixture = TlsFixture::new().await;
    let error = fixture
        .authority(&BTreeMap::new())
        .check_access(REPOSITORY, credential(), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("certificate"), "{error}");
    assert!(fixture.calls().is_empty());
    let environment = BTreeMap::from([(
        "SSL_CERT_DIR".into(),
        fixture
            .ca_file
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    )]);
    fixture
        .authority(&environment)
        .check_access(REPOSITORY, credential(), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn access_refuses_rejected_tokens_unreadable_projects_and_unsupported_versions() {
    let fixture = TlsFixture::new().await;
    let environment = fixture.ca_environment();
    let error = fixture
        .authority(&environment)
        .check_access(
            REPOSITORY,
            SourceDeliveryCredential::token("rejected-token"),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.api_status(), Some(401));
    assert!(!error.to_string().contains("rejected-token"));
    let error = fixture
        .authority(&environment)
        .check_access("private/unreadable", credential(), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("token cannot read"));
    for version in ["19.5.0", "unidentifiable"] {
        fixture.inner.state.lock().unwrap().version = version.into();
        let error = fixture
            .authority(&environment)
            .check_access(REPOSITORY, credential(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("19.5.1"));
    }
    assert_eq!(fixture.calls().len(), 4);
    assert!(
        fixture
            .calls()
            .iter()
            .all(|(_, path, body)| path.ends_with("/api/graphql")
                && body["query"].as_str().unwrap().starts_with("query "))
    );
}

#[tokio::test]
async fn lower_case_proxy_and_no_proxy_use_the_snapshot() {
    let fixture = TlsFixture::new().await;
    let proxy = ProxyFixture::new(fixture.address).await;
    for name in ["https_proxy", "ALL_PROXY", "all_proxy"] {
        let mut environment = fixture.ca_environment();
        environment.insert(name.into(), proxy.url());
        fixture
            .authority(&environment)
            .check_access(REPOSITORY, credential(), None)
            .await
            .unwrap();
    }
    assert_eq!(proxy.count(), 3);
    let mut environment = fixture.ca_environment();
    environment.insert("HTTPS_PROXY".into(), String::new());
    environment.insert("https_proxy".into(), proxy.url());
    fixture
        .authority(&environment)
        .check_access(REPOSITORY, credential(), None)
        .await
        .unwrap();
    assert_eq!(proxy.count(), 4);
    let mut environment = fixture.ca_environment();
    environment.insert("HTTPS_PROXY".into(), String::new());
    environment.insert("https_proxy".into(), "http://127.0.0.1:1".into());
    environment.insert("NO_PROXY".into(), String::new());
    environment.insert("no_proxy".into(), "127.0.0.1".into());
    fixture
        .authority(&environment)
        .check_access(REPOSITORY, credential(), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn proxy_credentials_never_enter_transport_errors() {
    let fixture = TlsFixture::new().await;
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut environment = fixture.ca_environment();
    let url = format!(
        "http://proxy-user:proxy-password@{}",
        proxy.local_addr().unwrap()
    );
    environment.insert("HTTPS_PROXY".into(), url.clone());
    let task = tokio::spawn(async move {
        let (mut stream, _) = proxy.accept().await.unwrap();
        let mut bytes = [0; 4096];
        assert!(stream.read(&mut bytes).await.unwrap() > 0);
        stream
            .write_all(format!("HTTP/1.1 407 {url}\r\nContent-Length: 0\r\n\r\n").as_bytes())
            .await
            .unwrap();
    });
    let error = fixture
        .authority(&environment)
        .check_access(REPOSITORY, credential(), None)
        .await
        .unwrap_err();
    let text = format!("{error:?}\n{error}");
    assert!(!text.contains("proxy-user"), "{text}");
    assert!(!text.contains("proxy-password"), "{text}");
    task.await.unwrap();
}

#[tokio::test]
async fn restored_check_reads_the_forge_once_and_stops_with_the_controller() {
    use crate::native_v2_delivery::DeliveryMode;
    use crate::native_v2_local::check_restored_forge_access;
    let fixture = TlsFixture::new().await;
    let state = tempfile::tempdir().unwrap();
    // A new run was already checked by the invoking CLI.
    let mut fresh = fixture.restored(DeliveryMode::PullRequest, state.path());
    fresh.checkpoint = None;
    check_restored_forge_access(&fresh, None).await.unwrap();
    assert!(fixture.calls().is_empty());

    let restored = fixture.restored(DeliveryMode::Merge, state.path());
    check_restored_forge_access(&restored, None).await.unwrap();
    assert_eq!(fixture.calls().len(), 1);

    let (_stop, stopping) = tokio::sync::watch::channel(true);
    let error = check_restored_forge_access(&restored, Some(stopping))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("authenticated project/version read")
            && error.contains("forge access check was cancelled"),
        "{error}"
    );
    assert_eq!(
        fixture.calls().len(),
        1,
        "a stopped check must not reach the forge"
    );
}

#[tokio::test]
async fn preflight_refusals_name_the_client_setup_and_authentication_steps() {
    use crate::native_v2_delivery::DeliveryMode;
    use crate::native_v2_local::check_forge_access;
    let fixture = TlsFixture::new().await;
    let state = tempfile::tempdir().unwrap();
    let mut unreadable = fixture.prepared(DeliveryMode::PullRequest, "test-private-token");
    unreadable.native_environment.insert(
        "SSL_CERT_FILE".into(),
        state
            .path()
            .join("missing-ca.pem")
            .to_string_lossy()
            .into_owned(),
    );
    let error = check_forge_access(&unreadable, state.path())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("source.gitlab")
            && error.contains("client setup")
            && error.contains("missing-ca.pem"),
        "{error}"
    );

    let blank = fixture.prepared(DeliveryMode::Merge, " ");
    let error = check_forge_access(&blank, state.path())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("authentication")
            && error.contains("required forge credential GITLAB_TOKEN was not supplied"),
        "{error}"
    );

    let mut sourceless = fixture.prepared(DeliveryMode::PullRequest, "test-private-token");
    sourceless.submission.source = None;
    assert!(check_forge_access(&sourceless, state.path()).await.is_err());
    assert!(
        fixture.calls().is_empty(),
        "a refusal before the request must not reach the forge"
    );
}

#[tokio::test]
async fn stop_request_ends_a_stalled_access_request() {
    // The listener accepts and never completes TLS, like a forge that stopped answering.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = SourceForgeBaseUrl::new(format!("https://{}", listener.local_addr().unwrap()));
    let held = tokio::spawn(async move {
        let mut connections = Vec::new();
        while let Ok((connection, _)) = listener.accept().await {
            connections.push(connection);
        }
    });
    let authority = GitLabDeliveryAuthority::new(base.unwrap(), None)
        .unwrap()
        .with_network(ForgeNetwork::from_snapshot(&BTreeMap::new()))
        .unwrap();
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let check = authority.check_access(REPOSITORY, credential(), Some(stopping));
    tokio::pin!(check);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut check)
            .await
            .is_err(),
        "the stalled request must still be pending"
    );
    stop.send(true).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), check)
        .await
        .expect("a stop request must end the stalled request")
        .unwrap_err();
    assert!(
        error.to_string().contains("stopped before it completed"),
        "{error}"
    );
    held.abort();
}

#[test]
fn unreadable_certificate_names_its_path_and_keeps_tls_enabled() {
    let root = tempfile::tempdir().unwrap();
    let environment = BTreeMap::from([(
        "SSL_CERT_FILE".into(),
        root.path()
            .join("missing-ca.pem")
            .to_string_lossy()
            .into_owned(),
    )]);
    let network = ForgeNetwork::from_snapshot(&environment);
    let error = network.client(reqwest::Client::builder()).unwrap_err();
    assert!(error.to_string().contains("missing-ca.pem"));
    assert!(!format!("{network:?}").contains("missing-ca.pem"));
}
