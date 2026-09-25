use super::*;
use openengine_cluster_protocol::{EnvironmentName, EnvironmentReference, RuntimeEnvironment};
use openengine_cluster_testkit::assertions::AssertValue;
use super::super::tests::{profile_request, selector};

fn create(store: &LocalRunProfileStore, name: &str) -> RuntimeEnvironmentResource {
    store
        .save_environment(
            RuntimeEnvironmentSaveRequest {
                id: None,
                name: EnvironmentName::new(name).assert_value(),
                expected_revision: None,
                definition: RuntimeEnvironment {
                    startup: Some("echo first".into()),
                    ..Default::default()
                },
            },
            None,
        )
        .assert_value()
}

#[test]
fn profiles_share_latest_resource_while_resolved_snapshots_remain_immutable() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalRunProfileStore::new(root.path().to_owned());
    let first = create(&store, "shared");
    for name in ["worker", "review"] {
        let mut request = profile_request(name, "worker", false);
        request.runtime = request.runtime.map_environment(|_| {
            Some(EnvironmentReference {
                id: first.id.clone(),
            })
        });
        store.set(request).assert_value();
    }
    let authored = store.show(selector("worker")).assert_value();
    let accepted = store.resolve_profile(selector("worker")).assert_value();
    assert_eq!(
        store.resolve_runtime(&authored.runtime).assert_value(),
        accepted.runtime
    );
    let updated = store
        .save_environment(
            RuntimeEnvironmentSaveRequest {
                id: Some(first.id.clone()),
                name: first.name.clone(),
                expected_revision: Some(first.revision.clone()),
                definition: RuntimeEnvironment {
                    startup: Some("echo updated".into()),
                    ..Default::default()
                },
            },
            None,
        )
        .assert_value();
    assert_eq!(updated.id, first.id);
    assert_ne!(updated.revision, first.revision);
    assert_eq!(
        accepted.runtime.environment().unwrap().startup.as_deref(),
        Some("echo first")
    );
    for name in ["worker", "review"] {
        let next = LocalRunProfileStore::new(root.path().to_owned())
            .resolve_profile(selector(name))
            .assert_value();
        assert_eq!(
            next.runtime.environment().unwrap().startup.as_deref(),
            Some("echo updated")
        );
        assert_eq!(
            store
                .show(selector(name))
                .assert_value()
                .runtime
                .environment()
                .unwrap()
                .id,
            first.id
        );
    }
    assert_eq!(store.environments().assert_value().environments.len(), 1);
    assert!(matches!(
        store.delete_environment(
            RuntimeEnvironmentDeleteRequest {
                id: updated.id.clone(),
                expected_revision: updated.revision.clone()
            },
            None
        ),
        Err(NativeV2CliError::EnvironmentInUse)
    ));
    store.delete(selector("worker")).assert_value();
    store.delete(selector("review")).assert_value();
    store
        .delete_environment(
            RuntimeEnvironmentDeleteRequest {
                id: updated.id,
                expected_revision: updated.revision,
            },
            None,
        )
        .assert_value();
    assert!(matches!(
        store.environment(&first.id),
        Err(NativeV2CliError::EnvironmentMissing(_))
    ));
    let replacement = create(&store, "shared");
    assert_ne!(replacement.id, first.id);
}

#[test]
fn environment_cas_and_reference_checks_fail_before_mutating_the_store() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalRunProfileStore::new(root.path().to_owned());
    let first = create(&store, "initial");
    let update = RuntimeEnvironmentSaveRequest {
        id: Some(first.id.clone()),
        name: first.name.clone(),
        definition: first.definition.clone(),
        expected_revision: Some(first.revision.clone()),
    };
    let current = store.save_environment(update.clone(), None).assert_value();
    assert!(matches!(
        store.save_environment(update, None),
        Err(NativeV2CliError::EnvironmentConflict)
    ));
    assert!(matches!(
        store.delete_environment(
            RuntimeEnvironmentDeleteRequest {
                id: first.id.clone(),
                expected_revision: first.revision
            },
            None
        ),
        Err(NativeV2CliError::EnvironmentConflict)
    ));
    assert_eq!(store.environment(&first.id).assert_value(), current);
    let mut request = profile_request("missing", "worker", false);
    request.runtime = request.runtime.map_environment(|_| {
        Some(EnvironmentReference {
            id: EnvironmentId::new("foreign-resource").assert_value(),
        })
    });
    assert!(matches!(
        store.set(request),
        Err(NativeV2CliError::EnvironmentMissing(_))
    ));
    assert!(store.show(selector("missing")).is_err());
}

#[cfg(feature = "ui")]
#[test]
fn environment_mutations_reject_replaced_workspaces() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalRunProfileStore::new(root.path().to_owned());
    let original = store.workspace_id().assert_value();
    let first = create(&store, "shared");
    std::fs::remove_file(root.path().join("ui-workspace-id")).assert_value();
    let replacement = store.workspace_id().assert_value();
    assert_ne!(original, replacement);
    assert!(matches!(
        store.delete_environment(
            RuntimeEnvironmentDeleteRequest {
                id: first.id,
                expected_revision: first.revision
            },
            Some(&original)
        ),
        Err(NativeV2CliError::EnvironmentWorkspaceChanged)
    ));
}
