//! Request examples are generated with the rest of the protocol contract.
use crate::artifacts::{Artifact, json_artifact};
use serde_json::json;
pub(crate) fn artifacts() -> Vec<Artifact> {
    let root = "protocol/openengine-cluster/v1/fixtures/native_v2_observation";
    let request = json!({"id":"request-1","text":"Ready to continue?","signals":{"decision":["approved","rejected"]},"output":{"kind":"record","fields":{"feedback":{"required":true,"type":{"kind":"string"}}}},"expiresAt":1763510400000u64});
    vec![
        json_artifact(
            format!("{root}/request-discovery.json"),
            serde_json::to_value(
                openengine_cluster_protocol::TargetDiscoveryDocument::direct(
                    openengine_cluster_protocol::TargetAuthentication::None,
                )
                .with_run_suspension()
                .with_request_ask(),
            )
            .expect("discovery serialization"),
        ),
        json_artifact(
            format!("{root}/request-resume.json"),
            json!({"runId":"run-1","successorRunId":"run-2","from":{"kind":"answer","signals":{"decision":"approved"},"output":{"feedback":""}},"connections":{}}),
        ),
        json_artifact(
            format!("{root}/inbox-page.json"),
            json!({"requests":[{"runId":"run-1","node":"approval","createdAt":1762905600000u64,"expiresAt":1763510400000u64,"state":"pending"}],"nextAfter":"run-1"}),
        ),
        json_artifact(
            format!("{root}/suspended-request.json"),
            json!({"phase":"suspended","reason":"request","node":"approval","createdAt":1762905600000u64,"request":request,"metadata":{}}),
        ),
        json_artifact(
            format!("{root}/request-vector.json"),
            json!({"id":"request-1","kind":"request","runId":"run-1","node":"approval","createdAt":1762905600000u64,"text":request["text"],"signals":request["signals"],"output":request["output"],"expiresAt":request["expiresAt"]}),
        ),
        json_artifact(
            format!("{root}/answer-vector.json"),
            json!({"id":"answer-1","kind":"answer","runId":"run-2","createdAt":1762905600001u64,"requestId":"request-1","answer":{"signals":{"decision":"approved"},"output":{"feedback":""}}}),
        ),
        json_artifact(
            format!("{root}/expiry-vector.json"),
            json!({"id":"expiry-1","kind":"expiry","runId":"run-3","createdAt":1763510400000u64,"requestId":"request-1"}),
        ),
    ]
}
