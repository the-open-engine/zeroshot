use std::sync::Arc;
use std::time::Duration;

use fs2::FileExt;
use openengine_cluster_protocol::{RunId, TargetAuthentication, TargetDiscoveryDocument};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use tokio::sync::Notify;
use zeroshot_engine::native_v2_cli::oecp::TargetConnector;

use super::super::fixtures::{direct_target, hosted_target, run_request, temp_root, FakeDialer};
use super::super::hosted_authority::{
    authority_response, bind_target_authority, read_http_request, write_http_response,
};
use super::*;

#[tokio::test]
async fn login_rejects_a_deleted_or_replaced_selection_before_contacting_the_target() {
    use super::super::fixtures::FakeAuthority;

    for replacement in [None, Some(false), Some(true)] {
        let root = temp_root();
        let registry = FileTargetRegistry::new(root.path("targets.json"));
        let target = hosted_target("cloud", "https://old.example");
        registry.insert(target.clone()).assert_value();
        let selected = registry.get("cloud").assert_value();
        registry
            .remove("cloud", &test_http_authority(root.path("auth")))
            .await
            .assert_value();
        if let Some(preserve_id) = replacement {
            let mut current = target.clone();
            current.origin = "https://new.example".to_owned();
            if !preserve_id {
                current.id = "22222222-2222-4222-8222-222222222222".to_owned();
            }
            registry.insert(current).assert_value();
        }
        let authority = FakeAuthority::new("ws://127.0.0.1:1");
        assert!(matches!(
            registry.login(&selected, &authority).await,
            Err(TargetConnectorError::RegistrationChanged)
        ));
        assert!(authority.calls().is_empty());
    }
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn removal_waits_for_login_paused_during_discovery_and_cleans_its_credentials() {
    let root = temp_root();
    let (listener, address, origin) = bind_target_authority().await;
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let server_started = started.clone();
    let server_release = release.clone();
    let server_origin = origin.clone();
    let server = tokio::spawn(async move {
        let mut token_index = 0;
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            if request.path == "/.well-known/zeroshot-native-v2" {
                server_started.notify_one();
                server_release.notified().await;
            }
            let body = authority_response(&request, &server_origin, address, &mut token_index);
            write_http_response(&mut stream, &body).await;
        }
    });
    let registry = FileTargetRegistry::new(root.path("targets.json"));
    let target = hosted_target("cloud", origin);
    registry.insert(target.clone()).assert_value();
    let credentials = Arc::new(MemoryCredentialStore::default());
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("auth"),
    );
    let connector =
        NativeV2TargetConnector::new(registry.clone(), authority.clone(), FakeDialer::default());
    let login = tokio::spawn(async move { connector.login("cloud").await });
    started.notified().await;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path("targets.lock"))
        .assert_value();
    let login_holds_registry = matches!(
        lock.try_lock_exclusive(),
        Err(error) if error.kind() == fs2::lock_contended_error().kind()
    );
    drop(lock);
    let removal_registry = registry.clone();
    let mut removal =
        tokio::spawn(async move { removal_registry.remove("cloud", &authority).await });
    let removed_during_discovery =
        match tokio::time::timeout(Duration::from_millis(100), &mut removal).await {
            Ok(result) => {
                result.assert_value().assert_value();
                true
            }
            Err(_) => false,
        };
    release.notify_one();
    login.await.assert_value().assert_value();
    if !removed_during_discovery {
        removal.await.assert_value().assert_value();
    }
    server.await.assert_value();
    assert!(login_holds_registry);
    assert!(!removed_during_discovery);
    assert!(matches!(
        registry.get("cloud"),
        Err(TargetConnectorError::NotFound(_))
    ));
    assert_eq!(credentials.get(&target.id).await.assert_value(), None);
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn delayed_submission_receipt_cannot_restore_removed_resume_permissions() {
    let root = temp_root();
    let (listener, _, origin) = bind_target_authority().await;
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let server_started = started.clone();
    let server_release = release.clone();
    let receipt_run = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c992");
    let server_run = receipt_run.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.assert_value();
        let discovery = read_http_request(&mut stream).await;
        assert_eq!(discovery.path, "/.well-known/zeroshot-native-v2");
        write_http_response(
            &mut stream,
            &serde_json::to_string(&TargetDiscoveryDocument::direct(TargetAuthentication::None))
                .assert_value(),
        )
        .await;
        let (mut stream, _) = listener.accept().await.assert_value();
        let submission = read_http_request(&mut stream).await;
        assert_eq!(submission.path, "/native-v2/run");
        server_started.notify_one();
        server_release.notified().await;
        write_http_response(
            &mut stream,
            &serde_json::json!({"runId": server_run}).to_string(),
        )
        .await;
    });
    let registry = FileTargetRegistry::new(root.path("targets.json"));
    let target = direct_target(&origin);
    registry.insert(target.clone()).assert_value();
    let authority = test_http_authority(root.path("auth"));
    let connector =
        NativeV2TargetConnector::new(registry.clone(), authority.clone(), FakeDialer::default());
    let request = run_request();
    let initial_run = request.run_id.clone();
    let submission = tokio::spawn(async move { connector.submit("vm", request).await });
    started.notified().await;
    registry
        .recovery_authorization(&target.id, &initial_run)
        .assert_value();
    registry.remove("vm", &authority).await.assert_value();
    let mut replacement = target.clone();
    replacement.id = "22222222-2222-4222-8222-222222222222".to_owned();
    registry.insert(replacement.clone()).assert_value();
    release.notify_one();
    assert_eq!(
        submission.await.assert_value().assert_value().run_id,
        receipt_run
    );
    server.await.assert_value();
    assert_eq!(registry.get("vm").assert_value(), replacement);
    for run in [&initial_run, &receipt_run] {
        assert!(matches!(
            registry.recovery_authorization(&target.id, run),
            Err(TargetConnectorError::RecoveryAuthorizationUnavailable)
        ));
    }
    assert!(!root.path("targets.recovery").join(&target.id).exists());
    registry
        .record_recovery_authorization(&target.id, &initial_run, &Default::default())
        .assert_error();
    assert!(!root.path("targets.recovery").join(&target.id).exists());
}
