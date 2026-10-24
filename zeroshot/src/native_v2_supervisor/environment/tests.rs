use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use openengine_cluster_protocol::{
    DeclaredConnections, DeclaredEnvironment, ModelId, NodeName, RunSize, SessionScope,
};
use openengine_cluster_testkit::assertions::AssertValue;

#[derive(Default)]
struct RotatingResolver {
    calls: AtomicUsize,
}

#[async_trait]
impl RunConnectionResolver for RotatingResolver {
    async fn resolve(
        &self,
        requirements: RunConnectionRequirements,
    ) -> Result<RunConnectionValues, ConnectionResolutionError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        requirements
            .into_iter()
            .map(|(key, fields)| {
                let values = fields
                    .into_iter()
                    .map(|field| (field, format!("dynamic-{call}")))
                    .collect();
                StaticConnectionValues::new(values)
                    .map(|values| (key, values))
                    .map_err(|_| ConnectionResolutionError::InvalidResponse)
            })
            .collect()
    }
}

fn binding(key: &str, name: &EnvironmentVariableName) -> NodeRuntimeBinding {
    NodeRuntimeBinding::Agent {
        lane: None,
        model: ModelId::new("gpt-5.6").assert_value(),
        effort: None,
        context_window: None,
        max_tokens: None,
        session_scope: SessionScope::Execution,
        connections: DeclaredConnections::single(
            key,
            DeclaredEnvironment::new([name.clone()]).assert_value(),
        )
        .assert_value(),
    }
}

#[tokio::test]
async fn same_environment_name_can_resolve_from_different_keys_on_different_nodes() {
    let name = EnvironmentVariableName::new("TOKEN").assert_value();
    let first = binding("first", &name);
    let second = binding("second", &name);
    let runtime = crate::native_v2_candidate::test_support::codex_runtime(
        RunSize::Small,
        BTreeMap::from([
            (NodeName::new("one").assert_value(), first.clone()),
            (NodeName::new("two").assert_value(), second.clone()),
        ]),
    );
    let connection = |value: &str| {
        StaticConnectionValues::new(BTreeMap::from([(name.clone(), value.to_owned())]))
            .assert_value()
    };
    let environment = RunEnvironment::exact(
        &runtime,
        None,
        BTreeMap::from([
            (
                ConnectionKey::new("first").assert_value(),
                connection("first-secret"),
            ),
            (
                ConnectionKey::new("second").assert_value(),
                connection("second-secret"),
            ),
        ]),
    )
    .assert_value();

    assert!(
        !environment
            .resolve(&first)
            .await
            .assert_value()
            .can_refresh()
    );
    assert!(
        !environment
            .resolve(&second)
            .await
            .assert_value()
            .can_refresh()
    );
    assert_eq!(
        environment.resolve(&first).await.assert_value().get(&name),
        Some("first-secret")
    );
    assert_eq!(
        environment.resolve(&second).await.assert_value().get(&name),
        Some("second-secret")
    );
}

#[tokio::test]
async fn dynamic_values_are_refreshed_for_node_start_and_runtime_refresh() {
    let (name, node, environment) = dynamic_environment(Arc::new(RotatingResolver::default()));

    let first = environment.resolve(&node).await.assert_value();
    assert!(first.can_refresh());
    assert_eq!(first.get(&name), Some("dynamic-1"));
    assert_eq!(
        crate::native_v2_runner::refresh_environment(&first)
            .await
            .assert_value()
            .get(&name),
        Some("dynamic-2")
    );
    assert_eq!(
        environment.resolve(&node).await.assert_value().get(&name),
        Some("dynamic-3")
    );
}

