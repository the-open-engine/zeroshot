use openengine_cluster_protocol::{
    ConnectionDeleteRequest, ConnectionKey, ConnectionListRequest, ConnectionScope,
    ConnectionSetRequest, EnvironmentVariableName, MergePlanId, RunForceParams, RunId,
    RunListParams, RunLogsParams, RunProfileDefaultRequest, RunProfileListRequest, RunProfileName,
    RunProfileRunRequest, RunProfileScope, RunProfileSelector, RunProfileSetRequest,
    RunStatusParams, RunWatchParams, StaticConnectionValues,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::super::*;
use super::fixtures::{
    direct_target, hosted_target, run_request, AuthorityCall, FakeAuthority, FakeDialer,
    MemoryRegistry,
};

fn profile_operations() -> (
    TargetRecord,
    RunProfileSelector,
    RunProfileSetRequest,
    RunProfileRunRequest,
) {
    let target = direct_target("http://127.0.0.1:8080");
    let prepared = run_request();
    let selector = RunProfileSelector {
        scope: RunProfileScope::User,
        name: RunProfileName::new("contract").assert_value(),
    };
    let set = RunProfileSetRequest {
        name: selector.name.clone(),
        scope: selector.scope,
        graph: prepared.intent.graph.clone(),
        runtime: prepared.intent.runtime.clone(),
        set_default: true,
    };
    let run = RunProfileRunRequest {
        run_id: prepared.run_id,
        profile: selector.clone(),
        title: prepared.intent.title,
        initial_input: prepared.intent.initial_input,
        source: prepared.source.assert_value().resolved,
        submission_key: prepared.intent.submission_key,
        connections: prepared.connections,
        github_token: prepared.github_token,
    };
    (target, selector, set, run)
}

#[test]
fn profile_run_debug_exposes_routing_identity_and_redacts_every_secret_value() {
    let (_, _, _, mut run) = profile_operations();
    run.connections.insert(
        ConnectionKey::new("provider").assert_value(),
        StaticConnectionValues::new(std::collections::BTreeMap::from([(
            EnvironmentVariableName::new("API_KEY").assert_value(),
            "connection-secret".to_owned(),
        )]))
        .assert_value(),
    );
    run.github_token = Some("github-secret".to_owned());

    let diagnostic = format!("{run:?}");
    for public in [
        "RunProfileRunRequest",
        run.run_id.as_str(),
        run.profile.name.as_str(),
        "provider",
        "[REDACTED]",
    ] {
        assert!(
            diagnostic.contains(public),
            "missing {public:?}: {diagnostic}"
        );
    }
    for secret in ["connection-secret", "github-secret", "API_KEY"] {
        assert!(
            !diagnostic.contains(secret),
            "debug output exposed {secret:?}: {diagnostic}"
        );
    }
}

fn hosted_connector(
    authority: FakeAuthority,
) -> NativeV2TargetConnector<MemoryRegistry, FakeAuthority, FakeDialer> {
    let registry = MemoryRegistry::default();
    registry
        .insert(hosted_target("cloud", "https://target.example"))
        .assert_value();
    NativeV2TargetConnector::new(registry, authority, FakeDialer::default())
}

#[test]
fn target_origins_match_the_existing_hosted_cli_contract() {
    assert_eq!(
        normalize_origin("https://target.example").assert_value(),
        "https://target.example"
    );
    assert_eq!(
        normalize_origin("http://127.0.0.1:8080").assert_value(),
        "http://127.0.0.1:8080"
    );
    for invalid in [
        "http://target.example",
        "https://user@target.example",
        "https://target.example/path",
        "https://target.example?query=1",
        "https://target.example/#fragment",
    ] {
        assert!(normalize_origin(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn target_access_is_explicit_and_hosted_remains_the_default() {
    let hosted = prepare_target(TargetAdd {
        name: "cloud".to_owned(),
        url: "https://target.example".to_owned(),
        direct: false,
    })
    .assert_value();
    assert!(matches!(hosted.access, TargetAccess::Hosted { .. }));

    let direct = prepare_target(TargetAdd {
        name: "vm".to_owned(),
        url: "http://127.0.0.1:8080".to_owned(),
        direct: true,
    })
    .assert_value();
    assert_eq!(direct.access, TargetAccess::Direct);
}

#[tokio::test]
async fn authorities_without_profile_support_fail_closed_for_every_profile_operation() {
    let authority = FakeAuthority::new("ws://127.0.0.1:1/native-v2/oecp");
    let (target, selector, set, run) = profile_operations();

    let messages = [
        authority
            .profile_list(
                &target,
                RunProfileListRequest {
                    scope: RunProfileScope::User,
                },
            )
            .await
            .assert_error()
            .to_string(),
        authority
            .profile_show(&target, selector.clone())
            .await
            .assert_error()
            .to_string(),
        authority
            .profile_set(&target, set.clone())
            .await
            .assert_error()
            .to_string(),
        authority
            .profile_delete(&target, selector.clone())
            .await
            .assert_error()
            .to_string(),
        authority
            .profile_default(
                &target,
                RunProfileDefaultRequest {
                    scope: RunProfileScope::User,
                    name: Some(selector.name.clone()),
                },
            )
            .await
            .assert_error()
            .to_string(),
    ];
    assert!(
        messages
            .iter()
            .all(|message| message == "profile management is unavailable")
    );
    assert_eq!(
        authority
            .profile_run(&target, &run)
            .await
            .assert_error()
            .to_string(),
        "profile runs are unavailable"
    );

    assert!(authority.calls().is_empty());
}

#[tokio::test]
async fn connector_preserves_fail_closed_profile_and_merge_plan_errors() {
    let authority = FakeAuthority::new("ws://127.0.0.1:1/native-v2/oecp");
    let (target, selector, set, _) = profile_operations();
    let registry = MemoryRegistry::default();
    registry.insert(target.clone()).assert_value();
    let connector =
        NativeV2TargetConnector::new(registry, authority.clone(), FakeDialer::default());
    let connector_messages = [
        connector
            .profile_list(
                "vm",
                RunProfileListRequest {
                    scope: RunProfileScope::User,
                },
            )
            .await
            .assert_error()
            .to_string(),
        connector
            .profile_show("vm", selector.clone())
            .await
            .assert_error()
            .to_string(),
        connector
            .profile_set("vm", set)
            .await
            .assert_error()
            .to_string(),
        connector
            .profile_delete("vm", selector.clone())
            .await
            .assert_error()
            .to_string(),
        connector
            .profile_default(
                "vm",
                RunProfileDefaultRequest {
                    scope: RunProfileScope::User,
                    name: Some(selector.name),
                },
            )
            .await
            .assert_error()
            .to_string(),
    ];
    assert!(
        connector_messages
            .iter()
            .all(|message| message.contains("profile management is unavailable"))
    );
    for error in [
        connector
            .merge_plan_status("vm", MergePlanId::new("plan-1"))
            .await
            .assert_error(),
        connector
            .merge_plan_force("vm", MergePlanId::new("plan-1"))
            .await
            .assert_error(),
    ] {
        assert!(
            error
                .to_string()
                .contains("hosted merge plans are unavailable")
        );
    }
    assert!(authority.calls().is_empty());
}

#[tokio::test]
async fn connector_preserves_connection_and_checkpoint_authority_failures() {
    let authority = FakeAuthority::new("ws://127.0.0.1:1/native-v2/oecp");
    let connector = hosted_connector(authority.clone());
    let key = ConnectionKey::new("github").assert_value();
    let values = StaticConnectionValues::new(std::collections::BTreeMap::from([(
        EnvironmentVariableName::new("GH_TOKEN").assert_value(),
        "private".to_owned(),
    )]))
    .assert_value();
    let errors = [
        connector
            .connection_list(
                "cloud",
                ConnectionListRequest {
                    scope: ConnectionScope::User,
                },
            )
            .await
            .assert_error(),
        connector
            .connection_set(
                "cloud",
                ConnectionSetRequest {
                    key: key.clone(),
                    scope: ConnectionScope::User,
                    values,
                },
            )
            .await
            .assert_error(),
        connector
            .connection_delete(
                "cloud",
                ConnectionDeleteRequest {
                    key,
                    scope: ConnectionScope::User,
                },
            )
            .await
            .assert_error(),
    ];
    assert!(errors.iter().all(|error| {
        error
            .to_string()
            .contains("fake connection management is unavailable")
    }));
    let checkpoint_error = connector
        .connect_workspace_checkpoints("cloud", RunId::new("checkpoint-run"))
        .await
        .assert_error()
        .to_string();
    assert!(
        checkpoint_error.contains("does not advertise workspace checkpoints"),
        "{checkpoint_error}"
    );
    assert!(authority.calls().is_empty());
}

#[tokio::test]
async fn connector_preserves_hosted_lifecycle_failures_without_fallback() {
    let authority = FakeAuthority::new("ws://127.0.0.1:1/native-v2/oecp");
    let connector = hosted_connector(authority.clone());
    let run_id = RunId::new("run-hosted");
    let errors = [
        connector
            .hosted_run_list("cloud", RunListParams {})
            .await
            .assert_error(),
        connector
            .hosted_run_status(
                "cloud",
                RunStatusParams {
                    run_id: run_id.clone(),
                },
            )
            .await
            .assert_error(),
        connector
            .hosted_run_watch(
                "cloud",
                RunWatchParams {
                    run_id: run_id.clone(),
                    from_cursor: None,
                },
            )
            .await
            .assert_error(),
        connector
            .hosted_run_logs(
                "cloud",
                RunLogsParams {
                    run_id: run_id.clone(),
                    from_cursor: None,
                    execution: None,
                },
            )
            .await
            .assert_error(),
        connector
            .hosted_run_force("cloud", RunForceParams { run_id })
            .await
            .assert_error(),
    ];
    assert!(errors.iter().all(|error| {
        error
            .to_string()
            .contains("fake hosted lifecycle is unavailable")
    }));
    assert!(authority.calls().is_empty());
}

#[tokio::test]
async fn connector_delegates_login_and_rejects_an_unresolved_source_before_submission() {
    let authority = FakeAuthority::new("ws://127.0.0.1:1/native-v2/oecp");
    let connector = hosted_connector(authority.clone());

    connector.login("cloud").await.assert_value();
    let mut request = run_request();
    request.source = None;
    let error = connector
        .submit("cloud", request)
        .await
        .assert_error()
        .to_string();
    assert!(error.contains("requires a resolved worktree source"));
    assert!(matches!(
        authority.calls().as_slice(),
        [AuthorityCall::Login(_)]
    ));
}

#[test]
fn connector_errors_hide_transport_details_but_preserve_typed_failures() {
    let target = hosted_target("cloud", "https://target.example");
    let disconnected = cli_connector_error(
        &target,
        TargetConnectorError::OecpConnection("private endpoint detail".to_owned()),
    )
    .to_string();
    assert!(!disconnected.contains("private endpoint detail"));
    assert!(disconnected.contains("WebSocket connection failed"));

    let invalid_token = cli_connector_error(&target, TargetConnectorError::InvalidBearerToken);
    assert_eq!(
        invalid_token.to_string(),
        "target operation failed: target OECP bearer token is invalid"
    );
    assert!(matches!(
        invalid_token,
        NativeV2CliError::Target(message)
            if message == TargetConnectorError::InvalidBearerToken.to_string()
    ));
}
