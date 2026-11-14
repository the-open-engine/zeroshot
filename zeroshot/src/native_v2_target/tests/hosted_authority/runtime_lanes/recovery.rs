use super::*;
use openengine_cluster_protocol::{
    RunId, RunSuspendParams, RunVectorParams, RunVectorsParams, VectorId,
};

fn control_document(origin: &str) -> Value {
    let mut document = hosted_document(origin);
    document["extensions"]["run_suspension"] = json!({"kind":"openengine.run-suspension/v1"});
    document
}

#[tokio::test]
async fn hosted_suspend_uses_the_advertised_authenticated_http_route() {
    let root = temp_root();
    let target = RecordingTarget::spawn(|request, origin, tokens| {
        match (request.method.as_str(), request.path.as_str()) {
            DISCOVERY => ("200 OK", control_document(origin).to_string()),
            ("POST", "/native-v2/runs/run-recoverable/suspend") => ("200 OK", json!({
                "runId":"run-recoverable", "title":"Suspend", "source":null, "size":"medium", "atCursor":"1",
                "status":{"phase":"running","activeExecutions":[],"suspendRequested":true}
            }).to_string()),
            _ => hosted_response(request, origin, tokens, true),
        }
    }).await;
    let (record, authority) = authenticated_authority(&root, &target.origin).await;
    let result = authority
        .hosted_run_suspend(
            &record,
            RunSuspendParams {
                run_id: RunId::new("run-recoverable"),
            },
        )
        .await
        .assert_value()
        .assert_value();
    assert!(matches!(
        result.status,
        openengine_cluster_protocol::HostedRunStatus::Target(
            openengine_cluster_protocol::RunStatus::Running {
                suspend_requested: true,
                ..
            }
        )
    ));
    let requests = target.stop();
    assert_eq!(
        routes(&requests),
        [
            DISCOVERY,
            OAUTH_METADATA,
            TOKEN,
            SESSION,
            ("POST", "/native-v2/runs/run-recoverable/suspend")
        ]
    );
    let request = requests.last().assert_value();
    assert_eq!(request.authorization.as_deref(), Some("Bearer access-1"));
    assert_eq!(
        serde_json::from_str::<Value>(&request.body).assert_value(),
        json!({"runId":"run-recoverable"})
    );
}

#[tokio::test]
async fn absent_hosted_control_routes_fall_back_without_resolving_access_tokens() {
    for operation in 0..3 {
        let root = temp_root();
        let target = RecordingTarget::spawn(|request, origin, tokens| {
            if (request.method.as_str(), request.path.as_str()) == DISCOVERY {
                let mut document = control_document(origin);
                document["extensions"]
                    .as_object_mut()
                    .assert_value()
                    .remove("hosted_run_controls");
                ("200 OK", document.to_string())
            } else {
                hosted_response(request, origin, tokens, true)
            }
        })
        .await;
        let (record, authority) = authenticated_authority(&root, &target.origin).await;
        let run_id = RunId::new("run-recoverable");
        let missing = match operation {
            0 => authority
                .hosted_run_suspend(&record, RunSuspendParams { run_id })
                .await
                .assert_value()
                .is_none(),
            1 => authority
                .hosted_run_vectors(
                    &record,
                    RunVectorsParams {
                        run_id,
                        after: None,
                        limit: None,
                    },
                )
                .await
                .assert_value()
                .is_none(),
            _ => authority
                .hosted_run_vector(
                    &record,
                    RunVectorParams {
                        run_id,
                        id: VectorId::new("entry").assert_value(),
                    },
                )
                .await
                .assert_value()
                .is_none(),
        };
        assert!(missing);
        let requests = target.stop();
        assert_eq!(routes(&requests), [DISCOVERY, OAUTH_METADATA]);
        assert!(requests.iter().all(|request| request.body.is_empty()));
    }
}