fn dynamic_environment(
    resolver: Arc<dyn RunConnectionResolver>,
) -> (EnvironmentVariableName, NodeRuntimeBinding, RunEnvironment) {
    let name = EnvironmentVariableName::new("GH_TOKEN").assert_value();
    let node = binding("github", &name);
    let runtime = crate::native_v2_candidate::test_support::codex_runtime(
        openengine_cluster_protocol::RunSize::Small,
        BTreeMap::from([(NodeName::new("deliver").assert_value(), node.clone())]),
    );
    let key = ConnectionKey::new("github").assert_value();
    let environment = RunEnvironment::with_resolver(
        &runtime,
        None,
        BTreeMap::new(),
        DynamicConnectionPlan {
            keys: BTreeSet::from([key.clone()]),
            source_connection: Some(key),
            resolver,
        },
    )
    .assert_value();

    (name, node, environment)
}

struct FailAfterFirstResolution {
    initial: RotatingResolver,
    error: ConnectionResolutionError,
}

#[async_trait]
impl RunConnectionResolver for FailAfterFirstResolution {
    async fn resolve(
        &self,
        requirements: RunConnectionRequirements,
    ) -> Result<RunConnectionValues, ConnectionResolutionError> {
        if self.initial.calls.load(Ordering::SeqCst) > 0 {
            return Err(self.error);
        }
        self.initial.resolve(requirements).await
    }
}

#[tokio::test]
async fn runtime_refresh_preserves_resolution_failure_classification() {
    for (error, expected) in [
        (
            ConnectionResolutionError::Unavailable,
            EnvironmentRefreshError::Unavailable,
        ),
        (
            ConnectionResolutionError::Refused,
            EnvironmentRefreshError::Refused,
        ),
        (
            ConnectionResolutionError::InvalidResponse,
            EnvironmentRefreshError::InvalidResponse,
        ),
    ] {
        let (_, node, environment) = dynamic_environment(Arc::new(FailAfterFirstResolution {
            initial: RotatingResolver::default(),
            error,
        }));
        let initial = environment.resolve(&node).await.assert_value();
        assert_eq!(
            crate::native_v2_runner::refresh_environment(&initial)
                .await
                .err(),
            Some(expected)
        );
    }
}

#[tokio::test]
async fn rebind_uses_admitted_public_variables_without_replacing_connection_values() {
    let token = EnvironmentVariableName::new("TOKEN").assert_value();
    let setting = EnvironmentVariableName::new("NODE_ENV").assert_value();
    let node = binding("registry", &token);
    let runtime = crate::native_v2_candidate::test_support::codex_runtime(
        RunSize::Small,
        BTreeMap::from([(NodeName::new("agent").assert_value(), node.clone())]),
    );
    let definition = |value: &str| RuntimeEnvironment {
        variables: BTreeMap::from([(setting.clone(), value.to_owned())]),
        ..Default::default()
    };
    let original = RunEnvironment::from_available(
        &runtime,
        Some(&definition("old")),
        &BTreeMap::from([(token.clone(), "private-value".to_owned())]),
    )
    .assert_value();
    let admitted = definition("new");
    let rebound = original
        .for_runtime(&runtime, Some(&admitted))
        .assert_value();
    let resolved = rebound.resolve(&node).await.assert_value();
    assert_eq!(resolved.get(&setting), Some("new"));
    assert_eq!(resolved.get(&token), Some("private-value"));
    assert_eq!(
        original.resolve(&node).await.assert_value().get(&setting),
        Some("old")
    );
    assert_eq!(
        rebound
            .preparation_values(&admitted)
            .await
            .assert_value()
            .get("NODE_ENV"),
        Some(&"new".to_owned())
    );
}

fn source_request() -> RunSubmission {
    let wire: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../crates/openengine-cluster-protocol/tests/fixtures/native-v2-lane-submission.json"
    )).assert_value();
    let mut request: RunSubmission =
        serde_json::from_value(wire["submission"].clone()).assert_value();
    for node in request.runtime.nodes_mut().values_mut() {
        if let NodeRuntimeBinding::Agent { connections, .. } = node {
            *connections = DeclaredConnections::default();
        }
    }
    request.source = Some(serde_json::from_value(serde_json::json!({
        "repository":"nested/team/project", "branch":"main", "revision":"a".repeat(64),
        "remote":{"url":"https://git.example/nested/team/project.git", "connection":"source-access"}
    })).assert_value());
    request
}

