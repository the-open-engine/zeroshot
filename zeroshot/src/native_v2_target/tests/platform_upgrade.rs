use std::collections::BTreeMap;
use std::path::Path;

use openengine_cluster_testkit::assertions::AssertValue;

use super::fixtures::temp_root;
use super::super::controller_authority::credentials::test_support::MemoryCredentialStore;
use super::super::controller_authority::TargetCredentialStore;
use super::super::{
    FileTargetRegistry, TargetAccess, TargetConnectorError, TargetRecord, TargetRegistry,
};

fn legacy_builtin() -> TargetRecord {
    TargetRecord {
        id: "11111111-1111-4111-8111-111111111111".to_owned(),
        name: "cloud".to_owned(),
        origin: "https://api.cloud.zeroshot.sh".to_owned(),
        access: TargetAccess::Hosted {
            device_token: "22222222-2222-4222-8222-222222222222".to_owned(),
        },
    }
}

fn store_targets(path: &Path, version: u32, targets: Vec<TargetRecord>) -> Vec<u8> {
    let targets: BTreeMap<_, _> = targets
        .into_iter()
        .map(|target| (target.name.clone(), target))
        .collect();
    let bytes = serde_json::to_vec(&serde_json::json!({
        "version": version,
        "targets": targets,
    }))
    .assert_value();
    std::fs::create_dir_all(path.parent().assert_value()).assert_value();
    std::fs::write(path, &bytes).assert_value();
    bytes
}

#[tokio::test]
async fn platform_upgrade_preserves_login_and_device_identity_and_runs_once() {
    for version in [4, 5] {
        let root = temp_root();
        let path = root.path("config/targets.json");
        let legacy = legacy_builtin();
        let credentials = MemoryCredentialStore::default();
        credentials
            .set(&legacy.id, "existing-refresh-token")
            .await
            .assert_value();
        store_targets(&path, version, vec![legacy.clone()]);

        let platform = FileTargetRegistry::new(path.clone())
            .get("platform")
            .assert_value();
        let mut expected = legacy;
        expected.name = "platform".to_owned();
        assert_eq!(platform, expected);
        assert_eq!(
            credentials
                .get(&platform.id)
                .await
                .assert_value()
                .as_deref(),
            Some("existing-refresh-token")
        );
        let upgraded = std::fs::read(&path).assert_value();
        let restarted = FileTargetRegistry::new(path.clone());
        assert_eq!(restarted.get("platform").assert_value(), platform);
        assert!(matches!(
            restarted.get("cloud"),
            Err(TargetConnectorError::NotFound(_))
        ));
        assert_eq!(std::fs::read(&path).assert_value(), upgraded);
        let stored: serde_json::Value = serde_json::from_slice(&upgraded).assert_value();
        assert_eq!(stored["version"], 5);
        assert!(stored["targets"].get("cloud").is_none());
    }
}

#[test]
fn platform_upgrade_conflict_preserves_both_records_and_original_file() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    let mut customized = legacy_builtin();
    customized.name = "platform".to_owned();
    customized.origin = "https://custom.example".to_owned();
    let before = store_targets(&path, 5, vec![legacy_builtin(), customized]);
    assert!(matches!(
        FileTargetRegistry::new(path.clone()).get("platform"),
        Err(TargetConnectorError::PlatformTargetUpgradeConflict)
    ));
    assert_eq!(std::fs::read(path).assert_value(), before);
}

#[test]
fn platform_upgrade_keeps_customized_legacy_targets() {
    let mut other_origin = legacy_builtin();
    other_origin.origin = "https://custom.example".to_owned();
    let mut direct = legacy_builtin();
    direct.access = TargetAccess::Direct;
    for customized in [other_origin, direct] {
        let root = temp_root();
        let path = root.path("config/targets.json");
        store_targets(&path, 5, vec![customized.clone()]);
        let registry = FileTargetRegistry::new(path);
        assert_eq!(registry.get("cloud").assert_value(), customized);
        let platform = registry.get("platform").assert_value();
        assert_ne!(platform.id, customized.id);
        assert_eq!(platform.origin, "https://api.cloud.zeroshot.sh");
        assert!(matches!(platform.access, TargetAccess::Hosted { .. }));
    }
}

#[test]
fn platform_upgrade_keeps_an_existing_platform_registration() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    let mut existing = legacy_builtin();
    existing.name = "platform".to_owned();
    existing.origin = "https://custom.example".to_owned();
    let before = store_targets(&path, 5, vec![existing.clone()]);
    assert_eq!(
        FileTargetRegistry::new(path.clone())
            .get("platform")
            .assert_value(),
        existing
    );
    assert_eq!(std::fs::read(path).assert_value(), before);
}

#[test]
fn platform_upgrade_is_serialized_between_cli_processes() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    store_targets(&path, 5, vec![legacy_builtin()]);
    let other_path = path.clone();
    let other = std::thread::spawn(move || {
        FileTargetRegistry::new(other_path)
            .get("platform")
            .assert_value()
    });
    let first = FileTargetRegistry::new(path).get("platform").assert_value();
    assert_eq!(first, other.join().assert_value());
}
