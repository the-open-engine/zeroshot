use super::*;
use crate::native_v2_target::{FileTargetRegistry, TargetHttpControlAuthority, TargetRegistry};
use super::super::test_support::MemoryDeviceCodeNotifier;

fn registered_target(root: &openengine_cluster_testkit::TemporaryDirectory) -> FileTargetRegistry {
    let registry = FileTargetRegistry::new(root.path("targets.json"));
    let mut target = registry.get("platform").assert_value();
    target.name = "cloud".to_owned();
    target.id = TARGET_ID.to_owned();
    registry.insert(target).assert_value();
    registry
}

#[tokio::test]
async fn system_to_file_switch_removal_cleans_both_stores_after_restart() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("zeroshot-both-stores");
    let directory = root.path("credentials");
    let system = Arc::new(MemoryCredentialStore::default());
    let first = LinuxTargetCredentialStore::with_dependencies(
        directory.clone(),
        system.clone(),
        Some("system"),
        true,
    )
    .assert_value();
    first.prepare_for_login(TARGET_ID).await.assert_value();
    first
        .set(TARGET_ID, "earlier-system-login")
        .await
        .assert_value();
    let second = LinuxTargetCredentialStore::with_dependencies(
        directory.clone(),
        system.clone(),
        Some("file"),
        false,
    )
    .assert_value();
    prepare_and_store(&second).await;
    assert_eq!(
        system.get(TARGET_ID).await.assert_value().as_deref(),
        Some("earlier-system-login")
    );

    let other_id = "22222222-2222-4222-8222-222222222222";
    system
        .set(other_id, "unrelated-system-login")
        .await
        .assert_value();
    let file = PrivateFileTargetCredentialStore::new(directory.clone());
    file.set(other_id, "unrelated-file-login")
        .await
        .assert_value();
    let restarted = Arc::new(
        LinuxTargetCredentialStore::with_dependencies(
            directory.clone(),
            system.clone(),
            None,
            true,
        )
        .assert_value(),
    );
    assert_eq!(
        restarted.get(TARGET_ID).await.assert_value().as_deref(),
        Some("refresh-token")
    );
    let registry = registered_target(&root);
    let authority = TargetHttpControlAuthority::with_dependencies(
        restarted,
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("auth"),
    );
    registry.remove("cloud", &authority).await.assert_value();
    assert_eq!(system.get(TARGET_ID).await.assert_value(), None);
    assert_eq!(file.get(TARGET_ID).await.assert_value(), None);
    assert_eq!(file.read_backend(TARGET_ID).await.assert_value(), None);
    assert!(registry.get("cloud").is_err());
    assert_eq!(
        system.get(other_id).await.assert_value().as_deref(),
        Some("unrelated-system-login")
    );
    assert_eq!(
        file.get(other_id).await.assert_value().as_deref(),
        Some("unrelated-file-login")
    );
}

#[tokio::test]
async fn legacy_file_selection_requires_keyring_cleanup_and_retains_retry_state_on_failure() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("zeroshot-legacy-stores");
    let directory = root.path("credentials");
    let system = Arc::new(MemoryCredentialStore::default());
    system
        .set(TARGET_ID, "legacy-system-login")
        .await
        .assert_value();
    let file = PrivateFileTargetCredentialStore::new(directory.clone());
    file.set(TARGET_ID, "legacy-file-login")
        .await
        .assert_value();
    // Older versions recorded only the selected store, losing evidence of the earlier keyring login.
    file.write_backend(TARGET_ID, "file\n").await.assert_value();
    let registry = registered_target(&root);
    let run = openengine_cluster_protocol::RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
    registry
        .record_recovery_authorization(TARGET_ID, &run, &Default::default())
        .assert_value();
    let before = std::fs::read(root.path("targets.json")).assert_value();
    let unavailable = Arc::new(
        LinuxTargetCredentialStore::with_dependencies(
            directory.clone(),
            Arc::new(UnavailableCredentialStore),
            Some("file"),
            false,
        )
        .assert_value(),
    );
    let authority = TargetHttpControlAuthority::with_dependencies(
        unavailable,
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("auth"),
    );
    registry.remove("cloud", &authority).await.assert_error();
    assert_eq!(
        std::fs::read(root.path("targets.json")).assert_value(),
        before
    );
    assert_eq!(
        file.get(TARGET_ID).await.assert_value().as_deref(),
        Some("legacy-file-login")
    );
    assert_eq!(
        file.read_backend(TARGET_ID).await.assert_value().as_deref(),
        Some("file\n")
    );
    registry
        .recovery_authorization(TARGET_ID, &run)
        .assert_value();

    let available = Arc::new(
        LinuxTargetCredentialStore::with_dependencies(
            directory,
            system.clone(),
            Some("file"),
            false,
        )
        .assert_value(),
    );
    let authority = TargetHttpControlAuthority::with_dependencies(
        available,
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("auth"),
    );
    registry.remove("cloud", &authority).await.assert_value();
    assert_eq!(system.get(TARGET_ID).await.assert_value(), None);
    assert_eq!(file.get(TARGET_ID).await.assert_value(), None);
    assert_eq!(file.read_backend(TARGET_ID).await.assert_value(), None);
    assert!(registry.get("cloud").is_err());
    assert!(registry.recovery_authorization(TARGET_ID, &run).is_err());
}
