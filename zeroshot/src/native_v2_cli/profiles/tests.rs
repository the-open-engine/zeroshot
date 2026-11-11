use openengine_cluster_testkit::admission::graph_fixture;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

fn profile_request(name: &str, node: &str, set_default: bool) -> RunProfileSetRequest {
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

fn selector(name: &str) -> RunProfileSelector {
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

fn nested_profile_request(name: &str, depth: usize) -> RunProfileSetRequest {
    let mut node = json!({
        "kind": "succeed", "name": "done", "output": {"kind": "null"}, "bindings": []
    });
    for index in 1..depth {
        node = json!({
            "kind": "seq", "name": format!("level{index}"), "state": {"kind": "null"},
            "children": [node], "promotedStatePaths": []
        });
    }
    RunProfileSetRequest {
        name: RunProfileName::new(name).assert_value(),
        scope: RunProfileScope::User,
        graph: serde_json::from_value(json!({
            "profile": "openengine.graph.full/v1", "initialInput": {"kind": "null"},
            "policy": {"policy": "policy.native-v2@1", "default": "deny"}, "root": node
        }))
        .assert_value(),
        runtime: serde_json::from_value(json!({
            "harness": "codex", "provider": "openai", "size": "small", "nodes": {}
        }))
        .assert_value(),
        set_default: false,
    }
}

#[test]
fn profile_store_round_trips_depth_64_graphs_with_transport_wrappers() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalRunProfileStore::new(root.path().to_path_buf());
    for depth in [61, 62, 63, 64] {
        store
            .set(nested_profile_request("nested", depth))
            .assert_value();
        let reopened = LocalRunProfileStore::new(root.path().to_path_buf());
        reopened.show(selector("nested")).assert_value();
    }
    assert!(store.delete(selector("nested")).assert_value().deleted);
}

fn temporary_store() -> (tempfile::TempDir, LocalRunProfileStore) {
    let root = tempfile::tempdir().assert_value();
    let store = LocalRunProfileStore::new(root.path().to_path_buf());
    (root, store)
}

fn read_profile_json(store: &LocalRunProfileStore) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(store.root.join(PROFILES_FILE)).assert_value())
        .assert_value()
}

fn edit_profile_entry(
    store: &LocalRunProfileStore,
    edit: impl FnOnce(&mut serde_json::Value),
) -> serde_json::Value {
    let mut encoded = read_profile_json(store);
    let entry = &mut encoded["profiles"]["broken"];
    edit(entry);
    let changed = entry.clone();
    std::fs::write(
        store.root.join(PROFILES_FILE),
        serde_json::to_vec(&encoded).assert_value(),
    )
    .assert_value();
    changed
}

fn replace_profile_payload(store: &LocalRunProfileStore, field: &str) -> serde_json::Value {
    edit_profile_entry(store, |entry| {
        entry[field] = json!({"futureFormat": {"children": [1, "two"]}});
    })
}

fn assert_profile_payload_unchanged(store: &LocalRunProfileStore, expected: &serde_json::Value) {
    assert_eq!(&read_profile_json(store)["profiles"]["broken"], expected);
}

fn assert_profile_error<T>(result: Result<T, NativeV2CliError>, expected: &str) {
    match result {
        Err(NativeV2CliError::Local(message)) => assert_eq!(message, expected),
        Err(error) => panic!("unexpected profile error: {error}"),
        Ok(_) => panic!("expected profile error: {expected}"),
    }
}

#[test]
fn incompatible_payloads_do_not_block_other_profiles_or_metadata_operations() {
    for field in ["graph", "runtime"] {
        let (_root, store) = temporary_store();
        let healthy = store
            .set(profile_request("healthy", "worker", true))
            .assert_value()
            .profile;
        let broken = store
            .set(profile_request("broken", "worker", false))
            .assert_value()
            .profile;
        let incompatible = replace_profile_payload(&store, field);

        let listed = store
            .list(RunProfileListRequest {
                scope: RunProfileScope::User,
            })
            .assert_value()
            .profiles;
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name.as_str(), "broken");
        assert_eq!(listed[0].id, broken.id);
        assert!(!listed[0].is_default);
        assert_eq!(listed[1].name, healthy.name);
        assert!(listed[1].is_default);
        assert_eq!(store.show(selector("healthy")).assert_value(), healthy);
        assert!(matches!(
            store.show(selector("broken")),
            Err(NativeV2CliError::Local(message))
                if message == format!("profile broken has a malformed {field}")
        ));

        let edited = store
            .set(profile_request("healthy", "reviewer", false))
            .assert_value()
            .profile;
        assert_eq!(edited.id, healthy.id);
        assert!(edited.is_default);
        store
            .set(profile_request("new", "worker", false))
            .assert_value();
        store
            .set_default(RunProfileDefaultRequest {
                scope: RunProfileScope::User,
                name: Some(broken.name.clone()),
            })
            .assert_value();
        assert!(!store.show(selector("healthy")).assert_value().is_default);
        assert!(
            store
                .list(RunProfileListRequest {
                    scope: RunProfileScope::User
                })
                .assert_value()
                .profiles[0]
                .is_default
        );
        assert_profile_payload_unchanged(&store, &incompatible);

        assert!(store.delete(selector("broken")).assert_value().deleted);
        assert_eq!(
            store
                .list(RunProfileListRequest {
                    scope: RunProfileScope::User
                })
                .assert_value()
                .profiles
                .iter()
                .filter(|profile| profile.is_default)
                .count(),
            0
        );
        assert_eq!(
            store.show(selector("new")).assert_value().name.as_str(),
            "new"
        );
    }
}

