//! Deterministic generated worker schemas and test-only conformance vectors.

use openengine_cluster_protocol::{
    legacy_ship_request_payload_type, legacy_ship_result_payload_type, LegacyShipRequest,
    LegacyShipResult, WorkerDescriptor, WorkerOutcome,
};
use schemars::schema_for;
use serde_json::{json, Value};

use crate::artifacts::Artifact;
use crate::schema_helpers::merge_schema;

const ROOT: &str = "protocol/openengine-cluster/v1";

#[must_use]
pub fn worker_schema() -> Value {
    let mut root = serde_json::to_value(schema_for!(WorkerDescriptor)).unwrap();
    for (name, component) in [
        (
            "WorkerOutcome",
            serde_json::to_value(schema_for!(WorkerOutcome)).unwrap(),
        ),
        (
            "LegacyShipRequest",
            serde_json::to_value(schema_for!(LegacyShipRequest)).unwrap(),
        ),
        (
            "LegacyShipResult",
            serde_json::to_value(schema_for!(LegacyShipResult)).unwrap(),
        ),
    ] {
        merge_schema(&mut root, name, component);
    }
    root
}

#[must_use]
pub fn worker_fixture_artifacts() -> Vec<Artifact> {
    let acp = descriptor("mock.acp@1", "acp", "1", "openengine.worker.acp/v1");
    let a2a = descriptor("mock.a2a@1", "a2a", "1.0", "openengine.worker.a2a/1.0");
    let mut legacy = descriptor(
        "legacy.zeroshot.ship@1",
        "legacy_zeroshot",
        "1",
        "legacy.zeroshot.ship/v1",
    );
    legacy["graphProfiles"] = json!(["openengine.graph.single-worker/v1"]);
    legacy["contract"]["input"] = serde_json::to_value(legacy_ship_request_payload_type()).unwrap();
    legacy["contract"]["output"] = serde_json::to_value(legacy_ship_result_payload_type()).unwrap();

    let mut artifacts = positive_worker_artifacts(acp.clone(), a2a, legacy.clone());
    artifacts.extend(negative_contract_artifacts(&acp));
    artifacts.extend(negative_secret_artifacts(&acp));
    artifacts
}

fn positive_worker_artifacts(acp: Value, a2a: Value, legacy: Value) -> Vec<Artifact> {
    vec![
        json_artifact("positive/acp-v1.json", acp.clone()),
        json_artifact("positive/a2a-1.0.json", a2a),
        json_artifact("positive/legacy-zeroshot-ship-v1.json", legacy),
        json_artifact(
            "positive/policy-refusal.json",
            json!({
                "status": "error", "code": "refusal", "reason": "policy_denied"
            }),
        ),
        json_artifact(
            "positive/artifact-receipt.json",
            json!({
                "artifactId": "worker-result", "sha256": "a".repeat(64), "byteLength": 42,
                "mediaType": "application/json", "typeId": "openengine.result@1",
                "producer": { "node": "worker", "worker": "mock.acp@1" },
                "lineage": { "generation": 1, "runId": "run-1", "attempt": 1 },
                "redaction": "internal"
            }),
        ),
        json_artifact(
            "mock/acp-input-request.json",
            json!({
                "profile": "openengine.worker.acp/v1", "version": "1",
                "normalized": { "status": "error", "code": "refusal", "reason": "interactive_input_required" }
            }),
        ),
        json_artifact(
            "mock/a2a-auth-required.json",
            json!({
                "profile": "openengine.worker.a2a/1.0", "version": "1.0",
                "normalized": { "status": "error", "code": "refusal", "reason": "authentication_required" }
            }),
        ),
    ]
}