fn source_credentials() -> GitTransportCredentials {
    GitTransportCredentials::Https {
        username: "private-user".to_owned(),
        password: "source-secret".to_owned(),
    }
}

#[tokio::test]
async fn source_only_static_authority_never_enters_agent_environment() {
    // These nodes declare no source fields. A caller's explicit node or hook declaration
    // remains authorized independently, even when it uses the same connection key and field.
    let request = source_request();
    let credentials = source_credentials();
    let values = BTreeMap::from([(
        ConnectionKey::new("source-access").assert_value(),
        credentials.connection_values().assert_value(),
    )]);
    let environment = RunEnvironment::exact_for_submission(&request, values).assert_value();
    assert_eq!(
        environment.resolve_git_source().await.assert_value(),
        Some(credentials)
    );
    for node in request.runtime.nodes().values() {
        let resolved = environment.resolve(node).await.assert_value();
        for field in ["GIT_USERNAME", "GIT_PASSWORD"] {
            assert!(
                resolved
                    .get(&EnvironmentVariableName::new(field).assert_value())
                    .is_none()
            );
        }
    }
    assert!(!format!("{environment:?}").contains("source-secret"));
    assert!(!format!("{environment:?}").contains("private-user"));
}

#[tokio::test]
async fn one_connection_key_can_authorize_disjoint_source_and_agent_fields() {
    let mut request = source_request();
    let token = EnvironmentVariableName::new("NODE_TOKEN").assert_value();
    let node = binding("source-access", &token);
    request
        .runtime
        .nodes_mut()
        .insert(NodeName::new("worker").assert_value(), node.clone());
    let mut values = source_credentials()
        .connection_values()
        .assert_value()
        .as_map()
        .clone();
    values.insert(token.clone(), "agent-secret".to_owned());
    let environment = RunEnvironment::exact_for_submission(
        &request,
        BTreeMap::from([(
            ConnectionKey::new("source-access").assert_value(),
            StaticConnectionValues::new(values).assert_value(),
        )]),
    )
    .assert_value();
    let admitted = crate::native_v2_admission::NativeV2Admission
        .admit(request)
        .await
        .assert_value();
    let environment = environment.for_admitted(&admitted).assert_value();
    let resolved = environment.resolve(&node).await.assert_value();
    assert_eq!(resolved.get(&token), Some("agent-secret"));
    assert!(
        resolved
            .get(&EnvironmentVariableName::new("GIT_PASSWORD").assert_value())
            .is_none()
    );
    assert_eq!(
        environment.resolve_git_source().await.assert_value(),
        Some(source_credentials())
    );
}

fn dynamic_source_environment(resolver: Arc<RotatingResolver>) -> (RunSubmission, RunEnvironment) {
    let request = source_request();
    let key = ConnectionKey::new("source-access").assert_value();
    let environment = RunEnvironment::with_resolver_for_submission(
        &request,
        BTreeMap::new(),
        DynamicConnectionPlan {
            resolver,
            keys: BTreeSet::from([key.clone()]),
            source_connection: Some(key),
        },
    )
    .assert_value();
    (request, environment)
}

#[tokio::test]
async fn dynamic_source_only_authority_refreshes_without_a_dummy_delivery_node() {
    let resolver = Arc::new(RotatingResolver::default());
    let (request, environment) = dynamic_source_environment(resolver.clone());
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    for node in request.runtime.nodes().values() {
        environment.resolve(node).await.assert_value();
    }
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    for expected in ["dynamic-1", "dynamic-2"] {
        assert_eq!(
            environment.resolve_git_source().await.assert_value(),
            Some(GitTransportCredentials::Https {
                username: expected.to_owned(),
                password: expected.to_owned(),
            })
        );
    }
}

