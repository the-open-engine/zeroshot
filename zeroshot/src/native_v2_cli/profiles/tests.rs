use openengine_cluster_testkit::admission::graph_fixture;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

pub(super) fn profile_request(name: &str, node: &str, set_default: bool) -> RunProfileSetRequest {
    RunProfileSetRequest {
        name: RunProfileName::new(name).assert_value(),
        scope: RunProfileScope::User,
        graph: graph_fixture(node, json!({"kind": "null"})),
        runtime: serde_json::from_value(json!({
            "harness": "codex",
            "provider": "openai",
            "size": "small",
            "nodes": {(node): {
                "kind": "agent",
                "model": "gpt-5.6-sol",
            }}
        }))
        .assert_value(),
        set_default,
    }
}

pub(super) fn selector(name: &str) -> RunProfileSelector {
    RunProfileSelector {
        scope: RunProfileScope::User,
        name: RunProfileName::new(name).assert_value(),
    }
}

fn assert_malformed<T>(result: Result<T, NativeV2CliError>) {
    assert!(matches!(
        result,
        Err(NativeV2CliError::Local(message))
            if message == "local profile store is malformed"
    ));
}

fn exercise_default_and_delete_lifecycle(store: &LocalRunProfileStore, edited: &RunProfile) {
    store
        .set_default(RunProfileDefaultRequest {
            scope: RunProfileScope::User,
            name: Some(edited.name.clone()),
        })
        .assert_value();
    let default_alpha = store.show(selector("alpha")).assert_value();
    assert!(default_alpha.is_default);
    assert!(!store.show(selector("beta")).assert_value().is_default);
    assert_ne!(
        profile_revision(&default_alpha).assert_value(),
        profile_revision(edited).assert_value()
    );

    let cleared = store
        .set_default(RunProfileDefaultRequest {
            scope: RunProfileScope::User,
            name: None,
        })
        .assert_value();
    assert_eq!(cleared.name, None);
    assert!(!store.show(selector("alpha")).assert_value().is_default);
    store
        .set_default(RunProfileDefaultRequest {
            scope: RunProfileScope::User,
            name: Some(edited.name.clone()),
        })
        .assert_value();

    let missing_default = RunProfileName::new("missing").assert_value();
    assert!(matches!(
        store.set_default(RunProfileDefaultRequest {
            scope: RunProfileScope::User,
            name: Some(missing_default),
        }),
        Err(NativeV2CliError::Local(message)) if message == "profile missing was not found"
    ));
    assert!(store.show(selector("alpha")).assert_value().is_default);

    assert!(store.delete(selector("beta")).assert_value().deleted);
    assert!(!store.delete(selector("beta")).assert_value().deleted);
    assert!(store.delete(selector("alpha")).assert_value().deleted);
    assert!(matches!(
        store.show(selector("alpha")),
        Err(NativeV2CliError::Local(message)) if message == "profile alpha was not found"
    ));
    assert!(
        store
            .list(RunProfileListRequest {
                scope: RunProfileScope::User,
            })
            .assert_value()
            .profiles
            .is_empty()
    );
}

#[cfg(feature = "ui")]
#[test]
fn workspace_identity_survives_restart_and_move_but_not_store_replacement() {
    let root = std::env::temp_dir().join(format!("zeroshot-workspace-{}", uuid::Uuid::now_v7()));
    let moved = root.with_extension("moved");
    let first = LocalRunProfileStore::new(root.clone())
        .workspace_id()
        .assert_value();
    assert_eq!(
        first,
        LocalRunProfileStore::new(root.clone())
            .workspace_id()
            .assert_value()
    );
    assert!(!root.join(PROFILES_FILE).exists());
    std::fs::rename(&root, &moved).assert_value();
    assert_eq!(
        first,
        LocalRunProfileStore::new(moved.clone())
            .workspace_id()
            .assert_value()
    );
    let replacement = LocalRunProfileStore::new(root.clone())
        .workspace_id()
        .assert_value();
    assert_ne!(first, replacement);
    std::fs::remove_dir_all(root).assert_value();
    std::fs::remove_dir_all(moved).assert_value();
}

#[cfg(feature = "ui")]
#[test]
fn concurrent_ui_hosts_share_one_durable_workspace_identity() {
    let root = std::env::temp_dir().join(format!("zeroshot-workspace-{}", uuid::Uuid::now_v7()));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let root = root.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                LocalRunProfileStore::new(root)
                    .workspace_id()
                    .assert_value()
            })
        })
        .collect();
    let ids: std::collections::BTreeSet<_> = threads
        .into_iter()
        .map(|thread| thread.join().assert_value())
        .collect();
    assert_eq!(ids.len(), 1);
    std::fs::remove_dir_all(root).assert_value();
}

#[cfg(feature = "ui")]
#[test]
fn malformed_workspace_identity_is_not_silently_replaced() {
    let root = std::env::temp_dir().join(format!("zeroshot-workspace-{}", uuid::Uuid::now_v7()));
    let store = LocalRunProfileStore::new(root.clone());
    store.workspace_id().assert_value();
    let path = root.join("ui-workspace-id");
    for malformed in ["not-an-identity".to_owned(), "x".repeat(128)] {
        std::fs::write(&path, &malformed).assert_value();
        assert!(store.workspace_id().is_err());
        assert_eq!(std::fs::read_to_string(&path).assert_value(), malformed);
    }
    std::fs::remove_dir_all(root).assert_value();
}

