use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn supported_non_hosted_target_discovery_advertises_workspace_recovery() {
    let direct =
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_workspace_recovery();
    assert_eq!(
        direct
            .extensions
            .workspace_recovery
            .as_ref()
            .map(|capability| capability.kind.as_str()),
        Some(WORKSPACE_RECOVERY_KIND)
    );
    assert!(
        TargetDiscoveryDocument::direct(TargetAuthentication::PrivateCapability)
            .with_workspace_recovery()
            .extensions
            .workspace_recovery
            .is_some()
    );
    for authentication in [
        TargetAuthentication::None,
        TargetAuthentication::PrivateCapability,
    ] {
        assert!(
            TargetDiscoveryDocument::direct(authentication)
                .extensions
                .workspace_recovery
                .is_none()
        );
    }
    assert!(
        TargetDiscoveryDocument::direct(TargetAuthentication::HostedOauth)
            .with_workspace_recovery()
            .extensions
            .workspace_recovery
            .is_none()
    );
}

#[test]
fn every_authentication_mode_can_advertise_node_runtime_lanes() {
    assert_eq!(NODE_RUNTIME_LANES_KIND, "openengine.node-runtime-lanes/v1");
    for authentication in [
        TargetAuthentication::None,
        TargetAuthentication::PrivateCapability,
        TargetAuthentication::HostedOauth,
    ] {
        let direct = TargetDiscoveryDocument::direct(authentication);
        assert!(direct.extensions.node_runtime_lanes.is_none());

        let document = direct.with_node_runtime_lanes();
        assert_eq!(
            document
                .extensions
                .node_runtime_lanes
                .as_ref()
                .map(|capability| capability.kind.as_str()),
            Some(NODE_RUNTIME_LANES_KIND)
        );
        let value = serde_json::to_value(&document).assert_value();
        assert_eq!(
            value["extensions"]["node_runtime_lanes"],
            serde_json::json!({ "kind": "openengine.node-runtime-lanes/v1" })
        );
        assert_eq!(
            serde_json::from_value::<TargetDiscoveryDocument>(value).assert_value(),
            document
        );
    }
}

#[test]
fn discovery_serialized_without_node_runtime_lanes_reads_back_without_the_marker() {
    for document in [
        TargetDiscoveryDocument::direct(TargetAuthentication::None),
        TargetDiscoveryDocument::direct(TargetAuthentication::None).with_workspace_recovery(),
    ] {
        let value = serde_json::to_value(&document).assert_value();
        assert!(value.pointer("/extensions/node_runtime_lanes").is_none());
        let decoded = serde_json::from_value::<TargetDiscoveryDocument>(value).assert_value();
        assert!(decoded.extensions.node_runtime_lanes.is_none());
        assert_eq!(decoded, document);
    }
}

#[test]
fn discovery_ignores_unknown_extension_keys_but_rejects_extra_marker_fields() {
    let mut wire =
        serde_json::to_value(TargetDiscoveryDocument::direct(TargetAuthentication::None))
            .assert_value();
    wire["extensions"] = serde_json::json!({
        "future_capability": {"kind": "openengine.future/v1", "detail": true},
        "node_runtime_lanes": {"kind": "openengine.node-runtime-lanes/v2"}
    });
    let decoded = serde_json::from_value::<TargetDiscoveryDocument>(wire.clone()).assert_value();
    assert_eq!(
        decoded
            .extensions
            .node_runtime_lanes
            .as_ref()
            .map(|marker| marker.kind.as_str()),
        Some("openengine.node-runtime-lanes/v2")
    );

    wire["extensions"]["node_runtime_lanes"]["harnesses"] = serde_json::json!(["codex"]);
    assert!(serde_json::from_value::<TargetDiscoveryDocument>(wire).is_err());
}

#[test]
fn node_runtime_lanes_marker_alone_makes_extensions_non_empty() {
    assert!(TargetDiscoveryExtensions::default().is_empty());
    let extensions = TargetDiscoveryExtensions {
        node_runtime_lanes: Some(TargetNodeRuntimeLanesDiscovery {
            kind: NODE_RUNTIME_LANES_KIND.to_owned(),
        }),
        ..TargetDiscoveryExtensions::default()
    };
    assert!(!extensions.is_empty());
}

#[test]
fn run_history_discovery_uses_the_versioned_camel_case_wire_shape() {
    let document = TargetDiscoveryDocument::direct(TargetAuthentication::None).with_run_history(
        TargetRunHistoryDiscovery {
            kind: RUN_HISTORY_KIND.to_owned(),
            base_url: "http://127.0.0.1:8080".to_owned(),
            route_templates: TargetRunHistoryRoutes {
                list: "/native-v2/run-history{?after}".to_owned(),
                detail: "/native-v2/run-history/{run_id}".to_owned(),
                page: "/native-v2/run-history/{run_id}/page{?after}".to_owned(),
            },
        },
    );
    let value = serde_json::to_value(document).assert_value();
    assert_eq!(
        value["extensions"]["run_history"],
        serde_json::json!({
            "kind": "zeroshot.run-history/v1",
            "baseUrl": "http://127.0.0.1:8080",
            "routeTemplates": {
                "list": "/native-v2/run-history{?after}",
                "detail": "/native-v2/run-history/{run_id}",
                "page": "/native-v2/run-history/{run_id}/page{?after}"
            }
        })
    );
}

#[test]
fn uuid_v7_validation_is_canonical_and_versioned() {
    assert!(is_canonical_uuid_v7(&RunId::new(
        "018f5e78-7f95-7c22-8d98-3f15af20c991"
    )));
    for invalid in [
        "018f5e78-7f95-4c22-8d98-3f15af20c991",
        "018F5E78-7F95-7C22-8D98-3F15AF20C991",
        "run-018f5e78-7f95-7c22-8d98-3f15af20c991",
    ] {
        assert!(!is_canonical_uuid_v7(&RunId::new(invalid)));
    }
}

#[test]
fn target_request_debug_redacts_ephemeral_values() {
    let request = serde_json::from_str::<TargetRunRequest>(include_str!(
        "../../tests/fixtures/native-v2-target-request.json"
    ));
    assert!(request.is_ok());
    let Ok(request) = request else {
        return;
    };
    let debug = format!("{request:?}");
    assert!(!debug.contains("environment-secret"));
    assert!(!debug.contains("github-secret"));
    assert!(!debug.contains("resolver-secret"));
    assert!(debug.contains("OPENAI_API_KEY"));
}

#[test]
fn static_connection_debug_redacts_values_and_validates_shape() {
    let values = StaticConnectionValues::new(BTreeMap::from([(
        EnvironmentVariableName::new("TOKEN").assert_value(),
        "secret-value".to_owned(),
    )]))
    .assert_value();
    let debug = format!("{values:?}");
    assert!(debug.contains("TOKEN"));
    assert!(!debug.contains("secret-value"));
    assert!(StaticConnectionValues::new(BTreeMap::new()).is_err());
}
