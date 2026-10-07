#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use openengine_cluster_protocol::{
    CodexProvider, DeclaredConnections, DeclaredEnvironment, ModelId, NodeName, NodeRuntimeBinding,
    RunSize, SessionScope,
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
    RuntimePlan::Codex {
        provider: CodexProvider::OpenAi,
        size: RunSize::Small,
        nodes: BTreeMap::from([(
            NodeName::new("worker").assert_value(),
            NodeRuntimeBinding::Agent {
                model: ModelId::new("gpt-5.6").assert_value(),
                effort: None,
                session_scope: SessionScope::Execution,
                connections,
            },
        )]),
    }
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
