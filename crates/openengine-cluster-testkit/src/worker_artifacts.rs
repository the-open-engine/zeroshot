//! Deterministic generated worker schemas and test-only conformance vectors.

mod compatibility;
use compatibility::compatibility_artifacts;

use crate::fixture::*;

use openengine_cluster_protocol::{WorkerDescriptor, WorkerOutcome, BUILTIN_PROFILE, BUILTIN_VERSION};
use schemars::schema_for;
use serde_json::{json, Value};

use crate::artifacts::Artifact;
use crate::schema_helpers::merge_schema;

const ROOT: &str = "protocol/openengine-cluster/v1";

#[must_use]
pub fn worker_schema() -> Value {
    let mut root = serde_json::to_value(schema_for!(WorkerDescriptor)).assert_value();
    merge_schema(
        &mut root,
        "WorkerOutcome",
        serde_json::to_value(schema_for!(WorkerOutcome)).assert_value(),
    );
    root
}

#[must_use]
pub fn with_worker_components(mut document: Value) -> Value {
    let schemas = document
        .assert_key_mut("components")
        .assert_key_mut("schemas")
        .as_object_mut()
        .assert_value_with("OpenRPC components.schemas must be an object");
    for (name, schema) in [
        ("WorkerDescriptor", json!({ "$ref": "worker.schema.json" })),
        (
            "WorkerOutcome",
            json!({ "$ref": "worker.schema.json#/$defs/WorkerOutcome" }),
        ),
    ] {
        schemas.insert(name.to_owned(), schema);
    }
    document
}

#[must_use]
pub fn worker_fixture_artifacts() -> Vec<Artifact> {
    let portable = descriptor("mock.worker@1", "fixture", "1", "fixture.worker/v1");
    let mut builtin = descriptor(
        "mock.builtin@1",
        "builtin",
        BUILTIN_VERSION,
        BUILTIN_PROFILE,
    );
    *builtin.assert_key_mut("credentialRequirements") = json!([]);

    let mut artifacts = positive_worker_artifacts(portable.clone(), builtin.clone());
    artifacts.extend(negative_contract_artifacts(&portable, &builtin));
    artifacts.extend(negative_secret_artifacts(&portable));
    artifacts.extend(negative_outcome_artifacts());
    artifacts.extend(compatibility_artifacts(&portable));
    artifacts
}

fn positive_worker_artifacts(portable: Value, builtin: Value) -> Vec<Artifact> {
    vec![
        json_artifact("positive/portable-binding.json", portable),
        json_artifact("positive/builtin-v1.json", builtin.clone()),
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
                "producer": { "node": "worker", "worker": "mock.worker@1" },
                "lineage": { "generation": 1, "runId": "run-1", "attempt": 1 },
                "redaction": "internal"
            }),
        ),
    ]
}

fn negative_contract_artifacts(portable: &Value, builtin: &Value) -> Vec<Artifact> {
    let mut artifacts = negative_descriptor_artifacts(
        portable,
        vec![
            (
                "invalid-binding-protocol",
                "INVALID_WORKER_BINDING",
                "/binding/protocol",
                json!("bad protocol"),
            ),
            (
                "invalid-binding-version",
                "INVALID_WORKER_BINDING",
                "/binding/version",
                json!(""),
            ),
            (
                "invalid-binding-profile",
                "INVALID_WORKER_BINDING",
                "/binding/profile",
                json!("bad profile"),
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
            (
                "duplicate-credentials",
                "DUPLICATE_CREDENTIAL_REQUIREMENTS",
                "/credentialRequirements",
                json!(["credential.test@1", "credential.test@1"]),
            ),
        ],
    );
    artifacts.extend(negative_descriptor_artifacts(
        builtin,
        vec![
            (
                "builtin-wrong-version",
                "UNSUPPORTED_WORKER_BINDING",
                "/binding/version",
                json!("2"),
            ),
            (
                "builtin-wrong-profile",
                "UNSUPPORTED_WORKER_BINDING",
                "/binding/profile",
                json!("openengine.worker.builtin/v2"),
            ),
            (
                "builtin-nonempty-credentials",
                "INVALID_BUILTIN_BINDING",
                "/credentialRequirements",
                json!(["credential.test@1"]),
            ),
        ],
    ));
    artifacts
}

fn negative_secret_artifacts(portable: &Value) -> Vec<Artifact> {
    negative_descriptor_artifacts(
        portable,
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

fn negative_descriptor_artifacts(
    base: &Value,
    vectors: Vec<(&str, &str, &str, Value)>,
) -> Vec<Artifact> {
    vectors
        .into_iter()
        .map(|(name, expected_code, pointer, replacement)| {
            let mut document = base.clone();
            set_pointer(&mut document, pointer, replacement);
            json_artifact(
                &format!("negative/{name}.json"),
                json!({
                    "fixtureKind": "descriptor",
                    "expectedCode": expected_code,
                    "document": document
                }),
            )
        })
        .collect()
}

fn negative_outcome_artifacts() -> Vec<Artifact> {
    [
        (
            "timeout-policy-reason",
            json!({ "status": "error", "code": "timeout", "reason": "policy_denied" }),
        ),
        (
            "malformed-auth-reason",
            json!({
                "status": "error", "code": "malformed", "reason": "authentication_required"
            }),
        ),
        (
            "refusal-malformed-reason",
            json!({ "status": "error", "code": "refusal", "reason": "malformed_result" }),
        ),
    ]
    .into_iter()
    .map(|(name, document)| {
        json_artifact(
            &format!("negative/{name}.json"),
            json!({
                "fixtureKind": "outcome",
                "expectedCode": "INVALID_FAILURE_PAIR",
                "document": document
            }),
        )
    })
    .collect()
}

fn mutate(base: &Value, pointer: &str, replacement: Value) -> Value {
    let mut value = base.clone();
    set_pointer(&mut value, pointer, replacement);
    value
}

fn descriptor(worker: &str, protocol: &str, version: &str, profile: &str) -> Value {
    json!({
        "worker": worker,
        "contract": { "input": { "kind": "string" }, "output": { "kind": "string" },
            "verifier": null, "errors": ["timeout", "crash", "malformed", "refusal"] },
        "binding": { "protocol": protocol, "version": version, "profile": profile },
        "graphProfiles": ["openengine.graph.full/v1"],
        "capabilityPolicy": { "autonomy": "strict", "permissionPolicy": "policy.strict@1" },
        "artifactProfile": { "allowedTypeIds": ["openengine.result@1"],
            "allowedMediaTypes": ["application/json"], "minimumRedaction": "internal" },
        "credentialRequirements": ["credential.test@1"]
    })
}

fn set_pointer(document: &mut Value, pointer: &str, value: Value) {
    let mut segments = pointer
        .strip_prefix('/')
        .assert_value()
        .split('/')
        .peekable();
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
    let mut bytes = serde_json::to_vec_pretty(&value).assert_value();
    bytes.push(b'\n');
    Artifact {
        relative_path: format!("{ROOT}/fixtures/workers/{suffix}"),
        bytes,
    }
}
