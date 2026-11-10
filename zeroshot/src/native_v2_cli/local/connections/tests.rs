use openengine_cluster_testkit::fixture::run_size;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use openengine_cluster_protocol::{
    DeclaredConnections, DeclaredEnvironment, ModelId, NodeName, NodeRuntimeBinding, SessionScope,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("zeroshot-connections-{}", uuid::Uuid::now_v7())))
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(value: &str) -> ConnectionKey {
    ConnectionKey::new(value).assert_value()
}

fn field(value: &str) -> EnvironmentVariableName {
    EnvironmentVariableName::new(value).assert_value()
}

fn values(entries: &[(&str, &str)]) -> StaticConnectionValues {
    StaticConnectionValues::new(
        entries
            .iter()
            .map(|(name, value)| (field(name), (*value).to_owned()))
            .collect(),
    )
    .assert_value()
}

fn runtime(fields: &[&str]) -> RuntimePlan {
    let environment =
        DeclaredEnvironment::new(fields.iter().map(|name| field(name))).assert_value();
    let connections = DeclaredConnections::single("provider", environment).assert_value();
    crate::native_v2_candidate::test_support::codex_runtime(
        run_size("host"),
        BTreeMap::from([(
            NodeName::new("worker").assert_value(),
            NodeRuntimeBinding::Agent {
                lane: None,
                model: ModelId::new("gpt-5.6").assert_value(),
                effort: None,
                harness_config: None,
                context_window: None,
                max_tokens: None,
                session_scope: SessionScope::Execution,
                connections,
            },
        )]),
    )
}

#[test]
fn static_crud_exposes_metadata_only_and_uses_private_files() {
    let root = TestRoot::new();
    let store = LocalConnectionStore::new(root.0.clone());
    let mutation = store
        .set(ConnectionSetRequest {
            key: key("provider"),
            scope: ConnectionScope::User,
            values: values(&[("OPENAI_API_KEY", "very-secret")]),
        })
        .assert_value();
    assert_eq!(mutation.connection.fields, [field("OPENAI_API_KEY")]);
    assert!(!format!("{mutation:?}").contains("very-secret"));

    let list = store
        .list(ConnectionListRequest {
            scope: ConnectionScope::User,
        })
        .assert_value();
    assert_eq!(list.connections, [mutation.connection]);
    drop(private_file(&root.0.join(CONNECTIONS_FILE), FileAccess::Read).assert_value());
    #[cfg(unix)]
    {
        let metadata = std::fs::metadata(root.0.join(CONNECTIONS_FILE)).assert_value();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(&root.0)
                .assert_value()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    let deleted = store
        .delete(ConnectionDeleteRequest {
            key: key("provider"),
            scope: ConnectionScope::User,
        })
        .assert_value();
    assert!(deleted.deleted);
    assert!(
        store
            .list(ConnectionListRequest {
                scope: ConnectionScope::User
            })
            .assert_value()
            .connections
            .is_empty()
    );
}

#[test]
fn resolution_selects_declared_fields_and_rejects_partial_explicit_overrides() {
    let root = TestRoot::new();
    let store = LocalConnectionStore::new(root.0.clone());
    store
        .set(ConnectionSetRequest {
            key: key("provider"),
            scope: ConnectionScope::User,
            values: values(&[
                ("OPENAI_API_KEY", "stored-key"),
                ("UNDECLARED", "stored-extra"),
            ]),
        })
        .assert_value();
    let single_field_runtime = runtime(&["OPENAI_API_KEY"]);
    let resolved = store
        .resolve(&single_field_runtime, None, &BTreeMap::new())
        .assert_value();
    assert_eq!(
        resolved.bootstrap_values(),
        BTreeMap::from([(key("provider"), values(&[("OPENAI_API_KEY", "stored-key")]),)])
    );
    let refreshed = store
        .resolve(
            &single_field_runtime,
            None,
            &BTreeMap::from([(key("provider"), values(&[("OPENAI_API_KEY", "fresh-key")]))]),
        )
        .assert_value();
    assert_eq!(
        refreshed.bootstrap_values(),
        BTreeMap::from([(key("provider"), values(&[("OPENAI_API_KEY", "fresh-key")]),)])
    );

    let runtime = runtime(&["OPENAI_API_KEY", "OPENAI_ORG"]);
    let error = store
        .resolve(
            &runtime,
            None,
            &BTreeMap::from([(key("provider"), values(&[("OPENAI_API_KEY", "inline")]))]),
        )
        .assert_error();
    assert!(error.to_string().contains("does not exactly define"));
}

#[test]
fn local_store_rejects_org_scope() {
    let root = TestRoot::new();
    let error = LocalConnectionStore::new(root.0.clone())
        .list(ConnectionListRequest {
            scope: ConnectionScope::Org,
        })
        .assert_error();
    assert!(error.to_string().contains("hosted target"));
    assert!(!root.0.exists());
}

/// What a save killed after writing and before its rename leaves: the complete store it was
/// committing, under the producer's temporary name and private file creation.
fn leave_interrupted_save(root: &Path, connections: &StoredConnections) {
    let mut encoded = serde_json::to_vec(connections).assert_value();
    encoded.push(b'\n');
    let mut file = private_file(&temporary_path(root), FileAccess::CreateNew).assert_value();
    std::io::Write::write_all(&mut file, &encoded).assert_value();
}

#[track_caller]
fn assert_stored_only_in(root: &Path, value: &str, expected: &[&str]) {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root).assert_value() {
        let path = entry.assert_value().path();
        let stored = std::fs::read_to_string(&path).assert_value();
        if stored.contains(value) {
            found.push(path);
        }
    }
    let expected: Vec<_> = expected.iter().map(|name| root.join(name)).collect();
    assert_eq!(found, expected, "files that still contain {value}");
}

