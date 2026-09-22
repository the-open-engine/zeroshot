use openengine_cluster_protocol::{
    CheckpointId, RunCheckpointsParams, RunConnectionRequirements, RunDiscardWorkspaceParams,
    RunId, RunResumeFrom, RunResumeParams,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use zeroshot_engine::native_v2_cli::NativeV2CliBackend;

use super::fixtures::temp_root;
use super::hosted_authority::{CapturedHttpRequest, hosted_backend};

#[tokio::test]
async fn hosted_recovery_uses_authenticated_http_without_the_deleted_capsules_transport() {
    let root = temp_root();
    let (backend, dialer, server) = hosted_backend(&root, 14).await;
    let run_id = RunId::new("run-recoverable");
    let requirements: RunConnectionRequirements = serde_json::from_value(json!({
        "provider": ["ANTHROPIC_API_KEY"]
    }))
    .assert_value();
    assert!(
        backend
            .authorize_resume_connection_requirements(Some("prod"), &run_id, requirements)
            .await
            .assert_value()
            .is_empty()
    );

    for from in [
        None,
        Some(RunResumeFrom::Checkpoint {
            checkpoint_id: CheckpointId::new("before/group").assert_value(),
        }),
    ] {
        let result = backend
            .run_resume(
                Some("prod"),
                RunResumeParams {
                    run_id: run_id.clone(),
                    successor_run_id: RunId::new("successor"),
                    from,
                    connections: Default::default(),
                    connection_resolver: None,
                    github_token: None,
                },
            )
            .await
            .assert_value();
        assert_eq!(result.run_id, RunId::new("successor"));
        assert_eq!(result.resumed_from, run_id);
    }
    let checkpoints = backend
        .run_checkpoints(
            Some("prod"),
            RunCheckpointsParams {
                run_id: run_id.clone(),
                after: Some(CheckpointId::new("page/after").assert_value()),
                limit: Some(5),
            },
        )
        .await
        .assert_value();
    assert_eq!(checkpoints.run_id, run_id);
    let discarded = backend
        .run_discard_workspace(
            Some("prod"),
            RunDiscardWorkspaceParams {
                run_id: run_id.clone(),
            },
        )
        .await
        .assert_value();
    assert!(discarded.discarded);
    assert_eq!(discarded.run_id, run_id);
    assert!(dialer.sessions.lock().assert_value().is_empty());
    let requests = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .assert_value()
        .assert_value();
    assert_recovery_requests(&requests);
}

fn assert_recovery_requests(requests: &[CapturedHttpRequest]) {
    let recovery = requests
        .iter()
        .filter(|request| request.path.starts_with("/native-v2/runs/"))
        .collect::<Vec<_>>();
    assert_eq!(recovery.len(), 4);
    for request in &recovery {
        assert_eq!(request.method, "POST");
        assert_eq!(request.authorization.as_deref(), Some("Bearer access-1"));
        assert_eq!(
            serde_json::from_str::<Value>(&request.body).assert_value()["runId"],
            "run-recoverable"
        );
    }
    let bodies = recovery
        .iter()
        .map(|request| serde_json::from_str::<Value>(&request.body).assert_value())
        .collect::<Vec<_>>();
    assert!(bodies[0].get("from").is_none());
    assert_eq!(
        bodies[1]["from"],
        json!({"kind":"checkpoint", "checkpointId":"before/group"})
    );
    assert_eq!(bodies[2]["after"], "page/after");
    assert_eq!(bodies[2]["limit"], 5);
    assert!(
        requests
            .iter()
            .all(|request| request.path != "/native-v2/oecp-session")
    );
}

pub(super) fn response(request: &CapturedHttpRequest) -> Option<String> {
    if request.method != "POST" {
        return None;
    }
    let suffix = request
        .path
        .strip_prefix("/native-v2/runs/run-recoverable/")?;
    let body: Value = serde_json::from_str(&request.body).assert_value();
    Some(
        match suffix {
            "resume" => json!({"runId": body["successorRunId"], "resumedFrom": body["runId"]}),
            "checkpoints" => json!({"runId": body["runId"], "checkpoints": []}),
            "discard-workspace" => json!({"runId": body["runId"], "discarded": true}),
            _ => return None,
        }
        .to_string(),
    )
}
