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

#[tokio::test]
async fn direct_resolution_uses_isolated_credentials_and_preserves_target_authority() {
    const ORIGIN: &str = "ZEROSHOT_TEST_DIRECT_REVIEW_ORIGIN";
    const MODE: &str = "ZEROSHOT_TEST_DIRECT_REVIEW_MODE";
    const COMPLETE: &str = "direct review assertions completed";
    if let Ok(origin) = std::env::var(ORIGIN) {
        let root = temp_root();
        let authority = test_http_authority(root.path("refresh-locks"));
        let record = direct_target(origin);
        let mode = std::env::var(MODE).assert_value();
        let namespace = authority.review_cache_namespace(&record).await;
        if mode == "missing" {
            assert!(namespace.is_err());
        } else {
            assert_eq!(namespace.assert_value().assert_value().len(), 64);
        }
        let runtime = run_request(json!({"kind":"comment_delivery"}))
            .submission
            .runtime;
        let resolved = authority
            .resolve_review(&record, resolution_request(), &runtime)
            .await;
        if matches!(mode.as_str(), "missing" | "invalid") {
            assert!(resolved.is_err());
        } else {
            assert_eq!(
                resolved.assert_value().source.repository.as_str(),
                "acme/project"
            );
        }
        println!("{COMPLETE}");
        return;
    }
    for mode in ["primary", "fallback", "missing", "invalid"] {
        let target = RecordingTarget::spawn(|request, _, _| match (request.method.as_str(), request.path.as_str()) {
            DISCOVERY => ("200 OK", serde_json::to_string(&TargetDiscoveryDocument::direct(TargetAuthentication::None).with_comment_delivery().with_task_resolution()).assert_value()),
            ("POST", "/native-v2/tasks/resolve") => {
                assert!(request.authorization.is_none());
                let body: Value = serde_json::from_str(&request.body).assert_value();
                assert_eq!(body, json!({"request":{"url":"https://github.com/acme/project/issues/42"},"githubToken":"direct-fixture-token"}));
                ("200 OK", json!({"initialInput":{"url":"https://github.com/acme/project/issues/42","context":{}},"source":{"repository":"acme/project","branch":"main","revision":"a".repeat(40)}}).to_string())
            }
            _ => unexpected(),
        }).await;
        let mut child = tokio::process::Command::new(std::env::current_exe().assert_value());
        child.args(["--exact", "native_v2_target::tests::hosted_authority::runtime_lanes::review::direct_resolution_uses_isolated_credentials_and_preserves_target_authority", "--nocapture"])
            .env(ORIGIN, &target.origin).env(MODE, mode).env_remove("GH_TOKEN").env_remove("GITHUB_TOKEN");
        match mode {
            "primary" => {
                child
                    .env("GH_TOKEN", "direct-fixture-token")
                    .env("GITHUB_TOKEN", "must-not-win");
            }
            "fallback" => {
                child.env("GITHUB_TOKEN", "direct-fixture-token");
            }
            "invalid" => {
                child.env("GH_TOKEN", "invalid\ncredential");
            }
            _ => {}
        }
        let output = child.output().await.assert_value();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(COMPLETE));
        let requests = target.stop();
        let posts = requests
            .iter()
            .filter(|request| request.path == "/native-v2/tasks/resolve")
            .count();
        assert_eq!(posts, usize::from(matches!(mode, "primary" | "fallback")));
    }
}
