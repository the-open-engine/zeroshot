use std::sync::Arc;

use fs2::FileExt;
use openengine_cluster_protocol::RunId;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use zeroshot_engine::native_v2_cli::oecp::TargetConnector;

use super::super::*;
use super::fixtures::{direct_target, hosted_target, temp_root, FakeDialer, test_http_authority};
use super::super::controller_authority::credentials::test_support::{
    MemoryCredentialStore, MemoryDeviceCodeNotifier, UnavailableCredentialStore,
};
use super::super::controller_authority::credentials::TargetCredentialStore;

#[path = "management/concurrency.rs"]
mod concurrency;

#[tokio::test]
async fn target_management_renders_human_and_json_results_through_the_cli() {
    use zeroshot_engine::native_v2_cli::{parse_native_v2_args, execute_native_v2_cli, NeverDetach};
    use zeroshot_engine::native_v2_cli::oecp::NamedTargetCliBackend;

    for json in [false, true] {
        let root = temp_root();
        let registry = FileTargetRegistry::new(root.path("targets.json"));
        registry
            .insert(direct_target("http://127.0.0.1:1"))
            .assert_value();
        let connector = NativeV2TargetConnector::new(
            registry,
            test_http_authority(root.path("auth")),
            FakeDialer::default(),
        );
        let backend = NamedTargetCliBackend::new(connector);
        for mut arguments in [vec!["target", "list"], vec!["target", "remove", "vm"]] {
            if json {
                arguments.push("--json");
            }
            let command =
                parse_native_v2_args(arguments.iter().map(std::ffi::OsString::from)).assert_value();
            let mut output = Vec::new();
            execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut output)
                .await
                .assert_value();
            let text = String::from_utf8(output).assert_value();
            if json {
                let value: serde_json::Value = serde_json::from_str(&text).assert_value();
                if arguments[1] == "list" {
                    assert_eq!(value.as_array().assert_value().len(), 2);
                    assert_eq!(
                        value[1],
                        serde_json::json!({"name":"vm", "origin":"http://127.0.0.1:1", "kind":"direct"})
                    );
                } else {
                    assert_eq!(value, serde_json::json!({"name":"vm", "removed":true}));
                }
            } else if arguments[1] == "list" {
                assert!(text.contains("platform\thosted\thttps://api.app.zeroshot.sh"));
                assert!(text.contains("vm\tdirect\thttp://127.0.0.1:1"));
            } else {
                assert_eq!(text, "Removed target \"vm\".\n");
            }
        }
    }
}

#[tokio::test]
async fn inventory_is_sorted_complete_and_secret_free_without_target_contact() {
    let root = temp_root();
    let registry = FileTargetRegistry::new(root.path("targets.json"));
    let hosted = hosted_target("cloud", "https://unreachable.example");
    let direct = direct_target("http://127.0.0.1:1");
    registry.insert(direct).assert_value();
    registry.insert(hosted.clone()).assert_value();
    let connector = NativeV2TargetConnector::new(
        registry,
        test_http_authority(root.path("auth")),
        FakeDialer::default(),
    );
    let inventory = serde_json::to_value(connector.list().await.assert_value()).assert_value();
    assert_eq!(
        inventory,
        serde_json::json!([
            {"name":"cloud", "origin":"https://unreachable.example", "kind":"hosted"},
            {"name":"platform", "origin":"https://api.app.zeroshot.sh", "kind":"hosted"},
            {"name":"vm", "origin":"http://127.0.0.1:1", "kind":"direct"},
        ])
    );
    assert!(
        !inventory
            .to_string()
            .contains(hosted.access.device_token().assert_value())
    );
}

