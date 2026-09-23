use openengine_cluster_protocol::{
    CheckpointId, HOSTED_WORKSPACE_RECOVERY_KIND, RunCheckpoint, RunCheckpointsParams,
    RunCheckpointsResult, RunResumeFrom, RunResumeParams, TargetAuthentication,
    TargetDiscoveryDocument, TargetDiscoveryExtensions, TargetHostedRunsDiscovery,
    MAX_SAFE_GENERATION, WORKSPACE_CHECKPOINTS_KIND, WORKSPACE_RECOVERY_KIND,
};
use openengine_cluster_testkit::assertions::AssertValue;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

fn assert_contract<T: DeserializeOwned + JsonSchema>(value: Value, accepted: bool) {
    let schema = serde_json::to_value(schemars::schema_for!(T)).assert_value();
    let validator = jsonschema::validator_for(&schema).assert_value();
    assert_eq!(validator.is_valid(&value), accepted, "schema: {value}");
    assert_eq!(
        serde_json::from_value::<T>(value.clone()).is_ok(),
        accepted,
        "wire: {value}"
    );
}

fn checkpoint() -> Value {
    json!({
        "checkpointId": "opaque-entry-2", "sequence": 2, "node": "verify-group",
        "mapIndices": [0, 2], "loopIterations": [1], "createdAt": 1_700_000_000_000_u64
    })
}

#[test]
fn resume_omission_preserves_restart_and_checkpoint_selection_is_closed() {
    let legacy = json!({"runId": "prior", "successorRunId": "successor"});
    let params: RunResumeParams = serde_json::from_value(legacy.clone()).assert_value();
    assert!(params.from.is_none());
    assert_eq!(serde_json::to_value(params).assert_value(), legacy);
    assert_contract::<RunResumeParams>(legacy.clone(), true);
    for from in [
        json!({"kind": "restart"}),
        json!({"kind": "checkpoint", "checkpointId": "opaque-entry-2"}),
    ] {
        let mut value = legacy.clone();
        value["from"] = from;
        assert_contract::<RunResumeParams>(value, true);
    }
    for from in [
        json!({"kind": "checkpoint"}),
        json!({"kind": "checkpoint", "checkpointId": ""}),
        json!({"kind": "node", "node": "verify-group"}),
        json!({"kind": "restart", "checkpointId": "opaque-entry-2"}),
        json!({"kind": "checkpoint", "checkpointId": "entry", "workspace": "/private"}),
    ] {
        assert_contract::<RunResumeFrom>(from, false);
    }
}

#[test]
fn checkpoint_ids_are_opaque_bounded_non_control_text() {
    for value in ["entry/opaque:1".to_owned(), "é".repeat(256)] {
        let id = CheckpointId::new(value.clone()).assert_value();
        assert_eq!(id.as_str(), value);
        assert_eq!(id.to_string(), value);
        assert_contract::<CheckpointId>(json!(value), true);
    }
    for value in [
        String::new(),
        "x".repeat(257),
        "entry\n".to_owned(),
        "\u{85}".to_owned(),
    ] {
        assert!(CheckpointId::new(value.clone()).is_err());
        assert_contract::<CheckpointId>(json!(value), false);
    }
}

#[test]
fn checkpoint_query_limits_match_schema_and_direct_validation() {
    let defaults: RunCheckpointsParams =
        serde_json::from_value(json!({"runId": "run-1"})).assert_value();
    assert_eq!(defaults.page_limit(), 50);
    assert!(defaults.after.is_none());
    assert!(defaults.validate().is_ok());
    for limit in [json!(1), json!(50), json!(100), json!(100.0), Value::Null] {
        assert_contract::<RunCheckpointsParams>(
            json!({"runId": "run-1", "after": "entry", "limit": limit}),
            true,
        );
    }
    for limit in [json!(0), json!(101), json!(-1), json!(1.5), json!(u64::MAX)] {
        assert_contract::<RunCheckpointsParams>(json!({"runId": "run-1", "limit": limit}), false);
    }
    for limit in [0, 101, u32::MAX] {
        let params = RunCheckpointsParams {
            limit: Some(limit),
            ..defaults.clone()
        };
        assert!(params.validate().is_err());
    }
    assert_contract::<RunCheckpointsParams>(json!({"runId": "run-1", "after": ""}), false);
    assert_contract::<RunCheckpointsParams>(
        json!({"runId": "run-1", "workspace": "/private"}),
        false,
    );
}