fn set_token(store: &LocalConnectionStore, name: &str, token: &str) {
    store
        .set(ConnectionSetRequest {
            key: key(name),
            scope: ConnectionScope::User,
            values: values(&[("TOKEN", token)]),
        })
        .assert_value();
}

fn delete(store: &LocalConnectionStore, name: &str) -> bool {
    store
        .delete(ConnectionDeleteRequest {
            key: key(name),
            scope: ConnectionScope::User,
        })
        .assert_value()
        .deleted
}

#[test]
fn interrupted_saves_do_not_keep_a_deleted_connection_secret() {
    let root = TestRoot::new();
    let store = LocalConnectionStore::new(root.0.clone());
    set_token(&store, "c0", "deleted-c0-secret");
    set_token(&store, "c1", "kept-c1-secret");
    // `connection set killed` was killed while committing a store that still held c0.
    let mut interrupted = store.read().assert_value();
    interrupted.insert(key("killed"), values(&[("TOKEN", "uncommitted")]));
    leave_interrupted_save(&root.0, &interrupted);

    assert!(delete(&store, "c0"));
    assert_stored_only_in(&root.0, "deleted-c0-secret", &[]);

    // A state directory from a release without cleanup: c0 is already absent from the store, but
    // an older interrupted save still holds it, so deleting c0 again must remove that copy.
    leave_interrupted_save(&root.0, &interrupted);
    assert!(!delete(&store, "c0"));
    assert_stored_only_in(&root.0, "deleted-c0-secret", &[]);
    assert_stored_only_in(&root.0, "kept-c1-secret", &[CONNECTIONS_FILE]);
}

#[test]
fn interrupted_saves_do_not_keep_a_rotated_connection_value() {
    let root = TestRoot::new();
    let store = LocalConnectionStore::new(root.0.clone());
    set_token(&store, "provider", "rotated-old-value");
    leave_interrupted_save(&root.0, &store.read().assert_value());

    set_token(&store, "provider", "rotated-new-value");
    assert_stored_only_in(&root.0, "rotated-old-value", &[]);
    assert_stored_only_in(&root.0, "rotated-new-value", &[CONNECTIONS_FILE]);
}
