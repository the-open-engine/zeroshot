use super::*;

#[tokio::test]
async fn context_sources_require_complete_ordered_pins_after_environment_selection() {
    let mut request = submission(
        graph(vec![null_step("work", "agent.work@1"), succeed("done")]),
        BTreeMap::from([(named("work"), binding("model", None))]),
    );
    request.environment = Some(
        serde_json::from_value(
            json!({"contextRepositories":[{"repository":"open-engine/library","branch":"main"}]}),
        )
        .assert_value(),
    );
    assert!(matches!(
        NativeV2Admission.admit(request.clone()).await,
        Err(NativeV2AdmissionError::InvalidEnvironment(_))
    ));
    let pin: ResolvedSource = serde_json::from_value(
        json!({"repository":"open-engine/library","branch":"main","revision":"b".repeat(40)}),
    )
    .assert_value();
    request.context_sources = vec![pin.clone()];
    let admitted = NativeV2Admission
        .admit(request.clone())
        .await
        .assert_value();
    assert_eq!(admitted.context_sources, vec![pin.clone()]);
    for pins in [
        vec![pin.clone(), pin.clone()],
        vec![resolved_source()],
        vec![ResolvedSource {
            branch: openengine_cluster_protocol::SourceBranchId::new("other").assert_value(),
            ..pin.clone()
        }],
    ] {
        request.context_sources = pins;
        assert!(matches!(
            NativeV2Admission.admit(request.clone()).await,
            Err(NativeV2AdmissionError::InvalidEnvironment(_))
        ));
    }
    request.context_sources = vec![pin];
    request.environment = None;
    assert!(NativeV2Admission.admit(request).await.is_err());
}

#[tokio::test]
async fn context_sources_cannot_alias_the_primary_source_through_percent_escapes() {
    let mut request = submission(
        graph(vec![null_step("work", "agent.work@1"), succeed("done")]),
        BTreeMap::from([(named("work"), binding("model", None))]),
    );
    request.environment = Some(
        serde_json::from_value(json!({
            "contextRepositories":[{"repository":"https://github.com/open-engine/%7aeroshot.git"}]
        }))
        .assert_value(),
    );
    request.context_sources = vec![
        serde_json::from_value(json!({
            "repository":"open-engine/%7aeroshot", "branch":"main", "revision":"b".repeat(40),
            "remote":{"url":"https://github.com/open-engine/%7aeroshot.git"}
        }))
        .assert_value(),
    ];
    assert!(matches!(
        NativeV2Admission.admit(request).await,
        Err(NativeV2AdmissionError::InvalidEnvironment(_))
    ));
}

#[tokio::test]
async fn context_git_trust_overrides_fail_before_preparation_on_every_core_lane() {
    for (harness, provider) in [
        ("codex", "openai"),
        ("claude", "anthropic"),
        ("copilot", "github"),
        ("pi", "openai"),
    ] {
        for name in [
            "GIT_CONFIG_GLOBAL",
            "Git_Config_Global",
            "GIT_CONFIG_PARAMETERS",
            "Git_Config_Parameters",
            "GIT_CONFIG_COUNT",
            "Git_Config_Count",
        ] {
            for carrier in ["variables", "hook", "node"] {
                let mut request = submission(
                    graph(vec![null_step("work", "agent.work@1"), succeed("done")]),
                    BTreeMap::from([(named("work"), binding("opaque-model", None))]),
                );
                let mut runtime = serde_json::to_value(&request.runtime).assert_value();
                runtime["harness"] = json!(harness);
                runtime["provider"] = json!(provider);
                if carrier == "node" {
                    runtime["nodes"]["work"]["connections"] = json!({"test":[name]});
                }
                request.runtime = serde_json::from_value(runtime).assert_value();
                let mut environment = json!({});
                match carrier {
                    "variables" => {
                        let value = if name.eq_ignore_ascii_case("GIT_CONFIG_PARAMETERS") {
                            "'safe.directory='"
                        } else if name.eq_ignore_ascii_case("GIT_CONFIG_COUNT") {
                            "1"
                        } else {
                            "/dev/null"
                        };
                        environment["variables"] = json!({name:value});
                    }
                    "hook" => environment["connections"] = json!({"test":[name]}),
                    _ => (),
                }
                // Existing context-free requests retain all caller Git settings.
                request.environment =
                    Some(serde_json::from_value(environment.clone()).assert_value());
                NativeV2Admission
                    .admit(request.clone())
                    .await
                    .assert_value();
                environment["contextRepositories"] = json!([{"repository":"acme/library"}]);
                request.environment = Some(serde_json::from_value(environment).assert_value());
                request.context_sources = vec![
                    serde_json::from_value(json!({
                        "repository":"acme/library", "branch":"main", "revision":"b".repeat(40)
                    }))
                    .assert_value(),
                ];
                let intent: RunSubmissionIntent = serde_json::from_value(json!({
                    "title":request.title, "graph":request.graph, "initialInput":request.initial_input,
                    "runtime":request.runtime, "environment":request.environment,
                    "submissionKey":request.submission_key
                })).assert_value();
                for error in [
                    NativeV2Admission
                        .validate_intent(&intent, DeliveryPolicy::Optional)
                        .await
                        .unwrap_err(),
                    NativeV2Admission.admit(request).await.unwrap_err(),
                ] {
                    assert!(
                        matches!(&error, NativeV2AdmissionError::InvalidEnvironment(detail)
                        if detail.contains("target-owned Git trust")),
                        "{harness}/{provider} {carrier} {name}: {error}"
                    );
                }
            }
        }
    }
}
