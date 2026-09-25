use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use openengine_cluster_protocol::{
    CodexProvider, DeclaredConnections, DeclaredEnvironment, ModelId, NodeName, RunSize,
    SessionScope,
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
        model: ModelId::new("gpt-5.6").assert_value(),
        effort: None,
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
    let runtime = RuntimePlan::Codex {
        provider: CodexProvider::OpenAi,
        size: RunSize::Small,
        nodes: BTreeMap::from([
            (NodeName::new("one").assert_value(), first.clone()),
            (NodeName::new("two").assert_value(), second.clone()),
        ]),
    };
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
    let runtime = crate::native_v2_candidate::test_support::codex_runtime(BTreeMap::from([(
        NodeName::new("deliver").assert_value(),
        node.clone(),
    )]));
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
    let runtime = RuntimePlan::Codex {
        provider: CodexProvider::OpenAi,
        size: RunSize::Small,
        nodes: BTreeMap::from([(NodeName::new("agent").assert_value(), node.clone())]),
    };
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