#[cfg(all(feature = "ui", unix))]
#[test]
fn workspace_identity_does_not_follow_a_symlink() {
    let root = std::env::temp_dir().join(format!("zeroshot-workspace-{}", uuid::Uuid::now_v7()));
    let store = LocalRunProfileStore::new(root.clone());
    store.workspace_id().assert_value();
    let path = root.join("ui-workspace-id");
    let outside = root.with_extension("identity");
    let id = uuid::Uuid::now_v7().to_string();
    std::fs::write(&outside, &id).assert_value();
    std::fs::remove_file(&path).assert_value();
    std::os::unix::fs::symlink(&outside, &path).assert_value();
    assert!(store.workspace_id().is_err());
    assert_eq!(std::fs::read_to_string(&outside).assert_value(), id);
    std::fs::remove_file(outside).assert_value();
    std::fs::remove_dir_all(root).assert_value();
}

#[test]
fn profile_crud_defaults_and_revisions_are_durable() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalRunProfileStore::new(root.path().to_path_buf());
    assert!(
        store
            .list(RunProfileListRequest {
                scope: RunProfileScope::User,
            })
            .assert_value()
            .profiles
            .is_empty()
    );
    assert!(matches!(
        store.list(RunProfileListRequest {
            scope: RunProfileScope::Org,
        }),
        Err(NativeV2CliError::Local(message))
            if message == "organization-scoped profiles require a hosted target"
    ));

    let alpha = store
        .set(profile_request("alpha", "worker", false))
        .assert_value()
        .profile;
    let alpha_revision = profile_revision(&alpha).assert_value();
    let beta = store
        .set(profile_request("beta", "reviewer", true))
        .assert_value()
        .profile;
    assert!(beta.is_default);

    let reopened = LocalRunProfileStore::new(root.path().to_path_buf());
    let listed = reopened
        .list(RunProfileListRequest {
            scope: RunProfileScope::User,
        })
        .assert_value()
        .profiles;
    assert_eq!(
        listed
            .iter()
            .map(|profile| (profile.name.as_str(), profile.is_default))
            .collect::<Vec<_>>(),
        [("alpha", false), ("beta", true)]
    );

    let edited = reopened
        .set(profile_request("alpha", "implementer", false))
        .assert_value()
        .profile;
    assert_eq!(edited.id, alpha.id);
    assert_ne!(profile_revision(&edited).assert_value(), alpha_revision);
    exercise_default_and_delete_lifecycle(&reopened, &edited);
}

#[test]
fn malformed_profile_store_fails_closed_for_reads_and_mutations() {
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join(PROFILES_FILE);
    let malformed = br#"{"profiles":{},"unexpected":true}"#;
    std::fs::write(&path, malformed).assert_value();
    let store = LocalRunProfileStore::new(root.path().to_path_buf());

    assert_malformed(store.list(RunProfileListRequest {
        scope: RunProfileScope::User,
    }));
    assert_malformed(store.show(selector("alpha")));
    assert_malformed(store.set(profile_request("alpha", "worker", false)));
    assert_malformed(store.delete(selector("alpha")));
    assert_malformed(store.set_default(RunProfileDefaultRequest {
        scope: RunProfileScope::User,
        name: None,
    }));
    assert_eq!(std::fs::read(path).assert_value(), malformed);
}

#[cfg(feature = "ui")]
#[test]
fn checked_profile_edits_enforce_workspace_and_revision_preconditions() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalRunProfileStore::new(root.path().to_path_buf());
    let workspace = store.workspace_id().assert_value();
    let request = profile_request("guarded", "worker", false);
    let created = match store
        .set_checked(request.clone(), None, &workspace)
        .assert_value()
    {
        Ok(created) => created.profile,
        Err(_) => panic!("new profile unexpectedly conflicted"),
    };
    let original_revision = profile_revision(&created).assert_value();

    assert!(matches!(
        store
            .set_checked(request.clone(), None, &workspace)
            .assert_value(),
        Err(ProfileSaveConflict::Revision)
    ));
    let updated = match store
        .set_checked(
            profile_request("guarded", "reviewer", false),
            Some(&original_revision),
            &workspace,
        )
        .assert_value()
    {
        Ok(updated) => updated.profile,
        Err(_) => panic!("current revision unexpectedly conflicted"),
    };
    assert_eq!(updated.id, created.id);
    assert_ne!(profile_revision(&updated).assert_value(), original_revision);
    assert!(matches!(
        store
            .set_checked(request.clone(), Some(&original_revision), &workspace)
            .assert_value(),
        Err(ProfileSaveConflict::Revision)
    ));
    assert!(matches!(
        store
            .set_checked(request, None, "different-workspace")
            .assert_value(),
        Err(ProfileSaveConflict::Workspace)
    ));
}
