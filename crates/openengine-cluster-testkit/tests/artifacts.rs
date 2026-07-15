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
    assert_eq!(
        paths,
        [
            "protocol/openengine-cluster/v1/schema.json",
            "protocol/openengine-cluster/v1/openrpc.json",
            "protocol/openengine-cluster/v1/goldens/initialize.ndjson",
            "protocol/openengine-cluster/v1/goldens/get-empty.ndjson",
            "protocol/openengine-cluster/v1/goldens/incompatible-version.ndjson",
            "protocol/openengine-cluster/v1/goldens/invalid-params.ndjson",
            "protocol/openengine-cluster/v1/goldens/unknown-method.ndjson",
            "protocol/openengine-cluster/v1/goldens/malformed-request.ndjson",
            "protocol/openengine-cluster/v1/goldens/rejected-batch.ndjson",
        ]
    );
    let all_newline_terminated = artifacts
        .iter()
        .all(|artifact| artifact.bytes.ends_with(b"\n"));
    assert!(all_newline_terminated);

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    check_artifacts(&workspace).await.unwrap();
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
