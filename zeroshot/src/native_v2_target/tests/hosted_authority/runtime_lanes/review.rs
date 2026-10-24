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

#[tokio::test]
async fn hosted_resolution_uses_target_authority_and_enforces_its_response_boundary() {
    for response in ["valid", "unauthorized", "invalid", "oversized"] {
        let root = temp_root();
        let target = RecordingTarget::spawn(move |request, origin, tokens| {
            match (request.method.as_str(), request.path.as_str()) {
                DISCOVERY => {
                    let mut document: Value = serde_json::from_str(&hosted_discovery(origin)).assert_value();
                    document["extensions"]["task_resolution"] = json!({"kind":"openengine.task-resolution/v1","resolve":"/native-v2/resolve-task"});
                    document["extensions"]["comment_delivery"] = json!({"kind":"openengine.comment-delivery/v1"});
                    ("200 OK", document.to_string())
                }
                ("POST", "/native-v2/resolve-task") => {
                    assert_eq!(request.authorization.as_deref(), Some("Bearer access-1"));
                    let body: Value = serde_json::from_str(&request.body).assert_value();
                    assert_eq!(body, json!({"url":"https://github.com/acme/project/issues/42"}));
                    match response {
                        "unauthorized" => ("401 Unauthorized", "{}".into()),
                        "invalid" => ("200 OK", "{".into()),
                        "oversized" => ("200 OK", "x".repeat(2 * 1024 * 1024 + 1)),
                        _ => ("200 OK", json!({
                            "initialInput":{"url":"https://github.com/acme/project/issues/42","context":{"title":"Proposal","body":"Assess this","reviewScope":{}}},
                            "source":{"repository":"acme/project","branch":"main","revision":"a".repeat(40)}
                        }).to_string()),
                    }
                }
                _ => hosted_response(request, origin, tokens, false),
            }
        }).await;
        let (record, authority) = authenticated_authority(&root, &target.origin).await;
        let runtime = run_request(json!({"kind":"comment_delivery"}))
            .submission
            .runtime;
        let result = authority
            .resolve_review(&record, resolution_request(), &runtime)
            .await;
        if response == "valid" {
            let prepared = result.assert_value();
            assert_eq!(prepared.source.repository.as_str(), "acme/project");
            assert_eq!(prepared.initial_input["context"]["title"], "Proposal");
        } else {
            assert!(result.is_err(), "accepted {response} response");
        }
        let requests = target.stop();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path == "/native-v2/resolve-task")
                .count(),
            1
        );
    }
}