#[test]
fn incompatible_profile_can_be_replaced_without_decoding_its_old_payloads() {
    for field in ["graph", "runtime"] {
        let (_root, store) = temporary_store();
        let original = store
            .set(profile_request("broken", "worker", true))
            .assert_value()
            .profile;
        replace_profile_payload(&store, field);

        let request = profile_request("broken", "reviewer", false);
        let expected = RunProfile {
            id: original.id,
            name: request.name.clone(),
            scope: request.scope,
            graph: request.graph.clone(),
            runtime: request.runtime.clone(),
            is_default: true,
        };
        assert_eq!(store.set(request).assert_value().profile, expected);
        assert_eq!(store.show(selector("broken")).assert_value(), expected);
    }
}

#[cfg(feature = "ui")]
fn exercise_checked_metadata_isolation(store: &LocalRunProfileStore) {
    let path = store.root.join(PROFILES_FILE);
    let workspace = store.workspace_id().assert_value();
    let revision = profile_revision(&store.show(selector("healthy")).assert_value()).assert_value();
    assert!(
        store
            .set_checked(
                profile_request("healthy", "checked", false),
                Some(&revision),
                &workspace
            )
            .assert_value()
            .is_ok()
    );
    let before = std::fs::read(&path).assert_value();
    assert!(
        store
            .set_checked(
                profile_request("broken", "worker", false),
                Some("stale"),
                &workspace
            )
            .is_err()
    );
    assert_eq!(std::fs::read(&path).assert_value(), before);
}

#[test]
fn malformed_profile_metadata_is_isolated_preserved_and_removable() {
    for corruption in ["entry", "id", "graph", "runtime", "extra"] {
        let (_root, store) = temporary_store();
        let healthy = store
            .set(profile_request("healthy", "worker", true))
            .assert_value()
            .profile;
        store
            .set(profile_request("broken", "worker", false))
            .assert_value();
        let damaged = edit_profile_entry(&store, |entry| match corruption {
            "entry" => *entry = json!(null),
            "id" => entry["id"] = json!(17),
            "extra" => entry["futureMetadata"] = json!({"preserve": true}),
            missing => {
                entry.as_object_mut().assert_value().remove(missing);
            }
        });

        let listed = store
            .list(RunProfileListRequest {
                scope: RunProfileScope::User,
            })
            .assert_value();
        assert_eq!(listed.profiles.len(), 1);
        assert_eq!(listed.profiles[0].id, healthy.id);
        assert_eq!(
            serde_json::to_value(&listed).assert_value()["problems"][0]["name"],
            "broken"
        );
        assert!(
            listed.clone().into_protocol().is_err(),
            "legacy results cannot silently discard profile problems"
        );
        assert_eq!(
            listed.problems,
            vec![CliProfileProblem {
                name: selector("broken").name,
                scope: RunProfileScope::User,
                is_default: false,
                message: "profile broken has malformed metadata".to_owned(),
            }]
        );
        assert_eq!(store.show(selector("healthy")).assert_value(), healthy);
        assert_profile_error(
            store.show(selector("broken")),
            "profile broken has malformed metadata",
        );

        store
            .set(profile_request("healthy", "reviewer", false))
            .assert_value();
        store
            .set(profile_request("new", "worker", false))
            .assert_value();
        store
            .set_default(RunProfileDefaultRequest {
                scope: RunProfileScope::User,
                name: Some(selector("broken").name),
            })
            .assert_value();
        assert!(
            store
                .list(RunProfileListRequest {
                    scope: RunProfileScope::User
                })
                .assert_value()
                .problems[0]
                .is_default
        );
        assert_profile_payload_unchanged(&store, &damaged);

        #[cfg(feature = "ui")]
        exercise_checked_metadata_isolation(&store);

        assert!(store.delete(selector("broken")).assert_value().deleted);
        let listed = store
            .list(RunProfileListRequest {
                scope: RunProfileScope::User,
            })
            .assert_value();
        assert_eq!(listed.profiles.len(), 2);
        assert!(listed.problems.is_empty());
        assert!(!listed.profiles.iter().any(|profile| profile.is_default));
        let wire = listed.into_protocol().assert_value();
        let presented = CliProfileListResult::from(wire.clone());
        assert!(presented.problems.is_empty());
        assert_eq!(
            serde_json::to_value(presented).assert_value(),
            serde_json::to_value(wire).assert_value()
        );
    }
}

