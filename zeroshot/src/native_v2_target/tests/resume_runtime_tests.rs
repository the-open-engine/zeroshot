use super::*;

#[tokio::test]
async fn direct_resume_runtime_authorizes_new_declarations_and_preserves_source_authority() {
    let fixture = direct_recovery_fixture().await;
    let runtime = serde_json::from_value(serde_json::json!({"kind":"uniform","runtime":{
        "harness":"claude","provider":"anthropic","model":"replacement",
        "connections":{"new-provider":["ANTHROPIC_API_KEY"]}
    }}))
    .assert_value();
    let requirements = fixture
        .connector
        .replacement_workspace_recovery_requirements("vm", &fixture.run_id, &runtime)
        .await
        .assert_value();
    let key = ConnectionKey::new("new-provider").assert_value();
    let field = EnvironmentVariableName::new("ANTHROPIC_API_KEY").assert_value();
    assert_eq!(
        requirements,
        BTreeMap::from([(key.clone(), vec![field.clone()])])
    );
    let params = RunResumeParams {
        runtime: Some(runtime),
        run_id: fixture.run_id.clone(),
        successor_run_id: RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c993"),
        from: None,
        connections: BTreeMap::from([(
            key,
            openengine_cluster_protocol::StaticConnectionValues::new(BTreeMap::from([(
                field,
                "fresh".to_owned(),
            )]))
            .assert_value(),
        )]),
        connection_resolver: None,
        github_token: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,
    };
    fixture
        .connector
        .prepare_workspace_recovery_resume("vm", &params)
        .await
        .assert_value();
    assert_eq!(
        fixture
            .registry
            .recovery_authorization(&fixture.target.id, &params.successor_run_id)
            .assert_value(),
        requirements
    );
    assert_eq!(
        fixture
            .registry
            .recovery_authorization(&fixture.target.id, &fixture.run_id)
            .assert_value(),
        fixture.trusted
    );
}

#[tokio::test]
async fn direct_resume_runtime_accepts_forge_default_names_without_expanding_secret_authority() {
    let registry = MemoryRegistry::default();
    let target = direct_target("http://127.0.0.1:8080");
    registry.insert(target.clone()).assert_value();
    let run_id = run_request().run_id;
    let original =
        serde_json::from_value(serde_json::json!({"custom-forge":["GITLAB_TOKEN"]})).assert_value();
    registry
        .record_recovery_authorization(&target.id, &run_id, &original)
        .assert_value();
    let dialer = FakeDialer::default();
    let expected: RunConnectionRequirements = serde_json::from_value(serde_json::json!({
        "gitlab":["GITLAB_TOKEN"],"new-provider":["ANTHROPIC_API_KEY"]
    }))
    .assert_value();
    *dialer.resume_requirements.lock().assert_value() = Some(expected.clone());
    let connector = NativeV2TargetConnector::new(
        registry.clone(),
        FakeAuthority::new("ws://127.0.0.1:8080/native-v2/oecp").with_workspace_recovery(),
        dialer.clone(),
    );
    let runtime = serde_json::from_value(serde_json::json!({"kind":"uniform","runtime":{
        "harness":"claude","provider":"anthropic","model":"fixture",
        "connections":{"new-provider":["ANTHROPIC_API_KEY"]}
    }}))
    .assert_value();
    assert_eq!(
        connector
            .replacement_workspace_recovery_requirements("vm", &run_id, &runtime)
            .await
            .assert_value(),
        expected
    );
    let connections = expected
        .iter()
        .map(|(key, fields)| {
            (
                key.clone(),
                openengine_cluster_protocol::StaticConnectionValues::new(
                    fields
                        .iter()
                        .map(|field| (field.clone(), "fresh-value".to_owned()))
                        .collect(),
                )
                .assert_value(),
            )
        })
        .collect();
    let params = RunResumeParams {
        run_id: run_id.clone(),
        successor_run_id: RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c993"),
        runtime: Some(runtime.clone()),
        from: None,
        connections,
        connection_resolver: None,
        github_token: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,
    };
    connector
        .prepare_workspace_recovery_resume("vm", &params)
        .await
        .assert_value();
    assert_eq!(
        registry
            .recovery_authorization(&target.id, &run_id)
            .assert_value(),
        original
    );
    for requirements in [
        serde_json::json!({"github":["GH_TOKEN"]}),
        serde_json::json!({"unexpected":["GITLAB_TOKEN"]}),
        serde_json::json!({"gitlab":["AWS_SECRET_ACCESS_KEY"]}),
    ] {
        *dialer.resume_requirements.lock().assert_value() =
            Some(serde_json::from_value(requirements).assert_value());
        assert!(
            connector
                .replacement_workspace_recovery_requirements("vm", &run_id, &runtime)
                .await
                .is_err()
        );
    }
}
