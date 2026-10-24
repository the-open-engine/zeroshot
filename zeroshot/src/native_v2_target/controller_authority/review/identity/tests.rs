use std::sync::Arc;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::native_v2_target::controller_authority::credentials::TargetCredentialStore;
use crate::native_v2_target::controller_authority::credentials::test_support::{
    MemoryCredentialStore, MemoryDeviceCodeNotifier,
};

fn target() -> TargetRecord {
    TargetRecord {
        id: uuid::Uuid::now_v7().to_string(),
        name: "cloud".to_owned(),
        origin: "https://cloud.example".to_owned(),
        access: TargetAccess::Hosted {
            device_token: "device-identity".to_owned(),
        },
    }
}

#[test]
fn namespace_separates_registration_origin_login_and_machine_credentials() {
    let original = target();
    let key = namespace(&original, "human-login-generation").assert_value();
    assert_eq!(key.len(), 64);
    for credential in [
        "new-human-login-generation",
        "machine-one:org-one",
        "machine-two:org-one",
        "machine-one:org-two",
    ] {
        assert_ne!(namespace(&original, credential).assert_value(), key);
    }
    let mut changed = original.clone();
    changed.origin = "https://other.example".to_owned();
    assert_ne!(
        namespace(&changed, "human-login-generation").assert_value(),
        key
    );
    changed = original;
    changed.id = uuid::Uuid::now_v7().to_string();
    assert_ne!(
        namespace(&changed, "human-login-generation").assert_value(),
        key
    );
}

#[tokio::test]
async fn human_identity_survives_refresh_rotation_and_changes_before_login() {
    let directory = tempfile::tempdir().assert_value();
    let credentials = Arc::new(MemoryCredentialStore::default());
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        directory.path().to_owned(),
    );
    let target = target();
    let first = authority
        .review_snapshot_namespace(&target)
        .await
        .assert_value();
    credentials
        .set(&target.id, "rotated-refresh-token")
        .await
        .assert_value();
    assert_eq!(
        authority
            .review_snapshot_namespace(&target)
            .await
            .assert_value(),
        first
    );
    {
        let _guard = authority
            .lock_refresh_family(&target.id)
            .await
            .assert_value();
        authority
            .review_login_generation(&target.id, true)
            .assert_value();
    }
    assert_ne!(
        authority
            .review_snapshot_namespace(&target)
            .await
            .assert_value(),
        first
    );
}

#[tokio::test]
async fn machine_identity_is_isolated_from_human_and_other_machine_selections() {
    let directory = tempfile::tempdir().assert_value();
    let authority = TargetHttpControlAuthority::with_dependencies(
        Arc::new(MemoryCredentialStore::default()),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        directory.path().to_owned(),
    );
    let target = target();
    let human = authority
        .review_snapshot_namespace(&target)
        .await
        .assert_value();
    let machine = authority.clone().with_machine_key("machine-one", None);
    let first = machine
        .review_snapshot_namespace(&target)
        .await
        .assert_value();
    assert_ne!(first, human);
    assert_eq!(
        machine
            .review_snapshot_namespace(&target)
            .await
            .assert_value(),
        first
    );
    let other = authority.clone().with_machine_key("machine-two", None);
    assert_ne!(
        other
            .review_snapshot_namespace(&target)
            .await
            .assert_value(),
        first
    );
    let organization =
        authority.with_machine_key("machine-one", Some("01932da1-8b39-7000-a000-000000000001"));
    assert_ne!(
        organization
            .review_snapshot_namespace(&target)
            .await
            .assert_value(),
        first
    );
}
