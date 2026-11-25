//! Main at 79ccf9e already has run parts and artifacts, but no workspace observation fields.
use std::sync::Arc;

use base_server::watch::fixtures as base;
use openengine_cluster_client::{ClusterClient, ClientError, NdjsonTransport, WebSocketTransport};
use openengine_cluster_protocol as current;
use openengine_cluster_server::watch::fixtures as now;
use serde_json::{json, Value};

fn current_backend() -> now::FixtureBackend {
    now::FixtureBackend::new(Arc::new(now::FixtureStore::new(
        current::RunId::new("must-not-observe"),
        Vec::new(),
        8,
    )))
}

fn base_backend() -> base::FixtureBackend {
    base::FixtureBackend::new(Arc::new(base::FixtureStore::new(
        base_protocol::RunId::new("must-not-observe"),
        Vec::new(),
        8,
    )))
}

fn rejection_details(requested: &str, supported: &str) -> Value {
    json!({"requestedProtocolVersion": requested, "supportedProtocolVersion": supported})
}

fn base_rejected(error: base_client::ClientError) {
    let base_client::ClientError::Rpc(error) = error else {
        panic!("base client must stop at initialization: {error}");
    };
    let data = error.data.expect("version error");
    assert_eq!(data.code, "UNSUPPORTED_PROTOCOL_VERSION");
    assert_eq!(
        data.details,
        Some(rejection_details(
            base_protocol::PROTOCOL_VERSION,
            current::PROTOCOL_VERSION
        ))
    );
}

fn current_rejected(error: ClientError) {
    let ClientError::Rpc(error) = error else {
        panic!("current client must stop at initialization: {error}");
    };
    let data = error.data.expect("version error");
    assert_eq!(data.code, "UNSUPPORTED_PROTOCOL_VERSION");
    assert_eq!(
        data.details,
        Some(rejection_details(
            current::PROTOCOL_VERSION,
            base_protocol::PROTOCOL_VERSION
        ))
    );
}

#[tokio::test]
async fn actual_main_client_is_refused_at_both_direct_initialization_transports() {
    let (write, read, server) = now::spawn_ndjson(current_backend());
    let transport = base_client::NdjsonTransport::new(read, write);
    base_rejected(
        base_client::ClusterClient::new(&transport)
            .initialize()
            .await
            .expect_err("base peer"),
    );
    drop(transport);
    now::await_ndjson_shutdown(server).await;

    let (socket, server) = now::spawn_websocket(current_backend()).await;
    let transport = base_client::WebSocketTransport::new(socket);
    base_rejected(
        base_client::ClusterClient::new(&transport)
            .initialize()
            .await
            .expect_err("base peer"),
    );
    drop(transport);
    now::await_websocket_shutdown(server).await;
}

#[tokio::test]
async fn current_client_is_refused_at_both_actual_main_initialization_transports() {
    let (write, read, server) = base::spawn_ndjson(base_backend());
    let transport = NdjsonTransport::new(read, write);
    current_rejected(
        ClusterClient::new(&transport)
            .initialize()
            .await
            .expect_err("base target"),
    );
    drop(transport);
    base::await_ndjson_shutdown(server).await;

    let (socket, server) = base::spawn_websocket(base_backend()).await;
    let transport = WebSocketTransport::new(socket);
    current_rejected(
        ClusterClient::new(&transport)
            .initialize()
            .await
            .expect_err("base target"),
    );
    drop(transport);
    base::await_websocket_shutdown(server).await;
}

