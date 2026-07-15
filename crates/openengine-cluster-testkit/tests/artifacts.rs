use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use openengine_cluster_protocol::PROTOCOL_VERSION;
use openengine_cluster_testkit::artifacts::{
    ArtifactError, check_artifacts, generate_artifacts, write_artifacts,
};

#[tokio::test]
async fn generated_artifacts_are_complete_and_committed_without_drift() {
    let artifacts = generate_artifacts().await;
    let paths: Vec<_> = artifacts
        .iter()
        .map(|artifact| artifact.relative_path.as_str())
        .collect();
    for required in [
        "protocol/openengine-cluster/v1/schema.json",
        "protocol/openengine-cluster/v1/graph.schema.json",
        "protocol/openengine-cluster/v1/compiled-ir.schema.json",
        "protocol/openengine-cluster/v1/openrpc.json",
        "protocol/openengine-cluster/v1/worker.schema.json",
        "protocol/openengine-cluster/v1/fixtures/workers/positive/acp-v1.json",
        "protocol/openengine-cluster/v1/fixtures/workers/positive/a2a-1.0.json",
        "protocol/openengine-cluster/v1/fixtures/workers/positive/legacy-zeroshot-ship-v1.json",
        "protocol/openengine-cluster/v1/fixtures/workers/positive/artifact-receipt.json",
        "protocol/openengine-cluster/v1/fixtures/workers/negative/bearer-token.json",
        "protocol/openengine-cluster/v1/fixtures/workers/mock/a2a-auth-required.json",
        "protocol/openengine-cluster/v1/fixtures/graph/positive/full-all-nodes.json",
        "protocol/openengine-cluster/v1/fixtures/graph/positive/single-worker.json",
        "protocol/openengine-cluster/v1/fixtures/graph/canonical/base.canonical.json",
        "protocol/openengine-cluster/v1/fixtures/graph/canonical/base.sha256",
        "protocol/openengine-cluster/v1/fixtures/graph/negative/unknown-node.json",
        "protocol/openengine-cluster/v1/goldens/initialize.ndjson",
        "protocol/openengine-cluster/v1/goldens/get-empty.ndjson",
    ] {
        assert!(paths.contains(&required), "missing {required}");
    }
    let unique: std::collections::BTreeSet<_> = paths.iter().copied().collect();
    assert_eq!(unique.len(), paths.len(), "artifact paths must be unique");
    for artifact in &artifacts {
        if artifact
            .relative_path
            .ends_with("/canonical/base.canonical.json")
        {
            assert!(
                !artifact.bytes.ends_with(b"\n"),
                "canonical-byte golden must contain exactly the hashed bytes"
            );
        } else {
            assert!(
                artifact.bytes.ends_with(b"\n"),
                "{} must be newline terminated",
                artifact.relative_path
            );
        }
    }

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    check_artifacts(&workspace).await.unwrap();
}

#[tokio::test]
async fn openrpc_exposes_contract_components_without_advertising_future_methods() {
    let artifacts = generate_artifacts().await;
    let openrpc = artifacts
        .iter()
        .find(|artifact| artifact.relative_path.ends_with("/openrpc.json"))
        .unwrap();
    let openrpc: serde_json::Value = serde_json::from_slice(&openrpc.bytes).unwrap();
    let methods: Vec<_> = openrpc["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|method| method["name"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["initialize", "get"]);
    for component in [
        "GraphSpec",
        "CompiledGraphIr",
        "GraphDiagnostic",
        "StructuralBounds",
        "ArtifactRef",
        "WorkerDescriptor",
        "WorkerOutcome",
        "LegacyShipRequest",
        "LegacyShipResult",
    ] {
        assert!(openrpc["components"]["schemas"].get(component).is_some());
    }
}

#[tokio::test]
async fn schema_is_derived_from_canonical_envelopes_with_required_success_ids() {
    let artifacts = generate_artifacts().await;
    let schema = artifacts
        .iter()
        .find(|artifact| artifact.relative_path.ends_with("/schema.json"))
        .unwrap();
    let schema: serde_json::Value = serde_json::from_slice(&schema.bytes).unwrap();
    let definitions = schema["$defs"].as_object().unwrap();

    assert!(
        definitions
            .keys()
            .any(|name| name.starts_with("JsonRpcRequest"))
    );
    assert!(
        definitions
            .keys()
            .any(|name| name.starts_with("JsonRpcSuccess"))
    );
    assert!(
        definitions
            .keys()
            .any(|name| name.starts_with("JsonRpcResponse"))
    );
    assert!(
        !definitions
            .keys()
            .any(|name| name.contains("EnvelopeSchema"))
    );

    for definition in definitions
        .iter()
        .filter(|(name, _)| name.starts_with("JsonRpcSuccess"))
        .map(|(_, definition)| definition)
    {
        assert!(
            definition["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("id"))
        );
        assert_eq!(definition["properties"]["id"]["$ref"], "#/$defs/RequestId");
    }

    assert_eq!(
        definitions["InitializeParams"]["properties"]["protocolVersion"]["const"],
        PROTOCOL_VERSION
    );
}

#[tokio::test]
async fn artifact_check_fails_on_byte_drift_and_missing_files() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let workspace = std::env::temp_dir().join(format!(
        "openengine-cluster-artifact-test-{}-{unique}",
        std::process::id()
    ));
    write_artifacts(&workspace).await.unwrap();

    let schema = workspace.join("protocol/openengine-cluster/v1/schema.json");
    fs::write(&schema, b"drift\n").unwrap();
    assert!(matches!(
        check_artifacts(&workspace).await,
        Err(ArtifactError::Drift(path)) if path == schema
    ));

    fs::remove_file(&schema).unwrap();
    assert!(matches!(
        check_artifacts(&workspace).await,
        Err(ArtifactError::Missing(path)) if path == schema
    ));
    fs::remove_dir_all(workspace).unwrap();
}