#[tokio::test]
async fn delivery_private_source_authority_refreshes_with_empty_node_connections() {
    let resolver = Arc::new(RotatingResolver::default());
    let (request, environment) = dynamic_source_environment(resolver.clone());
    let binding = NodeRuntimeBinding::GitDelivery {
        connections: DeclaredConnections::default(),
        pull_request_feedback: Default::default(),
    };
    let initial = environment.resolve(&binding).await.assert_value();
    assert!(initial.can_refresh());
    assert_eq!(initial.iter().count(), 0);
    assert_eq!(
        initial.source_git_credentials(),
        Some(&GitTransportCredentials::Https {
            username: "dynamic-1".to_owned(),
            password: "dynamic-1".to_owned(),
        })
    );
    let refreshed = crate::native_v2_runner::refresh_environment(&initial)
        .await
        .assert_value();
    assert_eq!(refreshed.iter().count(), 0);
    assert_eq!(
        refreshed.source_git_credentials(),
        Some(&GitTransportCredentials::Https {
            username: "dynamic-2".to_owned(),
            password: "dynamic-2".to_owned(),
        })
    );
    assert!(!format!("{refreshed:?}").contains("dynamic-2"));
    for node in request.runtime.nodes().values() {
        let agent = environment.resolve(node).await.assert_value();
        assert!(agent.source_git_credentials().is_none());
        assert!(!agent.can_refresh());
    }
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn generic_dynamic_source_selection_must_match_immutable_source_connection() {
    let request = source_request();
    let source = ConnectionKey::new("source-access").assert_value();
    for selected in [
        None,
        Some(ConnectionKey::new("another-source").assert_value()),
    ] {
        let result = RunEnvironment::with_resolver_for_submission(
            &request,
            BTreeMap::new(),
            DynamicConnectionPlan {
                resolver: Arc::new(RotatingResolver::default()),
                keys: BTreeSet::from([source.clone()]),
                source_connection: selected,
            },
        );
        assert!(matches!(result, Err(RunEnvironmentError::InvalidPlan)));
    }
}

#[tokio::test]
async fn generic_source_cannot_request_undeclared_legacy_forge_fields() {
    let request = source_request();
    let environment = RunEnvironment::exact_for_submission(
        &request,
        BTreeMap::from([(
            ConnectionKey::new("source-access").assert_value(),
            source_credentials().connection_values().assert_value(),
        )]),
    )
    .assert_value();
    assert!(matches!(
        environment
            .resolve_source(BTreeSet::from([
                EnvironmentVariableName::new("GH_TOKEN").assert_value()
            ]))
            .await,
        Err(RunEnvironmentError::InvalidPlan)
    ));
}

#[tokio::test]
async fn rebind_and_resume_refuse_changed_source_authorization() {
    let request = source_request();
    let key = ConnectionKey::new("source-access").assert_value();
    let values = BTreeMap::from([(key, source_credentials().connection_values().assert_value())]);
    let environment = RunEnvironment::exact_for_submission(&request, values.clone()).assert_value();
    let mut admitted = crate::native_v2_admission::NativeV2Admission
        .admit(request)
        .await
        .assert_value();
    RunEnvironment::exact_for_admitted(&admitted, values.clone()).assert_value();
    admitted
        .source
        .as_mut()
        .assert_value()
        .remote
        .as_mut()
        .assert_value()
        .connection = Some(ConnectionKey::new("other-source").assert_value());
    assert!(matches!(
        environment.for_admitted(&admitted),
        Err(RunEnvironmentError::InvalidPlan)
    ));
    assert!(matches!(
        RunEnvironment::exact_for_admitted(&admitted, values),
        Err(RunEnvironmentError::MissingConnection(_))
    ));
}