#[tokio::test]
async fn removal_cleans_only_selected_target_and_allows_registration_again() {
    let root = temp_root();
    let registry = FileTargetRegistry::new(root.path("targets.json"));
    let target = direct_target("http://127.0.0.1:1");
    let mut other = hosted_target("other", "https://other.example");
    other.id = "22222222-2222-4222-8222-222222222222".to_owned();
    registry.insert(target.clone()).assert_value();
    registry.insert(other.clone()).assert_value();
    let run = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
    for target in [&target, &other] {
        registry
            .record_recovery_authorization(&target.id, &run, &Default::default())
            .assert_value();
    }
    let authority = test_http_authority(root.path("auth"));
    registry
        .remove(&target.name, &authority)
        .await
        .assert_value();
    assert!(matches!(
        registry.get(&target.name),
        Err(TargetConnectorError::NotFound(_))
    ));
    assert!(matches!(
        registry.recovery_authorization(&target.id, &run),
        Err(TargetConnectorError::RecoveryAuthorizationUnavailable)
    ));
    assert_eq!(registry.get(&other.name).assert_value(), other);
    registry
        .recovery_authorization(&other.id, &run)
        .assert_value();
    let (origin, server) = super::hosted_authority::spawn_direct_target_authority(1).await;
    let connector = NativeV2TargetConnector::new(registry, authority, FakeDialer::default());
    let missing = connector
        .sizes(&target.name)
        .await
        .assert_error()
        .to_string();
    assert!(missing.contains("not found"));
    connector
        .add(zeroshot_engine::native_v2_cli::TargetAdd {
            name: target.name,
            url: origin,
            direct: true,
        })
        .await
        .assert_value();
    assert_eq!(server.await.assert_value().len(), 1);
}

#[tokio::test]
async fn refusals_leave_registry_credentials_and_permissions_unchanged() {
    let root = temp_root();
    let path = root.path("targets.json");
    let registry = FileTargetRegistry::new(path.clone());
    let target = hosted_target("cloud", "https://cloud.example");
    registry.insert(target.clone()).assert_value();
    let run = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
    registry
        .record_recovery_authorization(&target.id, &run, &Default::default())
        .assert_value();
    let before = std::fs::read(&path).assert_value();
    let authority = TargetHttpControlAuthority::with_dependencies(
        Arc::new(UnavailableCredentialStore),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("auth"),
    );
    let built_in = registry
        .remove("platform", &authority)
        .await
        .assert_error()
        .to_string();
    let missing = registry
        .remove("missing", &authority)
        .await
        .assert_error()
        .to_string();
    assert!(built_in.contains("built-in target"));
    assert!(missing.contains("not found"));
    assert_ne!(built_in, missing);
    registry.remove("cloud", &authority).await.assert_error();
    assert_eq!(std::fs::read(&path).assert_value(), before);
    registry
        .recovery_authorization(&target.id, &run)
        .assert_value();
}

#[tokio::test]
async fn hosted_removal_holds_registry_lock_through_credential_cleanup() {
    struct LockCheckingStore {
        inner: MemoryCredentialStore,
        lock_path: std::path::PathBuf,
    }
    #[async_trait::async_trait]
    impl TargetCredentialStore for LockCheckingStore {
        async fn get(&self, target_id: &str) -> Result<Option<String>, TargetAuthorityError> {
            self.inner.get(target_id).await
        }
        async fn set(&self, target_id: &str, token: &str) -> Result<(), TargetAuthorityError> {
            self.inner.set(target_id, token).await
        }
        async fn remove(&self, target_id: &str) -> Result<(), TargetAuthorityError> {
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.lock_path)
                .assert_value();
            assert!(
                matches!(lock.try_lock_exclusive(), Err(error) if error.kind() == fs2::lock_contended_error().kind())
            );
            self.inner.remove(target_id).await
        }
    }
    let root = temp_root();
    let registry = FileTargetRegistry::new(root.path("targets.json"));
    let target = hosted_target("cloud", "https://unreachable.example");
    registry.insert(target.clone()).assert_value();
    let store = Arc::new(LockCheckingStore {
        inner: MemoryCredentialStore::default(),
        lock_path: root.path("targets.lock"),
    });
    store.set(&target.id, "stored-login").await.assert_value();
    let authority = TargetHttpControlAuthority::with_dependencies(
        store.clone(),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("auth"),
    );
    registry.remove("cloud", &authority).await.assert_value();
    assert_eq!(store.get(&target.id).await.assert_value(), None);
}