fn negative_contract_artifacts(acp: &Value) -> Vec<Artifact> {
    negative_artifacts(
        acp,
        vec![
            (
                "unsupported-version",
                "UNSUPPORTED_WORKER_BINDING",
                "/binding/version",
                json!("2"),
            ),
            (
                "empty-profiles",
                "EMPTY_GRAPH_PROFILES",
                "/graphProfiles",
                json!([]),
            ),
            (
                "duplicate-profiles",
                "DUPLICATE_GRAPH_PROFILES",
                "/graphProfiles",
                json!(["openengine.graph.full/v1", "openengine.graph.full/v1"]),
            ),
            (
                "unknown-error",
                "UNKNOWN_WORKER_ERROR",
                "/contract/errors",
                json!(["unknown"]),
            ),
            (
                "empty-artifact-types",
                "EMPTY_ARTIFACT_TYPES",
                "/artifactProfile/allowedTypeIds",
                json!([]),
            ),
            (
                "missing-runtime-error",
                "INCOMPLETE_WORKER_ERRORS",
                "/contract/errors",
                json!(["timeout", "crash", "malformed"]),
            ),
        ],
    )
}

fn negative_secret_artifacts(acp: &Value) -> Vec<Artifact> {
    negative_artifacts(
        acp,
        vec![
            ("command", "FORBIDDEN_FIELD", "/command", json!("execute")),
            (
                "endpoint",
                "FORBIDDEN_FIELD",
                "/endpoint",
                json!("https://example.invalid"),
            ),
            (
                "credential-value",
                "FORBIDDEN_FIELD",
                "/credentialValue",
                json!("secret"),
            ),
            (
                "bearer-token",
                "FORBIDDEN_FIELD",
                "/bearerToken",
                json!("secret"),
            ),
            ("api-token", "FORBIDDEN_FIELD", "/apiToken", json!("secret")),
            (
                "signed-url",
                "FORBIDDEN_FIELD",
                "/signedUrl",
                json!("https://signed.invalid"),
            ),
            ("inline-bytes", "FORBIDDEN_FIELD", "/bytes", json!("AA==")),
            (
                "filesystem-path",
                "FORBIDDEN_FIELD",
                "/path",
                json!("/tmp/secret"),
            ),
            (
                "callback",
                "FORBIDDEN_FIELD",
                "/callback",
                json!("prompt-user"),
            ),
        ],
    )
}

fn negative_artifacts(base: &Value, vectors: Vec<(&str, &str, &str, Value)>) -> Vec<Artifact> {
    vectors
        .into_iter()
        .map(|(name, expected_code, pointer, replacement)| {
            let mut document = base.clone();
            set_pointer(&mut document, pointer, replacement);
            json_artifact(
                &format!("negative/{name}.json"),
                json!({ "expectedCode": expected_code, "document": document }),
            )
        })
        .collect()
}

fn descriptor(worker: &str, protocol: &str, version: &str, profile: &str) -> Value {
    json!({
        "worker": worker,
        "graphProfiles": ["openengine.graph.full/v1"],
        "binding": { "protocol": protocol, "version": version, "profile": profile },
        "contract": { "input": { "kind": "string" }, "output": { "kind": "string" },
            "verifier": null, "errors": ["timeout", "crash", "malformed", "refusal"] },
        "capabilityPolicy": { "autonomy": "strict", "permissionPolicy": "policy.strict@1" },
        "artifactProfile": { "allowedTypeIds": ["openengine.result@1"],
            "allowedMediaTypes": ["application/json"], "minimumRedaction": "internal" },
        "credentialRequirements": ["credential.test@1"]
    })
}

fn set_pointer(document: &mut Value, pointer: &str, value: Value) {
    let mut segments = pointer.strip_prefix('/').unwrap().split('/').peekable();
    let mut current = document;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            current[segment] = value;
            return;
        }
        current = &mut current[segment];
    }
}

fn json_artifact(suffix: &str, value: Value) -> Artifact {
    let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
    bytes.push(b'\n');
    Artifact {
        relative_path: format!("{ROOT}/fixtures/workers/{suffix}"),
        bytes,
    }
}
