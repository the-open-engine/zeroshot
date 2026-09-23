use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ClusterStatus, GetParams, GetResult, InitializeParams, InitializeResult, RequestId,
    ServerCapabilities,
};
use openengine_cluster_server::method_registry::{
    method_descriptor, methods_requiring, MethodKind, SubscriptionKind, TransportRequirements,
    METHOD_REGISTRY,
};
use openengine_cluster_server::{BackendError, ClusterBackend, ConnectionContext, Dispatcher};
use serde_json::Value;
use serde_json::json;

struct EmptyBackend;

const EXPECTED_METHODS: &[&str] = &[
    "initialize",
    "plan",
    "apply",
    "update",
    "stop",
    "retry",
    "resubmit",
    "delete",
    "get",
    "watch",
    "logs",
    "agent/attach",
    "run/submit",
    "run/list",
    "run/status",
    "run/watch",
    "run/logs",
    "run/attach",
    "run/force",
    "run/checkpoints",
    "run/resume",
    "run/discard_workspace",
];

const EXPECTED_SUBSCRIPTIONS: &[(&str, SubscriptionKind)] = &[
    ("watch", SubscriptionKind::Watch),
    ("logs", SubscriptionKind::Logs),
    ("agent/attach", SubscriptionKind::AgentAttach),
    ("run/watch", SubscriptionKind::RunWatch),
    ("run/logs", SubscriptionKind::RunLogs),
    ("run/attach", SubscriptionKind::RunAttach),
];

const EXPECTED_UNARY: &[&str] = &[
    "initialize",
    "plan",
    "apply",
    "update",
    "stop",
    "retry",
    "resubmit",
    "delete",
    "get",
    "run/submit",
    "run/list",
    "run/status",
    "run/force",
    "run/checkpoints",
    "run/resume",
    "run/discard_workspace",
];

#[async_trait]
impl ClusterBackend for EmptyBackend {
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        Ok(InitializeResult::new(
            ServerCapabilities::default(),
            ClusterStatus::empty(),
        ))
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        _params: GetParams,
    ) -> Result<GetResult, BackendError> {
        Ok(GetResult {
            spec: None,
            status: ClusterStatus::empty(),
            at_cursor: None,
            terminal_result: None,
        })
    }
}

#[test]
fn registry_is_the_exact_protocol_method_surface() {
    let names = METHOD_REGISTRY
        .iter()
        .map(|descriptor| descriptor.name)
        .collect::<Vec<_>>();
    assert_eq!(names, EXPECTED_METHODS);
    for descriptor in METHOD_REGISTRY {
        assert_eq!(method_descriptor(descriptor.name), Some(descriptor));
    }
    assert_eq!(method_descriptor(""), None);
    assert_eq!(method_descriptor("WATCH"), None);

    let subscriptions = METHOD_REGISTRY
        .iter()
        .filter_map(|descriptor| match descriptor.kind {
            MethodKind::Unary => None,
            MethodKind::Subscription(kind) => Some((descriptor.name, kind)),
        })
        .collect::<Vec<_>>();
    assert_eq!(subscriptions, EXPECTED_SUBSCRIPTIONS);

    for descriptor in METHOD_REGISTRY {
        let is_subscription = matches!(descriptor.kind, MethodKind::Subscription(_));
        assert_eq!(
            descriptor.transport_requirements,
            TransportRequirements {
                server_push: is_subscription,
                inbound_notifications: is_subscription,
            },
            "wrong transport requirements for {}",
            descriptor.name
        );
    }

    let request_response = methods_requiring(TransportRequirements::default())
        .map(|descriptor| descriptor.name)
        .collect::<Vec<_>>();
    assert_eq!(request_response, EXPECTED_UNARY);
    let subscription_transport = methods_requiring(TransportRequirements {
        server_push: true,
        inbound_notifications: true,
    })
    .map(|descriptor| descriptor.name)
    .collect::<Vec<_>>();
    let expected_subscriptions = EXPECTED_SUBSCRIPTIONS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    assert_eq!(subscription_transport, expected_subscriptions);
}

#[test]
fn bindings_do_not_classify_subscriptions_with_method_name_literals() {
    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    for relative in ["stdio.rs", "connection/frame.rs"] {
        let source = fs::read_to_string(source_root.join(relative)).assert_value();
        for method in [
            "watch",
            "logs",
            "agent/attach",
            "run/watch",
            "run/logs",
            "run/attach",
        ] {
            assert!(
                !source.contains(&format!("\"{method}\"")),
                "{relative} hardcodes subscription method {method}"
            );
        }
    }
}

#[tokio::test]
async fn subscription_methods_remain_unavailable_to_unary_dispatch() {
    let dispatcher = Dispatcher::new(EmptyBackend, ConnectionContext::default());

    for (index, method) in [
        "watch",
        "logs",
        "agent/attach",
        "run/watch",
        "run/logs",
        "run/attach",
    ]
    .into_iter()
    .enumerate()
    {
        let id = RequestId::String(format!("subscription-{index}"));
        let response = dispatcher
            .dispatch_decoded(id.clone(), method, Value::Array(Vec::new()))
            .await;
        let response: Value = serde_json::from_str(&response).assert_value();
        assert_eq!(
            response.assert_at("id"),
            &serde_json::to_value(id).assert_value()
        );
        assert_eq!(response.assert_at("error").assert_at("code"), -32601);
        assert_eq!(
            response.assert_at("error").assert_at("message"),
            "Method not found"
        );
    }
}

#[tokio::test]
async fn default_backend_fails_closed_for_every_supported_unary_extension() {
    let dispatcher = Dispatcher::new(EmptyBackend, ConnectionContext::default());
    let graph: Value = serde_json::from_str(include_str!(
        "../../../protocol/openengine-cluster/v1/fixtures/graph/positive/single-worker.json"
    ))
    .assert_value();
    let cases = [
        ("plan", json!({"graph": graph.clone()})),
        ("apply", json!({"graph": graph})),
        (
            "update",
            json!({"suspended": true, "ifGeneration": 1, "idempotencyKey": "update"}),
        ),
        (
            "stop",
            json!({"mode": "force", "ifGeneration": 1, "idempotencyKey": "stop"}),
        ),
        (
            "retry",
            json!({"ifGeneration": 1, "idempotencyKey": "retry"}),
        ),
        (
            "resubmit",
            json!({"ifGeneration": 1, "ifRunId": "run-1", "idempotencyKey": "resubmit"}),
        ),
        (
            "delete",
            json!({"ifGeneration": 1, "ifRunId": "run-1", "idempotencyKey": "delete"}),
        ),
        ("run/list", json!({})),
        ("run/status", json!({"runId": "run-1"})),
        ("run/force", json!({"runId": "run-1"})),
        (
            "run/resume",
            json!({"runId": "run-1", "successorRunId": "run-2"}),
        ),
        ("run/discard_workspace", json!({"runId": "run-1"})),
    ];

    for (index, (method, params)) in cases.into_iter().enumerate() {
        let response = dispatcher
            .dispatch_decoded(RequestId::Integer(index as i64), method, params)
            .await;
        let response: Value = serde_json::from_str(&response).assert_value();
        assert_eq!(
            response
                .assert_at("error")
                .assert_at("data")
                .assert_at("code"),
            "INVALID_PHASE",
            "{method} must be unavailable until a backend explicitly implements it"
        );
    }
}
#[path = "support/assert_value.rs"]
mod assert_value;
use assert_value::AssertValue;
#[path = "support/assert_at.rs"]
mod assert_at;
use assert_at::AssertAt;