#[cfg(feature = "ui")]
#[test]
fn checked_saves_decode_only_the_selected_profile_and_preserve_incompatible_payloads() {
    for field in ["graph", "runtime"] {
        let (_root, store) = temporary_store();
        let workspace = store.workspace_id().assert_value();
        let healthy = store
            .set(profile_request("healthy", "worker", false))
            .assert_value()
            .profile;
        store
            .set(profile_request("broken", "worker", false))
            .assert_value();
        let incompatible = replace_profile_payload(&store, field);
        let revision = profile_revision(&healthy).assert_value();

        let saved = store
            .set_checked(
                profile_request("healthy", "reviewer", true),
                Some(&revision),
                &workspace,
            )
            .assert_value()
            .unwrap_or_else(|_| panic!("healthy profile unexpectedly conflicted"))
            .profile;
        assert_eq!(saved.id, healthy.id);
        assert!(saved.is_default);
        assert_ne!(profile_revision(&saved).assert_value(), revision);
        assert!(
            store
                .set_checked(profile_request("new", "worker", false), None, &workspace)
                .assert_value()
                .is_ok()
        );
        assert_profile_payload_unchanged(&store, &incompatible);

        let before = std::fs::read(store.root.join(PROFILES_FILE)).assert_value();
        assert!(matches!(
            store.set_checked(
                profile_request("broken", "reviewer", false),
                Some("stale-revision"),
                &workspace,
            ),
            Err(NativeV2CliError::Local(message))
                if message == format!("profile broken has a malformed {field}")
        ));
        assert_eq!(
            std::fs::read(store.root.join(PROFILES_FILE)).assert_value(),
            before
        );
    }
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

#[test]
fn saved_profiles_preserve_workspace_checkpoint_opt_out() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalRunProfileStore::new(root.path().to_owned());
    let mut request = profile_request("checkpoint-off", "worker", false);
    request.runtime.workspace_checkpoints = false;
    store.set(request).assert_value();
    let saved = LocalRunProfileStore::new(root.path().to_owned())
        .show(selector("checkpoint-off"))
        .assert_value();
    assert!(!saved.runtime.workspace_checkpoints);
}

#[test]
fn interrupted_saves_do_not_keep_a_replaced_profile() {
    let (root, store) = temporary_store();
    store
        .set(profile_request("alpha", "retired", false))
        .assert_value();
    // `profile default alpha` was killed after writing its store and before the rename.
    let mut interrupted = store.read().assert_value();
    interrupted.default = Some(RunProfileName::new("alpha").assert_value());
    let mut encoded = serde_json::to_vec(&interrupted).assert_value();
    encoded.push(b'\n');
    let mut file = owner_only_options()
        .write(true)
        .create_new(true)
        .open(temporary_path(root.path()))
        .assert_value();
    std::io::Write::write_all(&mut file, &encoded).assert_value();
    drop(file);

    store
        .set(profile_request("alpha", "implementer", false))
        .assert_value();
    let mut retained = Vec::new();
    for entry in std::fs::read_dir(root.path()).assert_value() {
        let path = entry.assert_value().path();
        let stored = std::fs::read_to_string(&path).assert_value();
        if stored.contains("retired") {
            retained.push(path);
        }
    }
    assert_eq!(retained, Vec::<PathBuf>::new());
}

#[test]
fn an_unreadable_store_write_preserves_existing_bytes_and_never_creates_an_initial_store() {
    for existing in [false, true] {
        let (_root, store) = temporary_store();
        if existing {
            store
                .set(profile_request("valid", "worker", true))
                .assert_value();
        }
        let path = store.root.join(PROFILES_FILE);
        let before = std::fs::read(&path).ok();
        let mut stored = store.read().assert_value();
        let bad = RunProfileName::new("unreadable").assert_value();
        let payload = (0..513).fold(serde_json::Value::Null, |value, _| json!([value]));
        stored.profiles.insert(
            bad.clone(),
            serde_json::value::to_raw_value(&payload).assert_value(),
        );
        stored.default = Some(bad);
        assert!(store.write(&stored).is_err());
        assert_eq!(std::fs::read(&path).ok(), before);
        assert_eq!(
            store
                .read()
                .assert_value()
                .default
                .map(|name| name.to_string()),
            existing.then(|| "valid".to_owned())
        );
    }
}
