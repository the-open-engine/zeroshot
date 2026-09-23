use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use openengine_cluster_protocol::{
    ConnectionDeleteRequest, ConnectionDeleteResult, ConnectionListRequest, ConnectionListResult,
    ConnectionMutationResult, ConnectionSetRequest, ConnectionSummary, MergePlan, MergePlanState,
    NodeName, PositiveInteger, RunAttachEventNotification, RunAttachParams, RunCheckpoint,
    RunCheckpointsParams, RunCheckpointsResult, RunConnectionRequirements, RunConnectionValues,
    RunForceParams, RunId, RunListParams, RunLogEventNotification, RunLogsParams, RunProfile,
    RunProfileDefaultRequest, RunProfileDefaultResult, RunProfileDeleteResult,
    RunProfileListRequest, RunProfileListResult, RunProfileMutationResult, RunProfileName,
    RunProfileScope, RunProfileSelector, RunProfileSetRequest, RunProfileSummary, RunResumeParams,
    RunStatusParams, RunSubmitResult, RunTitle, RunWatchParams, RuntimePlan, UnixTimestampMillis,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{json, Value};

use super::*;

#[path = "support/attach.rs"]
mod attach;
use attach::{attach_event, AttachBehavior};
#[path = "support/cursor.rs"]
mod cursor;
use cursor::{record_cursor_call, CursorCallArgs};
#[path = "support/lifecycle.rs"]
mod lifecycle;
use lifecycle::{permanent_reopen_watch, queued_watch};

#[derive(Clone, Debug, PartialEq)]
pub(in crate::native_v2_cli) enum Call {
    TargetAdd {
        name: String,
        url: String,
        direct: bool,
    },
    TargetLogin {
        name: String,
    },
    ConnectionList {
        target: Option<String>,
        request: ConnectionListRequest,
    },
    ConnectionSet {
        target: Option<String>,
        request: ConnectionSetRequest,
    },
    ConnectionDelete {
        target: Option<String>,
        request: ConnectionDeleteRequest,
    },
    ProfileList {
        target: Option<String>,
        request: RunProfileListRequest,
    },
    ProfileShow {
        target: Option<String>,
        selector: RunProfileSelector,
    },
    ProfileSet {
        target: Option<String>,
        request: RunProfileSetRequest,
    },
    ProfileDelete {
        target: Option<String>,
        selector: RunProfileSelector,
    },
    ProfileDefault {
        target: Option<String>,
        request: RunProfileDefaultRequest,
    },
    Submit {
        target: Option<String>,
        title: RunTitle,
        runtime: RuntimePlan,
        input: Value,
        connections: RunConnectionValues,
        github_token: Option<String>,
        branch: Option<String>,
        submission_key: String,
    },
    Watch {
        target: Option<String>,
        run_id: String,
        from_cursor: Option<String>,
    },
    List {
        target: Option<String>,
    },
    Status {
        target: Option<String>,
        run_id: String,
    },
    Logs {
        target: Option<String>,
        run_id: String,
        from_cursor: Option<String>,
        execution: Option<String>,
    },
    Attach {
        target: Option<String>,
        run_id: String,
        execution: String,
    },
    Force {
        target: Option<String>,
        run_id: String,
    },
    Resume {
        target: Option<String>,
        params: RunResumeParams,
    },
    Checkpoints {
        target: Option<String>,
        params: RunCheckpointsParams,
    },
}

#[derive(Clone, Default)]
pub(super) struct SubmitGate {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl SubmitGate {
    pub(super) async fn wait_until_started(&self) {
        self.started.notified().await;
    }

    pub(super) fn release(&self) {
        self.release.notify_one();
    }
}

pub(in crate::native_v2_cli) struct FakeSubscription<E> {
    items: Option<VecDeque<FakeSubscriptionStep<E>>>,
}

enum FakeSubscriptionStep<E> {
    Item(CliSubscriptionItem<E>),
    Disconnected,
    ProtocolError,
}

impl<E> FakeSubscription<E> {
    fn items(items: Vec<CliSubscriptionItem<E>>) -> Self {
        Self {
            items: Some(items.into_iter().map(FakeSubscriptionStep::Item).collect()),
        }
    }

    fn disconnect_after(items: Vec<CliSubscriptionItem<E>>) -> Self {
        let mut steps = items
            .into_iter()
            .map(FakeSubscriptionStep::Item)
            .collect::<VecDeque<_>>();
        steps.push_back(FakeSubscriptionStep::Disconnected);
        Self { items: Some(steps) }
    }

    fn protocol_error() -> Self {
        Self {
            items: Some(VecDeque::from([FakeSubscriptionStep::ProtocolError])),
        }
    }

    fn pending() -> Self {
        Self { items: None }
    }
}

#[async_trait]
impl<E> CliSubscription<E> for FakeSubscription<E>
where
    E: Send,
{
    async fn next(&mut self) -> Result<Option<CliSubscriptionItem<E>>, NativeV2CliError> {
        match &mut self.items {
            Some(items) => match items.pop_front() {
                Some(FakeSubscriptionStep::Item(item)) => Ok(Some(item)),
                Some(FakeSubscriptionStep::Disconnected) => Err(NativeV2CliError::Disconnected),
                Some(FakeSubscriptionStep::ProtocolError) => {
                    Err(NativeV2CliError::Protocol("attach rejected".to_owned()))
                }
                None => Ok(None),
            },
            None => std::future::pending().await,
        }
    }
}

#[derive(Clone, Default)]
pub(in crate::native_v2_cli) struct FakeBackend {
    calls: Arc<Mutex<Vec<Call>>>,
    failed_submit: bool,
    submit_gate: Option<SubmitGate>,
    pending_watch: bool,
    reconnect_watch: bool,
    permanent_reopen_watch: bool,
    target_transport_reopen_watch: bool,
    reconnect_logs: bool,
    attach_behavior: AttachBehavior,
    failed_watch: bool,
    queued_lifecycle: bool,
    terminal_plan_state: Option<MergePlanState>,
    resume_requirements: Option<RunConnectionRequirements>,
    trusted_resume_requirements: Option<RunConnectionRequirements>,
}

#[derive(Clone, Copy)]
pub(super) enum CursorCallKind {
    Watch,
    Logs,
}

impl FakeBackend {
    pub(super) fn with_failed_submit() -> Self {
        Self {
            failed_submit: true,
            ..Self::default()
        }
    }

    pub(super) fn with_blocked_submit() -> (Self, SubmitGate) {
        Self::with_blocked_submit_failure(false)
    }

    pub(super) fn with_blocked_failed_submit() -> (Self, SubmitGate) {
        Self::with_blocked_submit_failure(true)
    }

    fn with_blocked_submit_failure(failed_submit: bool) -> (Self, SubmitGate) {
        let gate = SubmitGate::default();
        (
            Self {
                failed_submit,
                submit_gate: Some(gate.clone()),
                ..Self::default()
            },
            gate,
        )
    }

    pub(super) fn with_pending_watch() -> Self {
        Self {
            pending_watch: true,
            ..Self::default()
        }
    }

    pub(super) fn with_reconnecting_watch() -> Self {
        Self {
            reconnect_watch: true,
            ..Self::default()
        }
    }

    pub(super) fn with_reconnecting_logs() -> Self {
        Self {
            reconnect_logs: true,
            ..Self::default()
        }
    }

    pub(super) fn with_failed_watch() -> Self {
        Self {
            failed_watch: true,
            ..Self::default()
        }
    }

    pub(super) fn with_queued_lifecycle() -> Self {
        Self {
            queued_lifecycle: true,
            ..Self::default()
        }
    }

    pub(super) fn with_terminal_plan_state(state: MergePlanState) -> Self {
        Self {
            terminal_plan_state: Some(state),
            ..Self::default()
        }
    }

    pub(super) fn with_resume_requirements(requirements: RunConnectionRequirements) -> Self {
        Self {
            resume_requirements: Some(requirements.clone()),
            trusted_resume_requirements: Some(requirements),
            ..Self::default()
        }
    }

    pub(super) fn with_untrusted_resume_requirements(
        advertised: RunConnectionRequirements,
        trusted: RunConnectionRequirements,
    ) -> Self {
        Self {
            resume_requirements: Some(advertised),
            trusted_resume_requirements: Some(trusted),
            ..Self::default()
        }
    }

    pub(super) fn with_reconnecting_attach_after_disconnect() -> Self {
        Self {
            attach_behavior: AttachBehavior::Disconnect,
            ..Self::default()
        }
    }

    pub(super) fn with_reconnecting_attach_after_eof() -> Self {
        Self {
            attach_behavior: AttachBehavior::EndOfStream,
            ..Self::default()
        }
    }

    pub(super) fn with_reconnecting_attach_after_slow_consumer() -> Self {
        Self {
            attach_behavior: AttachBehavior::SlowConsumer,
            ..Self::default()
        }
    }

    pub(super) fn with_attach_connection_failure(attempt: usize) -> Self {
        Self {
            attach_behavior: AttachBehavior::TransportError { attempt },
            ..Self::default()
        }
    }

    pub(super) fn with_failed_attach() -> Self {
        Self {
            attach_behavior: AttachBehavior::ProtocolError,
            ..Self::default()
        }
    }

    pub(in crate::native_v2_cli) fn calls(&self) -> Vec<Call> {
        self.calls.lock().assert_value().clone()
    }

    fn terminal_plan(&self) -> Result<MergePlan, NativeV2CliError> {
        self.terminal_plan_state
            .map(terminal_merge_plan)
            .ok_or_else(|| {
                NativeV2CliError::Target("target does not advertise merge plans".to_owned())
            })
    }
}

fn merge_plan_profile() -> RunProfile {
    let runtime = serde_json::from_str(
        r#"{
            "harness":"codex","provider":"openai","size":"medium","nodes":{
                "worker":{"kind":"agent","model":"gpt-5.6-sol"},
                "acceptance":{"kind":"agent","model":"gpt-5.6-sol"},
                "code":{"kind":"agent","model":"gpt-5.6-sol"},
                "review_repair":{"kind":"agent","model":"gpt-5.6-sol"},
                "delivery_repair":{"kind":"agent","model":"gpt-5.6-sol"},
                "deliver":{"kind":"git_delivery","connections":{"github":["GH_TOKEN"]}}
            }
        }"#,
    )
    .assert_value();
    RunProfile {
        id: "profile-plan".to_owned(),
        name: RunProfileName::new("software-change").assert_value(),
        scope: RunProfileScope::Org,
        graph: BuiltinGraphTemplate::SoftwareChange
            .materialize(TemplateDelivery::Merge)
            .assert_value(),
        runtime,
        is_default: false,
    }
}

fn routed_profile(name: RunProfileName, scope: RunProfileScope) -> RunProfile {
    RunProfile {
        id: format!("profile-{}", name.as_str()),
        name,
        scope,
        graph: serde_json::from_value(graph()).assert_value(),
        runtime: runtime(),
        is_default: false,
    }
}

fn connection_summary(
    key: openengine_cluster_protocol::ConnectionKey,
    scope: openengine_cluster_protocol::ConnectionScope,
    fields: Vec<openengine_cluster_protocol::EnvironmentVariableName>,
) -> ConnectionSummary {
    ConnectionSummary {
        key,
        scope,
        kind: "static".to_owned(),
        fields,
    }
}

fn terminal_merge_plan(state: MergePlanState) -> MergePlan {
    serde_json::from_value(json!({
        "planId":"plan-public",
        "title":"Release",
        "state":state,
        "repository":"open-engine/zeroshot",
        "branch":"main",
        "submittedAt":"2026-09-10T00:00:00Z",
        "expiresAt":"2026-09-11T00:00:00Z",
        "runs":[]
    }))
    .assert_value()
}

#[async_trait]
impl NativeV2CliBackend for FakeBackend {
    type Watch = FakeSubscription<CliRunWatchEventNotification>;
    type Logs = FakeSubscription<RunLogEventNotification>;
    type Attach = FakeSubscription<RunAttachEventNotification>;

    async fn target_add(&self, request: TargetAdd) -> Result<(), NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::TargetAdd {
            name: request.name,
            url: request.url,
            direct: request.direct,
        });
        Ok(())
    }

    async fn target_login(&self, name: &str) -> Result<(), NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::TargetLogin {
            name: name.to_owned(),
        });
        Ok(())
    }

    async fn connection_list(
        &self,
        target: Option<&str>,
        request: ConnectionListRequest,
    ) -> Result<ConnectionListResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::ConnectionList {
            target: target.map(str::to_owned),
            request: request.clone(),
        });
        Ok(ConnectionListResult {
            connections: vec![connection_summary(
                openengine_cluster_protocol::ConnectionKey::new("openai").assert_value(),
                request.scope,
                vec![
                    openengine_cluster_protocol::EnvironmentVariableName::new("OPENAI_API_KEY")
                        .assert_value(),
                ],
            )],
        })
    }

    async fn connection_set(
        &self,
        target: Option<&str>,
        request: ConnectionSetRequest,
    ) -> Result<ConnectionMutationResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::ConnectionSet {
            target: target.map(str::to_owned),
            request: request.clone(),
        });
        Ok(ConnectionMutationResult {
            connection: connection_summary(
                request.key,
                request.scope,
                request.values.field_names(),
            ),
        })
    }

    async fn connection_delete(
        &self,
        target: Option<&str>,
        request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResult, NativeV2CliError> {
        self.calls
            .lock()
            .assert_value()
            .push(Call::ConnectionDelete {
                target: target.map(str::to_owned),
                request,
            });
        Ok(ConnectionDeleteResult { deleted: true })
    }

    async fn profile_list(
        &self,
        target: Option<&str>,
        request: RunProfileListRequest,
    ) -> Result<RunProfileListResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::ProfileList {
            target: target.map(str::to_owned),
            request: request.clone(),
        });
        Ok(RunProfileListResult {
            profiles: [("alpha", true), ("beta", false)]
                .into_iter()
                .map(|(name, is_default)| RunProfileSummary {
                    id: format!("profile-{name}"),
                    name: RunProfileName::new(name).assert_value(),
                    scope: request.scope,
                    is_default,
                })
                .collect(),
        })
    }

    async fn profile_show(
        &self,
        target: Option<&str>,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, NativeV2CliError> {
        if self.terminal_plan_state.is_some() {
            return Ok(merge_plan_profile());
        }
        self.calls.lock().assert_value().push(Call::ProfileShow {
            target: target.map(str::to_owned),
            selector: selector.clone(),
        });
        Ok(routed_profile(selector.name, selector.scope))
    }

    async fn profile_set(
        &self,
        target: Option<&str>,
        request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::ProfileSet {
            target: target.map(str::to_owned),
            request: request.clone(),
        });
        Ok(RunProfileMutationResult {
            profile: RunProfile {
                id: format!("profile-{}", request.name.as_str()),
                name: request.name,
                scope: request.scope,
                graph: request.graph,
                runtime: request.runtime,
                is_default: request.set_default,
            },
        })
    }

    async fn profile_delete(
        &self,
        target: Option<&str>,
        selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::ProfileDelete {
            target: target.map(str::to_owned),
            selector,
        });
        Ok(RunProfileDeleteResult { deleted: true })
    }

    async fn profile_default(
        &self,
        target: Option<&str>,
        request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::ProfileDefault {
            target: target.map(str::to_owned),
            request: request.clone(),
        });
        Ok(RunProfileDefaultResult {
            scope: request.scope,
            name: request.name,
        })
    }

    async fn merge_plan_submit(
        &self,
        _target: &str,
        _request: PreparedMergePlanRequest,
    ) -> Result<MergePlan, NativeV2CliError> {
        self.terminal_plan()
    }

    async fn merge_plan_status(
        &self,
        _target: &str,
        _plan_id: MergePlanId,
    ) -> Result<MergePlan, NativeV2CliError> {
        self.terminal_plan()
    }

    async fn merge_plan_force(
        &self,
        _target: &str,
        _plan_id: MergePlanId,
    ) -> Result<MergePlan, NativeV2CliError> {
        self.terminal_plan()
    }

    async fn run_submit(
        &self,
        target: Option<&str>,
        request: PreparedRunRequest,
    ) -> Result<RunSubmitResult, NativeV2CliError> {
        let PreparedRunRequest {
            intent,
            connections,
            github_token,
            run_id: _,
            source: _,
            profile: _,
        } = request;
        self.calls.lock().assert_value().push(Call::Submit {
            target: target.map(str::to_owned),
            title: intent.title,
            runtime: intent.runtime,
            input: intent.initial_input,
            connections,
            github_token,
            branch: intent.branch.map(|branch| branch.as_str().to_owned()),
            submission_key: intent.submission_key.as_str().to_owned(),
        });
        if let Some(gate) = &self.submit_gate {
            gate.started.notify_one();
            gate.release.notified().await;
        }
        if self.failed_submit {
            return Err(NativeV2CliError::Protocol("submission rejected".to_owned()));
        }
        Ok(RunSubmitResult {
            run_id: RunId::new("run-public"),
        })
    }

    async fn run_list(
        &self,
        target: Option<&str>,
        _params: RunListParams,
    ) -> Result<CliRunListResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::List {
            target: target.map(str::to_owned),
        });
        let runs = self
            .queued_lifecycle
            .then(|| status("run-public", "queued"))
            .into_iter()
            .collect();
        Ok(CliRunListResult { runs })
    }

    async fn run_status(
        &self,
        target: Option<&str>,
        params: RunStatusParams,
    ) -> Result<CliRunStatusResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::Status {
            target: target.map(str::to_owned),
            run_id: params.run_id.as_str().to_owned(),
        });
        if let Some(requirements) = &self.resume_requirements {
            return serde_json::from_value(json!({
                "runId":params.run_id,
                "title":"Repair checkout",
                "source":source(),
                "size":"medium",
                "atCursor":"v2:3",
                "status":{
                    "phase":"finished",
                    "terminalResult":{"status":"failed","reason":"worker_failed"},
                    "metadata":{}
                },
                "workspaceRecovery":{
                    "recoverable":true,
                    "connectionRequirements":requirements
                }
            }))
            .map_err(NativeV2CliError::OutputJson);
        }
        Ok(status(
            "run-public",
            if self.queued_lifecycle {
                "queued"
            } else {
                "admitted"
            },
        ))
    }

    async fn run_watch(
        &self,
        target: Option<&str>,
        params: RunWatchParams,
    ) -> Result<Self::Watch, NativeV2CliError> {
        let attempt = record_cursor_call(
            self,
            CursorCallArgs {
                kind: CursorCallKind::Watch,
                target,
                run_id: &params.run_id,
                from_cursor: params.from_cursor.as_ref(),
                execution: None,
            },
        );
        if self.pending_watch {
            return Ok(FakeSubscription::pending());
        }
        if let Some(result) = permanent_reopen_watch(self, &params, attempt) {
            return result;
        }
        if self.target_transport_reopen_watch && attempt == 2 {
            return Err(target_transport_error());
        }
        if self.queued_lifecycle {
            return Ok(queued_watch(&params, attempt));
        }
        if self.reconnect_watch && attempt == 1 {
            return Ok(FakeSubscription::disconnect_after(vec![
                CliSubscriptionItem::Event(
                    serde_json::from_value(json!({
                        "subscriptionId":"watch-1",
                        "runId":params.run_id,
                        "title":"Repair checkout",
                        "source":source(),
                        "size":"medium",
                        "cursor":"v2:1",
                        "status":{"phase":"running","activeExecutions":[]}
                    }))
                    .assert_value(),
                ),
            ]));
        }
        let terminal_result = if self.failed_watch {
            json!({"status":"failed","reason":"worker_failed"})
        } else {
            json!({"status":"succeeded","output":null})
        };
        Ok(FakeSubscription::items(vec![CliSubscriptionItem::Event(
            serde_json::from_value(json!({
                "subscriptionId":"watch-1",
                "runId":params.run_id,
                "title":"Repair checkout",
                "source":source(),
                "size":"medium",
                "cursor":"v2:2",
                "status":{"phase":"finished","terminalResult":terminal_result,"metadata":{}}
            }))
            .assert_value(),
        )]))
    }

    async fn run_logs(
        &self,
        target: Option<&str>,
        params: RunLogsParams,
    ) -> Result<Self::Logs, NativeV2CliError> {
        let attempt = record_cursor_call(
            self,
            CursorCallArgs {
                kind: CursorCallKind::Logs,
                target,
                run_id: &params.run_id,
                from_cursor: params.from_cursor.as_ref(),
                execution: params.execution.as_ref(),
            },
        );
        if self.reconnect_logs {
            let (cursor, message) = if attempt == 1 {
                ("v2:4", "before disconnect")
            } else {
                ("v2:5", "after reconnect")
            };
            let event = CliSubscriptionItem::Event(
                serde_json::from_value(json!({
                    "subscriptionId":format!("logs-{attempt}"),
                    "runId":params.run_id,
                    "cursor":cursor,
                    "timestamp":1_725_000_000_123_u64,
                    "record":{"level":"info","target":"agent","message":message}
                }))
                .assert_value(),
            );
            if attempt == 1 {
                return Ok(FakeSubscription::items(vec![event]));
            }
            return Ok(FakeSubscription::items(vec![
                event,
                CliSubscriptionItem::Closed {
                    reason: SubscriptionCloseReason::Done,
                },
            ]));
        }
        Ok(FakeSubscription::items(vec![CliSubscriptionItem::Closed {
            reason: SubscriptionCloseReason::Done,
        }]))
    }

    async fn run_attach(
        &self,
        target: Option<&str>,
        params: RunAttachParams,
    ) -> Result<Self::Attach, NativeV2CliError> {
        let mut calls = self.calls.lock().assert_value();
        let attempt = calls
            .iter()
            .filter(|call| matches!(call, Call::Attach { .. }))
            .count()
            + 1;
        calls.push(Call::Attach {
            target: target.map(str::to_owned),
            run_id: params.run_id.as_str().to_owned(),
            execution: params.execution.as_str().to_owned(),
        });
        drop(calls);

        match (self.attach_behavior, attempt) {
            (AttachBehavior::Done, _) => {
                Ok(FakeSubscription::items(vec![CliSubscriptionItem::Closed {
                    reason: SubscriptionCloseReason::Done,
                }]))
            }
            (AttachBehavior::ProtocolError, _) => Ok(FakeSubscription::protocol_error()),
            (AttachBehavior::TransportError { attempt: failed }, current) if failed == current => {
                Err(target_transport_error())
            }
            (_, 2..) => Ok(FakeSubscription::items(vec![
                attach_event(&params, attempt, "after reconnect"),
                CliSubscriptionItem::Closed {
                    reason: SubscriptionCloseReason::Done,
                },
            ])),
            (AttachBehavior::Disconnect | AttachBehavior::TransportError { .. }, _) => {
                Ok(FakeSubscription::disconnect_after(vec![attach_event(
                    &params,
                    attempt,
                    "before reconnect",
                )]))
            }
            (AttachBehavior::EndOfStream, _) => Ok(FakeSubscription::items(vec![attach_event(
                &params,
                attempt,
                "before reconnect",
            )])),
            (AttachBehavior::SlowConsumer, _) => Ok(FakeSubscription::items(vec![
                attach_event(&params, attempt, "before reconnect"),
                CliSubscriptionItem::Closed {
                    reason: SubscriptionCloseReason::SlowConsumer,
                },
            ])),
        }
    }

    async fn run_force(
        &self,
        target: Option<&str>,
        params: RunForceParams,
    ) -> Result<CliRunForceResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::Force {
            target: target.map(str::to_owned),
            run_id: params.run_id.as_str().to_owned(),
        });
        serde_json::from_value(json!({
            "runId":params.run_id,
            "title":"Repair checkout",
            "source":source(),
            "size":"medium",
            "atCursor":"v2:3",
            "status":{"phase":"stopping","activeExecutions":[]}
        }))
        .map_err(NativeV2CliError::OutputJson)
    }

    async fn authorize_resume_connection_requirements(
        &self,
        target: Option<&str>,
        _run_id: &RunId,
        requirements: RunConnectionRequirements,
    ) -> Result<RunConnectionRequirements, NativeV2CliError> {
        if target.is_none() {
            return Ok(requirements);
        }
        let trusted = self.trusted_resume_requirements.as_ref().ok_or_else(|| {
            NativeV2CliError::Target(
                "local authorization for workspace recovery is unavailable".to_owned(),
            )
        })?;
        if trusted != &requirements {
            return Err(NativeV2CliError::Target(
                "workspace-recovery connection requirements do not match the original run"
                    .to_owned(),
            ));
        }
        Ok(trusted.clone())
    }

    async fn run_checkpoints(
        &self,
        target: Option<&str>,
        params: RunCheckpointsParams,
    ) -> Result<RunCheckpointsResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::Checkpoints {
            target: target.map(str::to_owned),
            params: params.clone(),
        });
        Ok(RunCheckpointsResult {
            run_id: params.run_id,
            checkpoints: vec![RunCheckpoint {
                checkpoint_id: CheckpointId::new("entry-8").assert_value(),
                sequence: PositiveInteger::new(8).assert_value(),
                node: NodeName::new("repair").assert_value(),
                map_indices: vec![2, 1],
                loop_iterations: vec![3],
                created_at: UnixTimestampMillis::new(42).assert_value(),
            }],
            next_after: Some(CheckpointId::new("entry-8").assert_value()),
        })
    }

    async fn run_resume(
        &self,
        target: Option<&str>,
        params: RunResumeParams,
    ) -> Result<openengine_cluster_protocol::RunResumeResult, NativeV2CliError> {
        self.calls.lock().assert_value().push(Call::Resume {
            target: target.map(str::to_owned),
            params: params.clone(),
        });
        Ok(openengine_cluster_protocol::RunResumeResult {
            run_id: params.successor_run_id,
            resumed_from: params.run_id,
        })
    }
}

pub(super) struct ImmediateDetach;

#[async_trait]
impl DetachSignal for ImmediateDetach {
    async fn wait(&mut self) {}
}

fn target_transport_error() -> NativeV2CliError {
    NativeV2CliError::TargetTransport {
        name: "prod".to_owned(),
        origin: "https://target.example".to_owned(),
        message: "target discovery failed: connection refused".to_owned(),
    }
}