#[test]
fn actual_main_discovery_identifiers_require_bidirectional_direct_and_hosted_upgrade() {
    assert_eq!(base_protocol::PROTOCOL_VERSION, "openengine.cluster/v3");
    assert_eq!(
        base_protocol::TARGET_DISCOVERY_KIND,
        "zeroshot.native-v2-target/v4"
    );
    assert_eq!(base_protocol::HOSTED_RUNS_KIND, "zeroshot.hosted-runs/v3");
    assert_ne!(base_protocol::PROTOCOL_VERSION, current::PROTOCOL_VERSION);
    let old =
        base_protocol::TargetDiscoveryDocument::direct(base_protocol::TargetAuthentication::None);
    let new = current::TargetDiscoveryDocument::direct(current::TargetAuthentication::None)
        .with_run_workspaces();
    // Decode using the opposite peer's real discovery reader. Its existing exact-kind gate must
    // refuse before obtaining credentials or requesting any status/list/watch payload.
    let read_by_base: base_protocol::TargetDiscoveryDocument =
        serde_json::from_value(serde_json::to_value(&new).unwrap()).unwrap();
    let read_by_current: current::TargetDiscoveryDocument =
        serde_json::from_value(serde_json::to_value(old).unwrap()).unwrap();
    assert_ne!(read_by_base.kind, base_protocol::TARGET_DISCOVERY_KIND);
    assert_ne!(read_by_current.kind, current::TARGET_DISCOVERY_KIND);
    let routes = json!({"list":"/runs", "status":"/runs/{runId}", "watch":"/runs/{runId}/watch", "logs":"/runs/{runId}/logs", "force":"/runs/{runId}/force"});
    let old_hosted: base_protocol::TargetHostedRunsDiscovery = serde_json::from_value(json!({
        "kind":current::HOSTED_RUNS_KIND, "base_url":"https://target.example", "route_templates":routes,
    })).unwrap();
    let new_hosted: current::TargetHostedRunsDiscovery = serde_json::from_value(json!({
        "kind":base_protocol::HOSTED_RUNS_KIND, "base_url":"https://target.example", "route_templates":routes,
    })).unwrap();
    assert_ne!(old_hosted.kind, base_protocol::HOSTED_RUNS_KIND);
    assert_ne!(new_hosted.kind, current::HOSTED_RUNS_KIND);
}

#[test]
fn actual_main_workspace_incompatibility_isolated_without_removing_part_identity() {
    let status: current::RunStatusResult = serde_json::from_value(json!({
        "runId":"root", "partId":"part", "currentPartId":"part", "title":"Workspace",
        "size":"host", "atCursor":"v2:1", "status":{"phase":"admitted"},
        "workspace":{"placement":"managed"},
    }))
    .unwrap();
    let direct = serde_json::to_value(&status).unwrap();
    let hosted = serde_json::to_value(current::HostedRunStatusResult::from(status)).unwrap();
    let watch = json!({
        "subscriptionId":"watch", "runId":"root", "partId":"part", "title":"Workspace",
        "size":"host", "cursor":"v2:1", "status":{"phase":"admitted"},
        "workspace":{"placement":"managed"}, "recordedAt":1725000000999_u64,
    });
    macro_rules! check {
        ($ty:ident, $value:expr, $status:expr) => {{
            let value = $value;
            let same: current::$ty = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(same).unwrap(), value);
            let error = serde_json::from_value::<base_protocol::$ty>(value.clone()).unwrap_err();
            assert!(error.to_string().contains("workspace"), "{error}");
            let mut legacy = value;
            let status = $status(&mut legacy);
            assert_eq!(status["partId"], "part");
            status.as_object_mut().unwrap().remove("workspace");
            let decoded: base_protocol::$ty = serde_json::from_value(legacy.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
        }};
    }
    fn root(value: &mut Value) -> &mut Value {
        value
    }
    fn listed(value: &mut Value) -> &mut Value {
        &mut value["runs"][0]
    }
    check!(RunStatusResult, direct.clone(), root);
    check!(HostedRunStatusResult, hosted.clone(), root);
    check!(RunListResult, json!({"runs":[direct]}), listed);
    check!(HostedRunListResult, json!({"runs":[hosted]}), listed);
    check!(RunWatchEventNotification, watch.clone(), root);
    check!(HostedRunWatchEventNotification, watch, root);
}
