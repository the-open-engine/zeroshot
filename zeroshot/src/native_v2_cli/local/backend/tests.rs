use std::collections::BTreeMap;
use std::path::PathBuf;

use openengine_cluster_protocol::{
    ConnectionKey, ConnectionScope, EnvironmentVariableName, ExecutionRef,
    RunDiscardWorkspaceParams, RunProfileName, RunProfileScope, RunResumeParams,
    StaticConnectionValues,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::native_v2_candidate::test_support::{full_graph, success_node};

fn run_id(value: &str) -> RunId {
    RunId::new(value)
}

fn prepared_request(run_id: RunId) -> PreparedRunRequest {
    let intent = serde_json::from_value(json!({
        "title":"Local backend routing",
        "graph": full_graph(vec![success_node()]),
        "initialInput":null,
        "runtime":{"harness":"codex","provider":"openai","size":"small","nodes":{}},
        "submissionKey":"local-backend-routing"
    }))
    .assert_value();
    PreparedRunRequest {
        run_id,
        intent,
        connections: BTreeMap::new(),
        github_token: None,
        source: None,
        profile: None,
    }
}

fn assert_named_target_rejected<T>(result: Result<T, NativeV2CliError>) {
    assert!(matches!(
        result,
        Err(NativeV2CliError::Local(message))
            if message == "local backend cannot serve a named target"
    ));
}

async fn exercise_named_profile_rejections(backend: &LocalCliBackend) {
    let profile_name = RunProfileName::new("local").assert_value();
    let request = prepared_request(run_id("0199f33f-3b44-7d21-9000-000000000001"));

    assert_named_target_rejected(
        backend
            .profile_list(
                Some("prod"),
                RunProfileListRequest {
                    scope: RunProfileScope::User,
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .profile_show(
                Some("prod"),
                RunProfileSelector {
                    scope: RunProfileScope::User,
                    name: profile_name.clone(),
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .profile_set(
                Some("prod"),
                RunProfileSetRequest {
                    name: profile_name.clone(),
                    scope: RunProfileScope::User,
                    graph: request.intent.graph.clone(),
                    runtime: request.intent.runtime.clone(),
                    set_default: true,
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .profile_delete(
                Some("prod"),
                RunProfileSelector {
                    scope: RunProfileScope::User,
                    name: profile_name.clone(),
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .profile_default(
                Some("prod"),
                RunProfileDefaultRequest {
                    scope: RunProfileScope::User,
                    name: Some(profile_name),
                },
            )
            .await,
    );
}

async fn exercise_named_run_rejections(backend: &LocalCliBackend) {
    let run_id = run_id("0199f33f-3b44-7d21-9000-000000000001");
    let successor = RunId::new("0199f33f-3b44-7d21-9000-000000000002");
    let request = prepared_request(run_id.clone());

    assert_named_target_rejected(backend.run_submit(Some("prod"), request).await);
    assert_named_target_rejected(
        backend
            .run_list(Some("prod"), RunListParams::default())
            .await,
    );
    assert_named_target_rejected(
        backend
            .run_status(
                Some("prod"),
                RunStatusParams {
                    run_id: run_id.clone(),
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .run_watch(
                Some("prod"),
                RunWatchParams {
                    run_id: run_id.clone(),
                    from_cursor: None,
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .run_logs(
                Some("prod"),
                RunLogsParams {
                    run_id: run_id.clone(),
                    from_cursor: None,
                    execution: None,
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .run_attach(
                Some("prod"),
                RunAttachParams {
                    run_id: run_id.clone(),
                    execution: ExecutionRef::new("execution").assert_value(),
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .run_force(
                Some("prod"),
                RunForceParams {
                    run_id: run_id.clone(),
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .run_resume(
                Some("prod"),
                RunResumeParams {
                    run_id: run_id.clone(),
                    successor_run_id: successor,
                    connections: BTreeMap::new(),
                    connection_resolver: None,
                    github_token: None,
                },
            )
            .await,
    );
    assert_named_target_rejected(
        backend
            .run_discard_workspace(Some("prod"), RunDiscardWorkspaceParams { run_id })
            .await,
    );
}

async fn exercise_absent_local_run_failures(backend: &LocalCliBackend) {
    let run_id = run_id("0199f33f-3b44-7d21-9000-000000000031");
    assert!(
        backend
            .run_submit(None, prepared_request(run_id.clone()))
            .await
            .is_err()
    );
    assert!(matches!(
        backend
            .run_status(
                None,
                RunStatusParams {
                    run_id: run_id.clone(),
                },
            )
            .await,
        Err(NativeV2CliError::RunNotFound { .. })
    ));
    assert!(
        backend
            .run_watch(
                None,
                RunWatchParams {
                    run_id: run_id.clone(),
                    from_cursor: None,
                },
            )
            .await
            .is_err()
    );
    assert!(
        backend
            .run_logs(
                None,
                RunLogsParams {
                    run_id: run_id.clone(),
                    from_cursor: None,
                    execution: None,
                },
            )
            .await
            .is_err()
    );
    assert!(
        backend
            .run_attach(
                None,
                RunAttachParams {
                    run_id: run_id.clone(),
                    execution: ExecutionRef::new("execution").assert_value(),
                },
            )
            .await
            .is_err()
    );
    assert!(
        backend
            .run_force(
                None,
                RunForceParams {
                    run_id: run_id.clone(),
                },
            )
            .await
            .is_err()
    );
    assert!(
        backend
            .run_resume(
                None,
                RunResumeParams {
                    run_id,
                    successor_run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000032",),
                    connections: BTreeMap::new(),
                    connection_resolver: None,
                    github_token: None,
                },
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn local_backend_contract_delegates_storage_and_rejects_remote_routes() {
    let root = tempfile::tempdir().assert_value();
    let backend = LocalCliBackend::new(
        root.path().to_path_buf(),
        PathBuf::from("zeroshot"),
        root.path().to_path_buf(),
        PathBuf::from("git"),
    );
    let key = ConnectionKey::new("openai").assert_value();
    let field = EnvironmentVariableName::new("OPENAI_API_KEY").assert_value();
    let values = StaticConnectionValues::new(BTreeMap::from([(
        field.clone(),
        "provider-secret".to_owned(),
    )]))
    .assert_value();
    let mutation = backend
        .connection_set(
            None,
            ConnectionSetRequest {
                key: key.clone(),
                scope: ConnectionScope::User,
                values,
            },
        )
        .await
        .assert_value();
    assert_eq!(mutation.connection.fields, [field]);
    assert_eq!(
        backend
            .connection_list(
                None,
                ConnectionListRequest {
                    scope: ConnectionScope::User,
                },
            )
            .await
            .assert_value()
            .connections,
        [mutation.connection]
    );
    assert!(
        backend
            .connection_delete(
                None,
                ConnectionDeleteRequest {
                    key,
                    scope: ConnectionScope::User,
                },
            )
            .await
            .assert_value()
            .deleted
    );
    assert!(
        backend
            .run_list(None, RunListParams::default())
            .await
            .assert_value()
            .runs
            .is_empty()
    );
    assert!(matches!(
        backend
            .run_discard_workspace(
                None,
                RunDiscardWorkspaceParams {
                    run_id: run_id("0199f33f-3b44-7d21-9000-000000000001"),
                },
            )
            .await,
        Err(NativeV2CliError::Local(message))
            if message.contains("user-owned")
    ));
    assert!(
        backend
            .target_add(TargetAdd {
                name: "prod".to_owned(),
                url: "https://target.example".to_owned(),
                direct: false,
            })
            .await
            .is_err()
    );
    assert!(backend.target_login("prod").await.is_err());

    exercise_named_profile_rejections(&backend).await;
    exercise_named_run_rejections(&backend).await;
    exercise_absent_local_run_failures(&backend).await;
}
