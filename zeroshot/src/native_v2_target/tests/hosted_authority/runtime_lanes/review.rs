use super::*;
use zeroshot_engine::native_v2_review::ReviewResolutionRequest;

const COMMENT_REFUSAL: &str =
    "target does not support comment delivery (openengine.comment-delivery/v1)";

fn resolution_request() -> ReviewResolutionRequest {
    ReviewResolutionRequest {
        url: "https://github.com/acme/project/issues/42".to_owned(),
        branch: None,
        revision: None,
    }
}

fn runtime_cases() -> [(Value, &'static str); 2] {
    [
        (lane_binding(), REFUSAL),
        (
            json!({"kind":"comment_delivery","connections":{"github":["GH_TOKEN"]}}),
            COMMENT_REFUSAL,
        ),
    ]
}

#[tokio::test]
async fn hosted_task_resolution_checks_the_authored_runtime_before_access_tokens_or_posting() {
    for (binding, expected) in runtime_cases() {
        let root = temp_root();
        let target = RecordingTarget::spawn(|request, origin, tokens| {
            if (request.method.as_str(), request.path.as_str()) == DISCOVERY {
                let mut document: Value = serde_json::from_str(&hosted_discovery(origin)).assert_value();
                document["extensions"]["task_resolution"] = json!({"kind":"openengine.task-resolution/v1","resolve":"/native-v2/resolve-task"});
                return ("200 OK", document.to_string());
            }
            hosted_response(request, origin, tokens, false)
        }).await;
        let (record, authority) = authenticated_authority(&root, &target.origin).await;
        let runtime = run_request(binding).submission.runtime;
        let error = authority
            .resolve_review(&record, resolution_request(), &runtime)
            .await
            .assert_error();
        assert_eq!(error.to_string(), expected);
        assert_eq!(routes(&target.stop()), [DISCOVERY, OAUTH_METADATA]);
    }
}

#[tokio::test]
async fn direct_task_resolution_checks_the_authored_runtime_before_source_credentials_or_posting() {
    for (binding, expected) in runtime_cases() {
        let root = temp_root();
        let target = RecordingTarget::spawn(|request, _, _| {
            match (request.method.as_str(), request.path.as_str()) {
                DISCOVERY => (
                    "200 OK",
                    serde_json::to_string(
                        &TargetDiscoveryDocument::direct(TargetAuthentication::None)
                            .with_task_resolution(),
                    )
                    .assert_value(),
                ),
                _ => unexpected(),
            }
        })
        .await;
        let authority = test_http_authority(root.path("refresh-locks"));
        let runtime = run_request(binding).submission.runtime;
        let error = authority
            .resolve_review(
                &direct_target(target.origin.clone()),
                resolution_request(),
                &runtime,
            )
            .await
            .assert_error();
        assert_eq!(error.to_string(), expected);
        assert_eq!(routes(&target.stop()), [DISCOVERY]);
    }
}
