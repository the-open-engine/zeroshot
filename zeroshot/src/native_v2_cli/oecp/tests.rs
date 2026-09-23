use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use openengine_cluster_client::{
    JsonRpcTransport, PumpedSubscription, SubscriptionTransport, TransportError,
};
use openengine_cluster_protocol::{
    ConnectionKey, ConnectionScope, EnvironmentVariableName, ExecutionRef, RequestId, RunId,
    RunProfileName, RunProfileScope, StaticConnectionValues, SubscriptionId,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::*;

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

struct StubConnector {
    transport: Arc<FailingTransport>,
    hosted_status: Option<CliRunStatusResult>,
}

impl StubConnector {
    fn new(hosted_status: Option<CliRunStatusResult>) -> Self {
        Self {
            transport: Arc::new(FailingTransport {
                requests: Arc::new(Mutex::new(Vec::new())),
            }),
            hosted_status,
        }
    }
}

fn called(operation: &str) -> NativeV2CliError {
    NativeV2CliError::Target(format!("connector called {operation}"))
}

#[async_trait]
impl TargetConnector for StubConnector {
    type Transport = FailingTransport;

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
        "runId":"run-contract",
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
            .merge_plan_status("prod", MergePlanId::new("plan"))
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise merge plans")
    );
    assert!(
        backend
            .merge_plan_force("prod", MergePlanId::new("plan"))
            .await
            .assert_error()
            .to_string()
            .contains("does not advertise merge plans")
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
    let resume = openengine_cluster_protocol::RunResumeParams {
        run_id: run_id.clone(),
        successor_run_id: RunId::new("run-successor"),
        connections: BTreeMap::new(),
        connection_resolver: None,
        github_token: None,
    };
    assert!(
        backend
            .connector
            .prepare_workspace_recovery_resume("prod", &resume)
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
async fn wave7_cli_contract_oecp_subscription_framing_and_hosted_refusals_are_exact() {
    let connector = StubConnector::new(None);
    let requests = connector.transport.requests.clone();
    let backend = NamedTargetCliBackend::new(connector);
    let run_id = RunId::new("run-contract");
    let mut watch = backend
        .run_watch(
            Some("prod"),
            RunWatchParams {
                run_id: run_id.clone(),
                from_cursor: Some(Cursor::new("cloud:9")),
            },
        )
        .await
        .assert_value();
    assert!(matches!(
        watch.next().await,
        Err(NativeV2CliError::Disconnected)
    ));
    let mut logs = backend
        .run_logs(
            Some("prod"),
            RunLogsParams {
                run_id: run_id.clone(),
                from_cursor: Some(Cursor::new("v2:2")),
                execution: None,
            },
        )
        .await
        .assert_value();
    assert!(matches!(
        logs.next().await,
        Err(NativeV2CliError::Disconnected)
    ));
    let mut attach = backend
        .run_attach(
            Some("prod"),
            RunAttachParams {
                run_id: run_id.clone(),
                execution: ExecutionRef::new("worker-1").assert_value(),
            },
        )
        .await
        .assert_value();
    assert!(matches!(
        attach.next().await,
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
            .run_force(Some("prod"), RunForceParams { run_id })
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