#[test]
fn checkpoint_metadata_is_javascript_safe_and_storage_private() {
    assert_contract::<RunCheckpoint>(checkpoint(), true);
    for field in ["sequence", "createdAt"] {
        for number in [json!(0), json!(MAX_SAFE_GENERATION + 1), json!(1.5)] {
            let mut value = checkpoint();
            value[field] = number;
            assert_contract::<RunCheckpoint>(value, false);
        }
    }
    for field in ["mapIndices", "loopIterations"] {
        let mut value = checkpoint();
        value[field] = json!([0, MAX_SAFE_GENERATION]);
        assert_contract::<RunCheckpoint>(value, true);
        for number in [json!(-1), json!(MAX_SAFE_GENERATION + 1), json!(1.5)] {
            let mut value = checkpoint();
            value[field] = json!([number]);
            assert_contract::<RunCheckpoint>(value, false);
        }
    }
    for field in ["workspace", "seed", "providerSession"] {
        let mut value = checkpoint();
        value[field] = json!("private");
        assert_contract::<RunCheckpoint>(value, false);
    }
}

#[test]
fn checkpoint_result_pages_are_bounded_and_round_trip_the_cursor() {
    let value = json!({
        "runId": "run-1", "checkpoints": [checkpoint()], "nextAfter": "opaque-entry-2"
    });
    let result: RunCheckpointsResult = serde_json::from_value(value.clone()).assert_value();
    assert_eq!(serde_json::to_value(result).assert_value(), value);
    assert_contract::<RunCheckpointsResult>(value, true);
    for (count, accepted) in [(0, true), (100, true), (101, false)] {
        assert_contract::<RunCheckpointsResult>(
            json!({"runId": "run-1", "checkpoints": vec![checkpoint(); count]}),
            accepted,
        );
    }
}

#[test]
fn checkpoints_are_opt_in_and_discovery_tolerates_future_extensions() {
    for authentication in [
        TargetAuthentication::None,
        TargetAuthentication::PrivateCapability,
    ] {
        let direct = TargetDiscoveryDocument::direct(authentication);
        assert!(direct.extensions.is_empty());
        let advertised = direct
            .with_workspace_recovery()
            .with_workspace_checkpoints();
        assert!(!advertised.extensions.is_empty());
        assert_eq!(
            advertised
                .extensions
                .workspace_checkpoints
                .assert_value()
                .kind,
            WORKSPACE_CHECKPOINTS_KIND,
        );
        assert_eq!(
            advertised.extensions.workspace_recovery.assert_value().kind,
            WORKSPACE_RECOVERY_KIND,
        );
    }
    let hosted = TargetDiscoveryDocument::direct(TargetAuthentication::HostedOauth)
        .with_workspace_recovery()
        .with_workspace_checkpoints();
    assert!(hosted.extensions.is_empty());
    let extensions: TargetDiscoveryExtensions = serde_json::from_value(json!({
        "workspace_checkpoints": {"kind": WORKSPACE_CHECKPOINTS_KIND},
        "future_extension": {"kind": "future/v1", "opaque": true}
    }))
    .assert_value();
    assert!(extensions.workspace_checkpoints.is_some());
}

#[test]
fn hosted_recovery_routes_do_not_change_the_strict_hosted_runs_v1_shape() {
    let legacy_routes = json!({
        "kind": "zeroshot.hosted-runs/v1",
        "base_url": "https://target.example",
        "route_templates": {
            "list": "/runs",
            "status": "/runs/{run_id}",
            "watch": "/runs/{run_id}/watch{?from_cursor}",
            "logs": "/runs/{run_id}/logs{?from_cursor,execution}",
            "force": "/runs/{run_id}/force"
        }
    });
    assert!(serde_json::from_value::<TargetHostedRunsDiscovery>(legacy_routes.clone()).is_ok());
    let mut incompatible = legacy_routes;
    incompatible["route_templates"]["resume"] = json!("/runs/{run_id}/resume");
    assert!(serde_json::from_value::<TargetHostedRunsDiscovery>(incompatible).is_err());

    let extensions: TargetDiscoveryExtensions = serde_json::from_value(json!({
        "hosted_workspace_recovery": {
            "kind": HOSTED_WORKSPACE_RECOVERY_KIND,
            "route_templates": {
                "resume": "/runs/{run_id}/resume",
                "checkpoints": "/runs/{run_id}/checkpoints",
                "discard_workspace": "/runs/{run_id}/discard-workspace"
            }
        }
    }))
    .assert_value();
    assert!(extensions.hosted_workspace_recovery.is_some());
}
