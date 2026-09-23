use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

#[test]
fn controller_descriptor_reads_only_the_exact_workspace_recovery_capability() {
    let origin = Url::parse("http://127.0.0.1:8080").assert_value();
    let advertised = build_controller_descriptor(
        &origin,
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_workspace_recovery(),
        TargetAuthentication::None,
    )
    .assert_value();
    assert!(advertised.workspace_recovery);

    let mut absent = TargetDiscoveryDocument::direct(TargetAuthentication::None);
    absent.extensions.workspace_recovery = None;
    let absent =
        build_controller_descriptor(&origin, absent, TargetAuthentication::None).assert_value();
    assert!(!absent.workspace_recovery);

    let mut incompatible =
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_workspace_recovery();
    incompatible
        .extensions
        .workspace_recovery
        .as_mut()
        .assert_value()
        .kind = "openengine.workspace-recovery/v2".to_owned();
    let error = build_controller_descriptor(&origin, incompatible, TargetAuthentication::None)
        .assert_error();
    assert_eq!(
        error.to_string(),
        "workspace-recovery discovery is incompatible"
    );
}

#[test]
fn controller_descriptor_requires_the_exact_checkpoint_capability_version() {
    let origin = Url::parse("http://127.0.0.1:8080").assert_value();
    let document =
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_workspace_checkpoints();
    let advertised =
        build_controller_descriptor(&origin, document.clone(), TargetAuthentication::None)
            .assert_value();
    assert!(advertised.workspace_checkpoints);
    assert!(!advertised.workspace_recovery);
    let absent = build_controller_descriptor(
        &origin,
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_workspace_recovery(),
        TargetAuthentication::None,
    )
    .assert_value();
    assert!(!absent.workspace_checkpoints);
    let mut incompatible = document;
    incompatible
        .extensions
        .workspace_checkpoints
        .as_mut()
        .assert_value()
        .kind = "openengine.workspace-checkpoints/v2".to_owned();
    let error = build_controller_descriptor(&origin, incompatible, TargetAuthentication::None)
        .assert_error();
    assert_eq!(
        error.to_string(),
        "workspace-checkpoints discovery is incompatible"
    );
}
