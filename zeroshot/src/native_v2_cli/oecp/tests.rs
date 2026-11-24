use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use openengine_cluster_client::{
    JsonRpcTransport, NdjsonTransport, PumpedSubscription, RunSubscriptionClient,
    SubscriptionTransport, TransportError,
};
use openengine_cluster_protocol::{
    ConnectionKey, ConnectionScope, EnvironmentVariableName, ExecutionRef, IdempotencyKey,
    StackSource, RequestId, RunId, RunProfileName, RunProfileScope, RunTitle, SourceBranchId,
    SourceRepositoryId, StaticConnectionValues, SubscriptionCloseReason, SubscriptionId,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;

#[path = "reconnect_tests.rs"]
mod reconnect_tests;

struct FailingTransport {
    requests: Arc<Mutex<Vec<Value>>>,
}

impl FailingTransport {
    fn refusal(&self, request: &str) -> TransportError {
        self.requests
            .lock()
            .expect("request lock")
            .push(serde_json::from_str(request).expect("valid request"));
        TransportError::Protocol("fixture refusal".to_owned())
    }

    fn cancelled(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

#[async_trait]
impl JsonRpcTransport for FailingTransport {
    async fn request(&self, request: String) -> Result<String, TransportError> {
        Err(self.refusal(&request))
    }
}

#[async_trait]
impl SubscriptionTransport for FailingTransport {
    async fn open_subscription(
        &self,
        request: String,
        _id: RequestId,
    ) -> Result<(String, Option<PumpedSubscription>), TransportError> {
        Err(self.refusal(&request))
    }

    async fn cancel_subscription(&self, _id: SubscriptionId) -> Result<(), TransportError> {
        self.cancelled()
    }

    async fn cancel_request(&self, _id: RequestId) -> Result<(), TransportError> {
        self.cancelled()
    }

    fn next_watch_request_id(&self) -> RequestId {
        RequestId::String(uuid::Uuid::now_v7().to_string())
    }
}

struct StubConnector<T = FailingTransport> {
    transport: Arc<T>,
    hosted_status: Option<CliRunStatusResult>,
    run_parts: bool,
}

impl StubConnector {
    fn new(hosted_status: Option<CliRunStatusResult>) -> Self {
        Self {
            transport: Arc::new(FailingTransport {
                requests: Arc::new(Mutex::new(Vec::new())),
            }),
            hosted_status,
            run_parts: false,
        }
    }
}

fn called(operation: &str) -> NativeV2CliError {
    NativeV2CliError::Target(format!("connector called {operation}"))
}

#[async_trait]
impl<T> TargetConnector for StubConnector<T>
where
    T: SubscriptionTransport + Send + Sync + 'static,
{
    type Transport = T;

    async fn add(&self, _request: TargetAdd) -> Result<(), NativeV2CliError> {
        Err(called("add"))
    }

    async fn login(&self, _name: &str) -> Result<(), NativeV2CliError> {
        Err(called("login"))
    }

    async fn connection_list(
        &self,
        _name: &str,
        _request: ConnectionListRequest,
    ) -> Result<ConnectionListResult, NativeV2CliError> {
        Err(called("connection_list"))
    }

    async fn connection_set(
        &self,
        _name: &str,
        _request: ConnectionSetRequest,
    ) -> Result<ConnectionMutationResult, NativeV2CliError> {
        Err(called("connection_set"))
    }

    async fn connection_delete(
        &self,
        _name: &str,
        _request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResult, NativeV2CliError> {
        Err(called("connection_delete"))
    }

    async fn profile_list(
        &self,
        _name: &str,
        _request: RunProfileListRequest,
    ) -> Result<RunProfileListResult, NativeV2CliError> {
        Err(called("profile_list"))
    }

    async fn profile_show(
        &self,
        _name: &str,
        _selector: RunProfileSelector,
    ) -> Result<RunProfile, NativeV2CliError> {
        Err(called("profile_show"))
    }

    async fn profile_set(
        &self,
        _name: &str,
        _request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, NativeV2CliError> {
        Err(called("profile_set"))
    }

    async fn profile_delete(
        &self,
        _name: &str,
        _selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, NativeV2CliError> {
        Err(called("profile_delete"))
    }

    async fn profile_default(
        &self,
        _name: &str,
        _request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, NativeV2CliError> {
        Err(called("profile_default"))
    }

    async fn submit(
        &self,
        _name: &str,
        _request: PreparedRunRequest,
    ) -> Result<RunSubmitResult, NativeV2CliError> {
        Err(called("submit"))
    }

    async fn connect(
        &self,
        _name: &str,
        _run_id: Option<RunId>,
    ) -> Result<Arc<Self::Transport>, NativeV2CliError> {
        Ok(self.transport.clone())
    }

    async fn hosted_run_list(
        &self,
        _name: &str,
        _params: RunListParams,
    ) -> Result<Option<CliRunListResult>, NativeV2CliError> {
        Ok(None)
    }

    async fn hosted_run_status(
        &self,
        _name: &str,
        _params: RunStatusParams,
    ) -> Result<Option<CliRunStatusResult>, NativeV2CliError> {
        Ok(self.hosted_status.clone())
    }

    async fn hosted_run_route(
        &self,
        name: &str,
        params: RunStatusParams,
        grouped_only: bool,
    ) -> Result<Option<HostedRunRoute>, NativeV2CliError> {
        if grouped_only && !self.run_parts {
            return Ok(None);
        }
        Ok(self
            .hosted_run_status(name, params)
            .await?
            .map(|status| HostedRunRoute {
                status,
                run_parts: self.run_parts,
            }))
    }

    async fn connect_steering(
        &self,
        name: &str,
        run_id: RunId,
    ) -> Result<Arc<Self::Transport>, NativeV2CliError> {
        self.connect(name, Some(run_id)).await
    }

    async fn hosted_run_watch(
        &self,
        _name: &str,
        _params: RunWatchParams,
    ) -> Result<Option<BoxedSubscription<CliRunWatchEventNotification>>, NativeV2CliError> {
        Ok(None)
    }

    async fn hosted_run_logs(
        &self,
        _name: &str,
        _params: RunLogsParams,
    ) -> Result<Option<BoxedSubscription<RunLogEventNotification>>, NativeV2CliError> {
        Ok(None)
    }

    async fn hosted_run_force(
        &self,
        _name: &str,
        _params: RunForceParams,
    ) -> Result<Option<CliRunForceResult>, NativeV2CliError> {
        Ok(None)
    }
}

fn selector() -> RunProfileSelector {
    RunProfileSelector {
        scope: RunProfileScope::Org,
        name: RunProfileName::new("review").assert_value(),
    }
}

fn status(phase: &str) -> CliRunStatusResult {
    serde_json::from_value(json!({
        "partId":"run-contract","currentPartId":"run-contract","runId":"run-contract",
        "title":"Contract",
        "source":{
            "repository":"owner/repository",
            "branch":"main",
            "revision":"0123456789abcdef0123456789abcdef01234567"
        },
        "size":"small",
        "atCursor":"v2:1",
        "status":{"phase":phase}
    }))
    .assert_value()
}

fn profile_set_request() -> RunProfileSetRequest {
    RunProfileSetRequest {
        name: RunProfileName::new("review").assert_value(),
        scope: RunProfileScope::Org,
        graph: crate::native_v2_cli::BuiltinGraphTemplate::SingleWorker
            .materialize(crate::native_v2_cli::TemplateDelivery::None)
            .assert_value(),
        runtime: serde_json::from_value(json!({
            "harness":"codex",
            "provider":"openai",
            "size":"small",
            "nodes":{"worker":{"kind":"agent","model":"provider-model"}}
        }))
        .assert_value(),
        set_default: false,
    }
}

fn stack_request() -> PreparedStackRequest {
    PreparedStackRequest {
        environment: None,
        submission_key: IdempotencyKey::new("oecp-stack").assert_value(),
        title: RunTitle::new("OECP stack").assert_value(),
        expires_at: "2026-09-24T00:00:00Z".to_owned(),
        delivery: openengine_cluster_protocol::StackDelivery::Merge,
        source: StackSource {
            repository: SourceRepositoryId::new("open-engine/zeroshot").assert_value(),
            branch: SourceBranchId::new("main").assert_value(),
        },
        profile: selector(),
        runs: Vec::new(),
        connections: BTreeMap::new(),
        github_token: None,
    }
}

#[tokio::test]
async fn wave7_cli_contract_oecp_management_and_default_authority_fail_closed() {
    let connector = StubConnector::new(None);
    let backend = NamedTargetCliBackend::new(connector);
    let field = EnvironmentVariableName::new("TOKEN").assert_value();
    let key = ConnectionKey::new("provider").assert_value();
    let values =
        StaticConnectionValues::new(BTreeMap::from([(field, "secret".to_owned())])).assert_value();
    let scope = ConnectionScope::Org;
    let calls = [
        backend
            .connection_list(Some("prod"), ConnectionListRequest { scope })
            .await
            .assert_error()
            .to_string(),
        backend
            .connection_set(
                Some("prod"),
                ConnectionSetRequest {
                    key: key.clone(),
                    scope,
                    values,
                },
            )
            .await
            .assert_error()
            .to_string(),
        backend
            .connection_delete(
                Some("prod"),
                ConnectionDeleteRequest {
                    key: key.clone(),
                    scope,
                },
            )
            .await
            .assert_error()
            .to_string(),
        backend
            .profile_list(
                Some("prod"),
                RunProfileListRequest {
                    scope: RunProfileScope::Org,
                },
            )
            .await
            .assert_error()
            .to_string(),
        backend
            .profile_show(Some("prod"), selector())
            .await
            .assert_error()
            .to_string(),
        backend
            .profile_set(Some("prod"), profile_set_request())
            .await
            .assert_error()
            .to_string(),
        backend
            .profile_delete(Some("prod"), selector())
            .await
            .assert_error()
            .to_string(),
        backend
            .profile_default(
                Some("prod"),
                RunProfileDefaultRequest {
                    scope: RunProfileScope::Org,
                    name: Some(selector().name),
                },
            )
            .await
            .assert_error()
            .to_string(),
    ];
    for (message, operation) in calls.iter().zip([
        "connection_list",
        "connection_set",
        "connection_delete",
        "profile_list",
        "profile_show",
        "profile_set",
        "profile_delete",
        "profile_default",
    ]) {
        assert!(message.contains(operation), "{message}");
    }
}

#[tokio::test]
async fn wave7_cli_contract_oecp_default_optional_authority_is_refused() {
    let backend = NamedTargetCliBackend::new(StubConnector::new(None));
    let scope = ConnectionScope::Org;
    assert!(matches!(
        backend
            .connection_list(None, ConnectionListRequest { scope })
            .await,
        Err(NativeV2CliError::Target(message))
            if message.contains("local controller composition")
    ));
    assert!(
        backend
            .stack_submit("prod", stack_request())
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise stacks")
    );
    assert!(
        backend
            .stack_status("prod", StackId::new("plan"))
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise stacks")
    );
    assert!(
        backend
            .stack_force("prod", StackId::new("plan"))
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise stacks")
    );

    let run_id = RunId::new("run-contract");
    assert!(
        backend
            .authorize_resume_connection_requirements(Some("prod"), &run_id, BTreeMap::new())
            .await
            .assert_error()
            .to_string()
            .contains("authorization")
    );
    assert!(
        backend
            .connector
            .connect_workspace_recovery("prod", run_id.clone())
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise workspace recovery")
    );
    let successor_run_id = RunId::new("run-successor");
    let resume = openengine_cluster_protocol::RunResumeParams {
        runtime: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,

        part_id: run_id.clone(),
        run_id: run_id.clone(),
        successor_run_id,
        from: None,
        connections: BTreeMap::new(),
        connection_resolver: None,
        github_token: None,
    };
    assert!(
        backend
            .connector
            .prepare_workspace_recovery_resume("prod", &resume)
            .await
            .assert_error()
            .to_string()
            .contains("authorization")
    );
    assert!(
        backend
            .connector
            .revoke_workspace_recovery("prod", &run_id)
            .assert_error()
            .to_string()
            .contains("authorization")
    );
}

#[tokio::test]
async fn oecp_optional_authority_defaults_are_explicit_and_fail_closed() {
    let connector = StubConnector::new(None);
    assert_eq!(
        connector
            .review_cache_namespace("prod", None)
            .await
            .assert_value(),
        None
    );
    assert_eq!(
        connector
            .default_resource_scope("prod")
            .await
            .assert_value(),
        ConnectionScope::User
    );
    let request = crate::native_v2_review::ReviewResolutionRequest {
        url: "https://github.com/open-engine/zeroshot/issues/1".to_owned(),
        branch: None,
        revision: None,
    };
    assert!(
        connector
            .resolve_review(
                "prod",
                request.clone(),
                crate::native_v2_review::ReviewResolutionAccess {
                    runtime: &profile_set_request().runtime,
                    github_token: None
                }
            )
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise task resolution")
    );
    assert!(
        connector
            .delegation(crate::native_v2_cli::DelegationCommand {
                target: "prod".to_owned(),
                action: crate::native_v2_cli::DelegationAction::List {
                    owner: openengine_cluster_protocol::DelegationOwner::User,
                    organization_id: None,
                },
            })
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise delegation management")
    );

    let backend = NamedTargetCliBackend::new(StubConnector::new(None));
    assert_eq!(
        backend
            .review_cache_namespace(Some("prod"), None)
            .await
            .assert_value(),
        None
    );
    assert_eq!(
        backend
            .default_resource_scope(Some("prod"))
            .await
            .assert_value(),
        ConnectionScope::User
    );
    assert!(
        backend
            .resolve_review(
                Some("prod"),
                request,
                crate::native_v2_review::ReviewResolutionAccess {
                    runtime: &profile_set_request().runtime,
                    github_token: None
                }
            )
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise task resolution")
    );
    assert!(
        backend
            .delegation(crate::native_v2_cli::DelegationCommand {
                target: "prod".to_owned(),
                action: crate::native_v2_cli::DelegationAction::List {
                    owner: openengine_cluster_protocol::DelegationOwner::User,
                    organization_id: None,
                },
            })
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise delegation management")
    );

    let run_id = RunId::new("default-authority-run");
    assert!(
        backend
            .run_checkpoints(
                Some("prod"),
                openengine_cluster_protocol::RunCheckpointsParams {
                    part_id: None,
                    run_id: run_id.clone(),
                    after: None,
                    limit: None,
                },
            )
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise workspace checkpoints")
    );
    assert!(
        backend
            .run_resume(
                Some("prod"),
                openengine_cluster_protocol::RunResumeParams {
                    part_id: run_id.clone(),
                    runtime: None,
                    run_id: run_id.clone(),
                    successor_run_id: RunId::new("default-authority-successor"),
                    from: None,
                    connections: BTreeMap::new(),
                    connection_resolver: None,
                    github_token: None,
                    git_credentials: None,
                    commit_identity: None,
                    commit_co_author: None,
                },
                false,
            )
            .await
            .assert_error()
            .to_string()
            .contains("local authorization for workspace recovery is unavailable")
    );
    assert!(
        backend
            .run_discard_workspace(
                Some("prod"),
                openengine_cluster_protocol::RunDiscardWorkspaceParams {
                    part_id: None,
                    run_id
                },
            )
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise workspace recovery")
    );
}

#[tokio::test]
async fn wave7_cli_contract_oecp_subscription_framing_and_hosted_refusals_are_exact() {
    let connector = StubConnector::new(None);
    let requests = connector.transport.requests.clone();
    let backend = NamedTargetCliBackend::new(connector);
    let run_id = RunId::new("run-contract");
    assert!(matches!(
        backend
            .run_watch(
                Some("prod"),
                RunWatchParams {
                    part_id: None,
                    run_id: run_id.clone(),
                    from_cursor: Some(Cursor::new("cloud:9")),
                },
            )
            .await,
        Err(NativeV2CliError::Disconnected)
    ));
    assert!(matches!(
        backend
            .run_logs(
                Some("prod"),
                RunLogsParams {
                    part_id: None,
                    run_id: run_id.clone(),
                    from_cursor: Some(Cursor::new("v2:2")),
                    execution: None,
                },
            )
            .await,
        Err(NativeV2CliError::Disconnected)
    ));
    assert!(matches!(
        backend
            .run_attach(
                Some("prod"),
                RunAttachParams {
                    part_id: None,
                    run_id: run_id.clone(),
                    execution: ExecutionRef::new("worker-1").assert_value(),
                },
            )
            .await,
        Err(NativeV2CliError::Disconnected)
    ));
    assert_eq!(
        requests
            .lock()
            .expect("request lock")
            .iter()
            .filter_map(|request| request.get("method").and_then(Value::as_str))
            .collect::<Vec<_>>(),
        ["run/watch", "run/logs", "run/attach"]
    );
}

#[tokio::test]
async fn wave7_cli_contract_oecp_hosted_operations_and_task_cursors_fail_closed() {
    let run_id = RunId::new("run-contract");
    let hosted = NamedTargetCliBackend::new(StubConnector::new(Some(status("queued"))));
    assert!(
        hosted
            .run_watch(
                Some("prod"),
                RunWatchParams {
                    part_id: None,
                    run_id: run_id.clone(),
                    from_cursor: None,
                },
            )
            .await
            .assert_error()
            .to_string()
            .contains("did not provide its run watch")
    );
    assert!(
        hosted
            .run_logs(
                Some("prod"),
                RunLogsParams {
                    part_id: None,
                    run_id: run_id.clone(),
                    from_cursor: None,
                    execution: None,
                },
            )
            .await
            .assert_error()
            .to_string()
            .contains("did not provide its retained logs")
    );
    assert!(
        hosted
            .run_force(Some("prod"), RunForceParams::new(run_id))
            .await
            .assert_error()
            .to_string()
            .contains("did not provide its force operation")
    );

    assert!(task_is_active(&status("admitted").status));
    assert!(!task_is_active(&status("queued").status));
    assert_eq!(
        task_cursor(Some(Cursor::new("v2:7")))
            .assert_value()
            .as_str(),
        "v2:7"
    );
    assert!(task_cursor(Some(Cursor::new("cloud:7"))).is_none());
    assert!(task_cursor(None).is_none());
}

#[tokio::test]
async fn hosted_capsule_actions_use_the_selected_physical_part_and_refuse_stale_steers() {
    let root = RunId::new("run-contract");
    let child = RunId::new("second-part");
    let mut active = status("admitted");
    active.current_part_id = child.clone();
    active.part_id = child.clone();

    let mut connector = StubConnector::new(Some(active));
    let requests = connector.transport.requests.clone();
    let physical = NamedTargetCliBackend::new(connector);
    physical
        .run_status(Some("prod"), RunStatusParams::new(root.clone()))
        .await
        .assert_error();
    assert_eq!(requests.lock().assert_value()[0]["method"], "run/status");

    connector = physical.connector;
    connector.run_parts = true;
    let mut grouped = NamedTargetCliBackend::new(connector);
    let steer = openengine_cluster_protocol::RunSteerParams {
        run_id: root.clone(),
        part_id: None,
        node: None,
        mode: Default::default(),
        text: "wait".into(),
    };
    grouped
        .run_steer(Some("prod"), steer.clone())
        .await
        .assert_error();
    grouped
        .run_attach(
            Some("prod"),
            RunAttachParams {
                run_id: root.clone(),
                part_id: Some(child.clone()),
                execution: ExecutionRef::new("worker").assert_value(),
            },
        )
        .await
        .assert_error();
    {
        let calls = requests.lock().assert_value();
        assert_eq!(calls[1]["method"], "run/steer");
        assert_eq!(calls[2]["method"], "run/attach");
        for call in &calls[1..] {
            assert_eq!(call["params"]["runId"], child.as_str());
            assert_eq!(call["params"]["partId"], child.as_str());
        }
    }
    grouped
        .connector
        .hosted_status
        .as_mut()
        .assert_value()
        .part_id = root.clone();
    let error = grouped
        .run_steer(
            Some("prod"),
            openengine_cluster_protocol::RunSteerParams {
                part_id: Some(root),
                ..steer
            },
        )
        .await
        .assert_error();
    assert!(
        matches!(error, NativeV2CliError::Remote { code, .. } if code == openengine_cluster_protocol::RUN_CONFLICT)
    );
    assert_eq!(requests.lock().assert_value().len(), 3);
}

async fn forward_watch_script(
    notifications: Vec<Value>,
    consumer_is_open: bool,
) -> Vec<Result<CliSubscriptionItem<CliRunWatchEventNotification>, NativeV2CliError>> {
    let (client_write, server_read) = tokio::io::duplex(16 * 1024);
    let (mut server_write, client_read) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(async move {
        let mut reader = BufReader::new(server_read);
        let mut request = String::new();
        reader.read_line(&mut request).await.assert_value();
        let request: Value = serde_json::from_str(&request).assert_value();
        assert_eq!(request.get("method"), Some(&json!("run/watch")));
        let response = json!({
            "jsonrpc":"2.0",
            "id":request.get("id").assert_value(),
            "result":{
                "subscriptionId":"watch-forward",
                "runId":"run-contract",
                "atCursor":"v2:0"
            }
        });
        server_write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .assert_value();
        for notification in notifications {
            server_write
                .write_all(format!("{notification}\n").as_bytes())
                .await
                .assert_value();
        }
        server_write.shutdown().await.assert_value();
    });

    let transport = NdjsonTransport::new(client_read, client_write);
    let (_, mut stream) = RunSubscriptionClient::new(&transport)
        .run_watch(RunWatchParams {
            part_id: None,
            run_id: RunId::new("run-contract"),
            from_cursor: None,
        })
        .await
        .assert_value();
    let (sender, mut receiver) = mpsc::channel(4);
    if consumer_is_open {
        forward(&mut stream, sender, |event| Ok(event.into())).await;
        let mut items = Vec::new();
        while let Some(item) = receiver.recv().await {
            items.push(item);
        }
        server.await.assert_value();
        items
    } else {
        drop(receiver);
        forward(&mut stream, sender, |event| Ok(event.into())).await;
        server.await.assert_value();
        Vec::new()
    }
}

#[tokio::test]
async fn oecp_forwarding_preserves_close_errors_eof_and_consumer_drop() {
    let closed = forward_watch_script(
        vec![json!({
            "jsonrpc":"2.0",
            "method":"subscription/closed",
            "params":{"subscriptionId":"watch-forward","reason":"done"}
        })],
        true,
    )
    .await;
    assert!(matches!(
        closed.as_slice(),
        [Ok(CliSubscriptionItem::Closed {
            reason: SubscriptionCloseReason::Done,
        })]
    ));

    let malformed = forward_watch_script(
        vec![json!({
            "jsonrpc":"2.0",
            "method":"unexpected",
            "params":{"subscriptionId":"watch-forward"}
        })],
        true,
    )
    .await;
    assert!(matches!(malformed.as_slice(), [Err(_)]));

    assert!(forward_watch_script(Vec::new(), true).await.is_empty());
    assert!(
        forward_watch_script(
            vec![json!({
                "jsonrpc":"2.0",
                "method":"event",
                "params":{
                    "subscriptionId":"watch-forward",
                    "partId":"run-contract","runId":"run-contract",
                    "cursor":"v2:1",
                    "title":"Forwarding contract",
                    "source":{
                        "repository":"open-engine/zeroshot",
                        "branch":"main",
                        "revision":"0123456789abcdef0123456789abcdef01234567"
                    },
                    "size":"small",
                    "status":{"phase":"admitted"}
                }
            })],
            false,
        )
        .await
        .is_empty()
    );
}

#[tokio::test]
async fn queued_hosted_run_refuses_suspend_without_contacting_the_run() {
    let connector = StubConnector::new(Some(status("queued")));
    let requests = connector.transport.requests.clone();
    let backend = NamedTargetCliBackend::new(connector);
    let error = backend
        .run_suspend(
            Some("prod"),
            RunStatusParams::new(RunId::new("run-contract")),
        )
        .await
        .assert_error();
    assert!(
        matches!(error, NativeV2CliError::Remote { ref code, .. } if code == "invalid_request")
    );
    assert!(requests.lock().assert_value().is_empty());
}
