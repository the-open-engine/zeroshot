use openengine_cluster_protocol::{
    ArtifactId, ArtifactLineage, ArtifactProducer, ArtifactRef, ByteLength, Generation, MediaType,
    NodeName, PositiveInteger, RedactionClass, RunId, Sha256Digest, TypeId, WorkerRef,
};
use serde_json::json;

fn receipt() -> ArtifactRef {
    ArtifactRef {
        artifact_id: ArtifactId::new("artifact-123").unwrap(),
        sha256: Sha256Digest::new("a".repeat(64)).unwrap(),
        byte_length: ByteLength::new(42).unwrap(),
        media_type: MediaType::new("application/json").unwrap(),
        type_id: TypeId::new("openengine.result@1").unwrap(),
        producer: ArtifactProducer {
            node: NodeName::new("worker").unwrap(),
            worker: WorkerRef::new("worker.impl@1").unwrap(),
        },
        lineage: ArtifactLineage {
            generation: Generation::new(7).unwrap(),
            run_id: RunId::new("run-9"),
            attempt: PositiveInteger::new(2).unwrap(),
        },
        redaction: RedactionClass::Confidential,
    }
}

#[test]
fn artifact_receipt_has_the_exact_byte_free_durable_shape() {
    let value = serde_json::to_value(receipt()).unwrap();
    assert_eq!(
        value,
        json!({
            "artifactId": "artifact-123",
            "sha256": "a".repeat(64),
            "byteLength": 42,
            "mediaType": "application/json",
            "typeId": "openengine.result@1",
            "producer": { "node": "worker", "worker": "worker.impl@1" },
            "lineage": { "generation": 7, "runId": "run-9", "attempt": 2 },
            "redaction": "confidential"
        })
    );
    assert_eq!(
        serde_json::from_value::<ArtifactRef>(value).unwrap(),
        receipt()
    );
}

#[test]
fn artifact_receipts_reject_bytes_urls_tokens_paths_bad_hashes_and_unsafe_counts() {
    assert!(TypeId::new("unstable").is_err());
    assert!(TypeId::new("openengine.result@").is_err());

    let mut value = serde_json::to_value(receipt()).unwrap();
    value["typeId"] = json!("openengine.result@");
    assert!(serde_json::from_value::<ArtifactRef>(value.clone()).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(ArtifactRef)).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(!validator.is_valid(&value));

    for field in ["artifactId", "mediaType"] {
        let mut value = serde_json::to_value(receipt()).unwrap();
        value[field] = json!("bad\nvalue");
        assert!(
            serde_json::from_value::<ArtifactRef>(value.clone()).is_err(),
            "Rust accepted a control character in {field}"
        );
        assert!(
            !validator.is_valid(&value),
            "JSON Schema accepted a control character in {field}"
        );

        let mut maximum = serde_json::to_value(receipt()).unwrap();
        maximum[field] = json!("é".repeat(256));
        assert!(
            serde_json::from_value::<ArtifactRef>(maximum.clone()).is_ok(),
            "Rust must apply the JSON Schema character-count bound to {field}"
        );
        assert!(validator.is_valid(&maximum));

        let mut overlong = serde_json::to_value(receipt()).unwrap();
        overlong[field] = json!("é".repeat(257));
        assert!(serde_json::from_value::<ArtifactRef>(overlong.clone()).is_err());
        assert!(!validator.is_valid(&overlong));
    }

    for field in ["bytes", "signedUrl", "bearerToken", "path"] {
        let mut value = serde_json::to_value(receipt()).unwrap();
        value[field] = json!("forbidden");
        assert!(
            serde_json::from_value::<ArtifactRef>(value).is_err(),
            "accepted {field}"
        );
    }

    let mut value = serde_json::to_value(receipt()).unwrap();
    value["sha256"] = json!("ABC");
    assert!(serde_json::from_value::<ArtifactRef>(value).is_err());

    let mut value = serde_json::to_value(receipt()).unwrap();
    value["byteLength"] = json!(9_007_199_254_740_992_u64);
    assert!(serde_json::from_value::<ArtifactRef>(value).is_err());

    let mut value = serde_json::to_value(receipt()).unwrap();
    value["lineage"]["attempt"] = json!(0);
    assert!(serde_json::from_value::<ArtifactRef>(value).is_err());

    let decimal_integral: serde_json::Value = serde_json::from_str("42.0").unwrap();
    let mut value = serde_json::to_value(receipt()).unwrap();
    value["byteLength"] = decimal_integral;
    assert!(serde_json::from_value::<ArtifactRef>(value.clone()).is_ok());
    assert!(validator.is_valid(&value));
}
