use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openengine_cluster_protocol::{
    ConnectionScope, EnvironmentVariableName, ExecutionRef, MergePlanId, RunAttachParams,
    RunDiscardWorkspaceParams, RunForceParams, RunListParams, RunLogsParams, RunProfileName,
    RunProfileScope, RunResumeParams, RunStatusParams, RunTitle, RunWatchParams, RuntimePlan,
    SourceBranchId, SourceRepositoryId, SourceRevisionId, StaticConnectionValues,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{json, Value};

use super::*;

#[path = "tests/attach.rs"]
mod attach_tests;
#[path = "tests/checkpoints.rs"]
mod checkpoint_tests;
#[path = "tests/connections.rs"]
mod connection_tests;
#[path = "tests/environment.rs"]
mod environment_tests;
#[path = "tests/lifecycle.rs"]
mod lifecycle_tests;
#[path = "tests/management.rs"]
mod management_tests;
#[path = "tests/outcome.rs"]
mod outcome_tests;
#[path = "tests/parser.rs"]
mod parser_tests;

#[path = "tests/support.rs"]
pub(in crate::native_v2_cli) mod support;

use support::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

struct EdgeDetachSignal {
    notification: tokio::sync::watch::Sender<()>,
}

#[async_trait::async_trait]
impl DetachSignal for EdgeDetachSignal {
    async fn wait(&mut self) {
        let mut receiver = self.notification.subscribe();
        let _ = receiver.changed().await;
    }
}

struct SubmitDetachSignal {
    gate: SubmitGate,
}

#[async_trait::async_trait]
impl DetachSignal for SubmitDetachSignal {
    async fn wait(&mut self) {
        self.gate.wait_until_started().await;
        self.gate.release();
    }
}

struct NotifyingOutput {
    notification: Option<tokio::sync::watch::Sender<()>>,
    bytes: Vec<u8>,
}

impl Write for NotifyingOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        if let Some(notification) = self.notification.take() {
            let _ = notification.send(());
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn notifying_detach() -> (EdgeDetachSignal, NotifyingOutput) {
    let (notification, _) = tokio::sync::watch::channel(());
    (
        EdgeDetachSignal {
            notification: notification.clone(),
        },
        NotifyingOutput {
            notification: Some(notification),
            bytes: Vec::new(),
        },
    )
}

fn assert_cursor_calls(calls: &[Call], kind: CursorCallKind, expected: &[Option<&str>]) {
    assert_eq!(calls.len(), expected.len());
    for (call, expected_cursor) in calls.iter().zip(expected) {
        let cursor_call = match (kind, call) {
            (
                CursorCallKind::Watch,
                Call::Watch {
                    target,
                    run_id,
                    from_cursor,
                },
            )
            | (
                CursorCallKind::Logs,
                Call::Logs {
                    target,
                    run_id,
                    from_cursor,
                    ..
                },
            ) => Some((target, run_id, from_cursor)),
            _ => None,
        };
        let (target, run_id, from_cursor) =
            cursor_call.assert_value_with("expected one durable observation call kind");
        assert_eq!(target.as_deref(), Some("prod"));
        assert_eq!(run_id, "run-public");
        assert_eq!(from_cursor.as_deref(), *expected_cursor);
    }
}

async fn execute_durable_command(
    command_name: &str,
    backend: &FakeBackend,
) -> (CliOutcome, String) {
    let command = parse_native_v2_args(args(&[command_name, "run-public", "--target", "prod"]))
        .assert_value();
    let mut output = Vec::new();
    let outcome = execute_native_v2_cli(command, backend, &mut NeverDetach, &mut output)
        .await
        .assert_value();
    (outcome, String::from_utf8(output).assert_value())
}

fn assert_cursor_once(output: &str, cursor: &str) {
    assert_eq!(
        output.matches(&format!("\"cursor\":\"{cursor}\"")).count(),
        1
    );
}

fn run_args(graph: &Path, input: &Path, runtime: &Path, extra: &[&str]) -> Vec<OsString> {
    let mut values = vec![
        OsString::from("run"),
        OsString::from("--target"),
        OsString::from("prod"),
        OsString::from("--repository"),
        OsString::from("open-engine/zeroshot"),
        OsString::from("--revision"),
        OsString::from("0123456789abcdef0123456789abcdef01234567"),
        OsString::from("--title"),
        OsString::from("Repair checkout"),
        OsString::from("--graph"),
        graph.as_os_str().to_owned(),
        OsString::from("--input"),
        input.as_os_str().to_owned(),
        OsString::from("--runtime-config"),
        runtime.as_os_str().to_owned(),
    ];
    if !extra.contains(&"--branch") {
        values.extend([OsString::from("--branch"), OsString::from("main")]);
    }
    values.extend(extra.iter().map(OsString::from));
    values
}

async fn execute_run_task<S, W>(
    task: &str,
    backend: &FakeBackend,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    S: DetachSignal,
    W: Write,
{
    let files = FixtureFiles::new(graph(), json!({"task":task}));
    let command = parse_native_v2_args(run_args(&files.graph, &files.input, &files.runtime, &[]))
        .assert_value();
    execute_native_v2_cli(command, backend, signal, output).await
}

fn assert_detached_submission(outcome: CliOutcome, backend: &FakeBackend) {
    assert_eq!(outcome, CliOutcome::Detached);
    assert!(matches!(backend.calls().as_slice(), [Call::Submit { .. }]));
}

async fn rejected_without_backend_contact(
    command: NativeV2CliCommand,
    backend: &FakeBackend,
) -> NativeV2CliError {
    let error = execute_native_v2_cli(command, backend, &mut NeverDetach, &mut Vec::new())
        .await
        .assert_error();
    assert!(backend.calls().is_empty());
    error
}

struct DefaultContractBackend;

#[async_trait::async_trait]
impl NativeV2CliBackend for DefaultContractBackend {
    type Watch = FakeSubscription<CliRunWatchEventNotification>;
    type Logs = FakeSubscription<RunLogEventNotification>;
    type Attach = FakeSubscription<RunAttachEventNotification>;

    async fn target_add(&self, _: TargetAdd) -> Result<(), NativeV2CliError> {
        unreachable!("default-contract test does not route target operations")
    }

    async fn target_login(&self, _: &str) -> Result<(), NativeV2CliError> {
        unreachable!("default-contract test does not route target operations")
    }

    async fn run_submit(
        &self,
        _: Option<&str>,
        _: PreparedRunRequest,
    ) -> Result<openengine_cluster_protocol::RunSubmitResult, NativeV2CliError> {
        unreachable!("default-contract test does not route run operations")
    }

    async fn run_list(
        &self,
        _: Option<&str>,
        _: RunListParams,
    ) -> Result<CliRunListResult, NativeV2CliError> {
        unreachable!("default-contract test does not route run operations")
    }

    async fn run_status(
        &self,
        _: Option<&str>,
        _: RunStatusParams,
    ) -> Result<CliRunStatusResult, NativeV2CliError> {
        unreachable!("default-contract test does not route run operations")
    }

    async fn run_watch(
        &self,
        _: Option<&str>,
        _: RunWatchParams,
    ) -> Result<Self::Watch, NativeV2CliError> {
        unreachable!("default-contract test does not route run operations")
    }

    async fn run_logs(
        &self,
        _: Option<&str>,
        _: RunLogsParams,
    ) -> Result<Self::Logs, NativeV2CliError> {
        unreachable!("default-contract test does not route run operations")
    }

    async fn run_attach(
        &self,
        _: Option<&str>,
        _: RunAttachParams,
    ) -> Result<Self::Attach, NativeV2CliError> {
        unreachable!("default-contract test does not route run operations")
    }

    async fn run_force(
        &self,
        _: Option<&str>,
        _: RunForceParams,
    ) -> Result<CliRunForceResult, NativeV2CliError> {
        unreachable!("default-contract test does not route run operations")
    }
}

#[tokio::test]
async fn wave5_cli_contract_default_backend_refuses_unadvertised_management() {
    let backend = DefaultContractBackend;
    let key = ConnectionKey::new("provider").assert_value();
    let field = EnvironmentVariableName::new("PROVIDER_TOKEN").assert_value();
    let values =
        StaticConnectionValues::new(BTreeMap::from([(field, "connection-secret".to_owned())]))
            .assert_value();
    let profile_name = RunProfileName::new("default-contract").assert_value();
    let profile = RunProfileSelector {
        name: profile_name.clone(),
        scope: RunProfileScope::User,
    };
    let graph: openengine_cluster_protocol::GraphSpec =
        serde_json::from_value(graph()).assert_value();
    let runtime = runtime();

    let errors = [
        backend
            .connection_list(
                None,
                ConnectionListRequest {
                    scope: ConnectionScope::User,
                },
            )
            .await
            .assert_error(),
        backend
            .connection_set(
                None,
                ConnectionSetRequest {
                    key: key.clone(),
                    scope: ConnectionScope::User,
                    values: values.clone(),
                },
            )
            .await
            .assert_error(),
        backend
            .connection_delete(
                None,
                ConnectionDeleteRequest {
                    key: key.clone(),
                    scope: ConnectionScope::User,
                },
            )
            .await
            .assert_error(),
        backend
            .profile_list(
                None,
                RunProfileListRequest {
                    scope: RunProfileScope::User,
                },
            )
            .await
            .assert_error(),
        backend
            .profile_show(None, profile.clone())
            .await
            .assert_error(),
        backend
            .profile_set(
                None,
                RunProfileSetRequest {
                    name: profile_name,
                    scope: RunProfileScope::User,
                    graph: graph.clone(),
                    runtime: runtime.clone(),
                    set_default: false,
                },
            )
            .await
            .assert_error(),
        backend
            .profile_delete(None, profile.clone())
            .await
            .assert_error(),
        backend
            .profile_default(
                None,
                RunProfileDefaultRequest {
                    scope: RunProfileScope::User,
                    name: Some(profile.name.clone()),
                },
            )
            .await
            .assert_error(),
    ];
    assert!(
        errors
            .iter()
            .all(|error| matches!(error, NativeV2CliError::Target(_)))
    );
}

#[tokio::test]
async fn wave5_cli_contract_default_backend_bounds_recovery_and_redacts_requests() {
    let backend = DefaultContractBackend;
    let key = ConnectionKey::new("provider").assert_value();
    let values = StaticConnectionValues::new(BTreeMap::from([(
        EnvironmentVariableName::new("PROVIDER_TOKEN").assert_value(),
        "connection-secret".to_owned(),
    )]))
    .assert_value();
    let graph: openengine_cluster_protocol::GraphSpec =
        serde_json::from_value(graph()).assert_value();
    let runtime = runtime();
    assert!(
        backend
            .merge_plan_status("target", MergePlanId::new("plan-default"))
            .await
            .is_err()
    );
    assert!(
        backend
            .merge_plan_force("target", MergePlanId::new("plan-default"))
            .await
            .is_err()
    );
    let run_id = RunId::new("run-default");
    let requirements = BTreeMap::from([(
        key.clone(),
        vec![EnvironmentVariableName::new("PROVIDER_TOKEN").assert_value()],
    )]);
    assert_eq!(
        backend
            .authorize_resume_connection_requirements(None, &run_id, requirements.clone())
            .await
            .assert_value(),
        requirements
    );
    assert!(
        backend
            .authorize_resume_connection_requirements(Some("target"), &run_id, BTreeMap::new(),)
            .await
            .is_err()
    );
    let resume = RunResumeParams {
        run_id: run_id.clone(),
        successor_run_id: RunId::new("run-successor"),
        from: None,
        connections: BTreeMap::new(),
        connection_resolver: None,
        github_token: None,
    };
    assert!(backend.run_resume(None, resume).await.is_err());
    assert!(
        backend
            .run_discard_workspace(
                None,
                RunDiscardWorkspaceParams {
                    run_id: run_id.clone()
                }
            )
            .await
            .is_err()
    );

    let request = PreparedRunRequest {
        run_id,
        intent: TargetRunIntent {
            environment: None,
            title: RunTitle::new("Redacted request").assert_value(),
            graph,
            initial_input: serde_json::Value::Null,
            runtime,
            branch: None,
            submission_key: IdempotencyKey::new("redacted-request").assert_value(),
        },
        connections: BTreeMap::from([(key, values)]),
        github_token: Some("github-secret".to_owned()),
        source: None,
        profile: None,
    };
    let debug = format!("{request:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("connection-secret"));
    assert!(!debug.contains("github-secret"));
}

struct FixtureFiles {
    directory: PathBuf,
    graph: PathBuf,
    input: PathBuf,
    runtime: PathBuf,
}

impl FixtureFiles {
    fn new(graph: Value, input: Value) -> Self {
        Self::with_runtime(graph, input, runtime())
    }

    fn with_runtime(graph: Value, input: Value, runtime: RuntimePlan) -> Self {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).assert_value();
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let directory = std::env::temp_dir().join(format!("zeroshot-v2-cli-{suffix}"));
        std::fs::create_dir(&directory).assert_value();
        let graph_path = directory.join("graph.json");
        let input_path = directory.join("input.json");
        let runtime_path = directory.join("runtime.json");
        std::fs::write(&graph_path, serde_json::to_vec(&graph).assert_value()).assert_value();
        std::fs::write(&input_path, serde_json::to_vec(&input).assert_value()).assert_value();
        std::fs::write(&runtime_path, serde_json::to_vec(&runtime).assert_value()).assert_value();
        Self {
            directory,
            graph: graph_path,
            input: input_path,
            runtime: runtime_path,
        }
    }
}

impl Drop for FixtureFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.graph);
        let _ = std::fs::remove_file(&self.input);
        let _ = std::fs::remove_file(&self.runtime);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

fn graph() -> Value {
    json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":{
            "kind":"record",
            "fields":{"task":{"type":{"kind":"string"},"required":true}}
        },
        "policy":{"policy":"policy.native-v2@1","default":"deny"},
        "root":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
    })
}

fn runtime() -> RuntimePlan {
    serde_json::from_value(json!({
        "harness":"codex",
        "provider":"openai",
        "size":"medium",
        "nodes":{}
    }))
    .assert_value()
}

fn source() -> Value {
    json!({
        "repository":"open-engine/zeroshot",
        "branch":"main",
        "revision":"0123456789abcdef0123456789abcdef01234567"
    })
}

fn status(run_id: &str, phase: &str) -> CliRunStatusResult {
    serde_json::from_value(json!({
        "runId":run_id,
        "title":"Repair checkout",
        "source":source(),
        "size":"medium",
        "atCursor":"v2:1",
        "status":{"phase":phase}
    }))
    .assert_value()
}

#[test]
fn template_list_and_show_are_static_and_emit_ordinary_json() {
    let backend = FakeBackend::default();
    let mut list_output = Vec::new();
    let list = parse_native_v2_args(args(&["template", "list"])).assert_value();
    let outcome = try_execute_native_v2_static(&list, &mut list_output)
        .assert_value()
        .assert_value();
    assert_eq!(outcome, CliOutcome::Completed);
    assert_eq!(
        serde_json::from_slice::<Value>(&list_output).assert_value(),
        json!(["single-worker", "software-change", "auto-research"])
    );

    let mut show_output = Vec::new();
    let show =
        parse_native_v2_args(args(&["template", "show", "software-change", "--pr"])).assert_value();
    try_execute_native_v2_static(&show, &mut show_output)
        .assert_value()
        .assert_value();
    let shown = serde_json::from_slice::<Value>(&show_output).assert_value();
    assert_eq!(
        shown.pointer("/profile"),
        Some(&json!("openengine.graph.full/v1"))
    );
    assert_eq!(
        shown.pointer("/initialInput/fields/task/required"),
        Some(&json!(true))
    );
    assert_eq!(
        shown.pointer("/initialInput/fields/issueNumber/required"),
        Some(&json!(false))
    );
    assert!(
        shown
            .pointer("/initialInput/fields/acceptanceFeedback")
            .is_none()
    );
    assert_eq!(
        shown.pointer("/root/state/fields/acceptanceFeedback/required"),
        Some(&json!(true))
    );
    assert_eq!(
        shown.pointer("/root/state/fields/codeFeedback/required"),
        Some(&json!(true))
    );
    assert_eq!(
        shown.pointer("/root/state/fields/deliveryFeedback/required"),
        Some(&json!(true))
    );
    for field in ["title", "description", "issueNumber"] {
        assert_eq!(
            shown.pointer(&format!("/root/state/fields/{field}/required")),
            Some(&json!(true))
        );
    }
    assert!(shown.to_string().contains("builtin.git-delivery.pr@2"));
    assert!(backend.calls().is_empty());
}

#[tokio::test]
async fn run_follows_by_default_and_forwards_per_run_intent_unchanged() {
    let files = FixtureFiles::new(graph(), json!({"task":"ship it"}));
    let command = parse_native_v2_args(run_args(
        &files.graph,
        &files.input,
        &files.runtime,
        &["--branch", "feature", "--submission-key", "stable-key"],
    ))
    .assert_value();
    let backend = FakeBackend::default();
    let mut output = Vec::new();
    let outcome = execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut output)
        .await
        .assert_value();
    assert_eq!(outcome, CliOutcome::Finished);
    assert_eq!(
        backend.calls(),
        [
            Call::Submit {
                target: Some("prod".to_owned()),
                title: RunTitle::new("Repair checkout").assert_value(),
                runtime: runtime(),
                environment: None,
                input: json!({"task":"ship it"}),
                connections: BTreeMap::new(),
                github_token: None,
                branch: Some("feature".to_owned()),
                submission_key: "stable-key".to_owned(),
            },
            Call::Watch {
                target: Some("prod".to_owned()),
                run_id: "run-public".to_owned(),
                from_cursor: None,
            },
        ]
    );
    let lines = String::from_utf8(output).assert_value();
    let mut values = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).assert_value());
    let source = values.next().assert_value();
    assert_eq!(
        source["source"],
        "open-engine/zeroshot@feature#0123456789abcdef0123456789abcdef01234567"
    );
    assert_eq!(source["target"], "prod");
    assert!(source["dirty"].is_boolean());
    assert_eq!(values.next().assert_value()["runId"], "run-public");
    assert!(lines.contains("\"phase\":\"finished\""));
}

#[tokio::test]
async fn rejected_named_submission_does_not_emit_source_metadata() {
    let files = FixtureFiles::new(graph(), json!({"task":"reject it"}));
    let command = parse_native_v2_args(run_args(
        &files.graph,
        &files.input,
        &files.runtime,
        &["--detach"],
    ))
    .assert_value();
    let backend = FakeBackend::with_failed_submit();
    let mut output = std::io::Cursor::new(Vec::new());

    let result = execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut output)
        .await
        .map_err(|error| error.to_string());

    assert_eq!(
        result,
        Err("Zeroshot OECP request failed: submission rejected".to_owned())
    );
    assert_eq!(output.position(), 0);
    assert!(matches!(backend.calls().as_slice(), [Call::Submit { .. }]));
}

#[tokio::test]
async fn detach_flag_returns_after_submit_without_opening_watch() {
    let files = FixtureFiles::new(graph(), json!({"task":"detach"}));
    let command = parse_native_v2_args(run_args(
        &files.graph,
        &files.input,
        &files.runtime,
        &["-d"],
    ))
    .assert_value();
    let backend = FakeBackend::default();
    let outcome = execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut Vec::new())
        .await
        .assert_value();
    assert_detached_submission(outcome, &backend);
}

#[tokio::test]
async fn ctrl_c_during_preparation_does_not_submit() {
    let backend = FakeBackend::default();
    let mut output = Vec::new();
    let outcome = execute_run_task("interrupt", &backend, &mut ImmediateDetach, &mut output)
        .await
        .assert_value();

    assert_eq!(outcome, CliOutcome::Detached);
    assert!(backend.calls().is_empty());
    assert!(output.is_empty());
}

#[tokio::test]
async fn ctrl_c_during_submission_waits_for_the_receipt_and_skips_watch() {
    let (backend, gate) = FakeBackend::with_blocked_submit();
    let mut signal = SubmitDetachSignal { gate };
    let mut output = Vec::new();

    let outcome = execute_run_task("interrupt submission", &backend, &mut signal, &mut output)
        .await
        .assert_value();

    assert_detached_submission(outcome, &backend);
    let lines = String::from_utf8(output).assert_value();
    assert!(lines.contains("\"source\":"));
    assert!(lines.contains("\"runId\":\"run-public\""));
}

#[tokio::test]
async fn ctrl_c_during_rejected_submission_reports_the_error_without_output() {
    let (backend, gate) = FakeBackend::with_blocked_failed_submit();
    let mut signal = SubmitDetachSignal { gate };
    let mut output = Vec::new();

    let result = execute_run_task(
        "reject interrupted submission",
        &backend,
        &mut signal,
        &mut output,
    )
    .await
    .map_err(|error| error.to_string());

    assert_eq!(
        result,
        Err("Zeroshot OECP request failed: submission rejected".to_owned())
    );
    assert!(matches!(backend.calls().as_slice(), [Call::Submit { .. }]));
    assert!(output.is_empty());
}

#[tokio::test]
async fn ctrl_c_after_submission_detaches_observation_without_force_stop() {
    let backend = FakeBackend::with_pending_watch();
    let (mut signal, mut output) = notifying_detach();

    let outcome = execute_run_task("interrupt observation", &backend, &mut signal, &mut output)
        .await
        .assert_value();

    assert_detached_submission(outcome, &backend);
    assert!(
        String::from_utf8(output.bytes)
            .assert_value()
            .contains("\"runId\":\"run-public\"")
    );
    assert!(
        !backend
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Force { .. }))
    );
}

#[tokio::test]
async fn detach_notification_survives_a_completed_subscription_branch() {
    let backend = FakeBackend::with_reconnecting_watch();
    let (mut signal, mut output) = notifying_detach();
    let command =
        parse_native_v2_args(args(&["watch", "run-public", "--target", "prod"])).assert_value();

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        execute_native_v2_cli(command, &backend, &mut signal, &mut output),
    )
    .await
    .expect("detach notification remains observable")
    .assert_value();

    assert_eq!(outcome, CliOutcome::Detached);
    assert!(
        String::from_utf8(output.bytes)
            .assert_value()
            .contains("\"cursor\":\"v2:1\"")
    );
    assert_cursor_calls(&backend.calls(), CursorCallKind::Watch, &[None]);
}

#[tokio::test]
async fn watch_reconnects_after_transport_failure_from_the_last_emitted_cursor() {
    let backend = FakeBackend::with_reconnecting_watch();
    let (outcome, output) = execute_durable_command("watch", &backend).await;
    assert_eq!(outcome, CliOutcome::Finished);
    assert_cursor_calls(
        &backend.calls(),
        CursorCallKind::Watch,
        &[None, Some("v2:1")],
    );
    assert_cursor_once(&output, "v2:1");
    assert_cursor_once(&output, "v2:2");
}

#[tokio::test]
async fn logs_reconnect_after_transport_close_without_replaying_the_boundary() {
    let backend = FakeBackend::with_reconnecting_logs();
    let (outcome, output) = execute_durable_command("logs", &backend).await;
    assert_eq!(outcome, CliOutcome::Completed);
    assert_cursor_calls(
        &backend.calls(),
        CursorCallKind::Logs,
        &[None, Some("v2:4")],
    );
    assert_cursor_once(&output, "v2:4");
    assert_cursor_once(&output, "v2:5");
}

#[tokio::test]
async fn restarted_client_reconnects_with_public_run_id_only() {
    let backend = FakeBackend::default();
    for _client_process in 0..2 {
        let command =
            parse_native_v2_args(args(&["watch", "run-public", "--target", "prod"])).assert_value();
        let outcome = execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut Vec::new())
            .await
            .assert_value();
        assert_eq!(outcome, CliOutcome::Finished);
    }
    assert_eq!(
        backend.calls(),
        [
            Call::Watch {
                target: Some("prod".to_owned()),
                run_id: "run-public".to_owned(),
                from_cursor: None,
            },
            Call::Watch {
                target: Some("prod".to_owned()),
                run_id: "run-public".to_owned(),
                from_cursor: None,
            },
        ]
    );
}

#[tokio::test]
async fn list_and_status_are_run_centric() {
    let backend = FakeBackend::default();
    for argv in [
        args(&["list"]),
        args(&["status", "run-8", "--target", "prod"]),
    ] {
        let command = parse_native_v2_args(argv).assert_value();
        execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut Vec::new())
            .await
            .assert_value();
    }
    assert_eq!(
        backend.calls(),
        [
            Call::List { target: None },
            Call::Status {
                target: Some("prod".to_owned()),
                run_id: "run-8".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn logs_attach_and_force_use_the_run_scoped_methods() {
    let backend = FakeBackend::default();
    for argv in [
        args(&[
            "logs",
            "run-1",
            "--target",
            "prod",
            "--after",
            "v2:7",
            "--execution",
            "exec-9",
        ]),
        args(&["attach", "run-1", "exec-9", "--target", "prod"]),
        args(&["force-stop", "run-1", "--target", "prod"]),
    ] {
        let command = parse_native_v2_args(argv).assert_value();
        execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut Vec::new())
            .await
            .assert_value();
    }
    assert_eq!(
        backend.calls(),
        [
            Call::Logs {
                target: Some("prod".to_owned()),
                run_id: "run-1".to_owned(),
                from_cursor: Some("v2:7".to_owned()),
                execution: Some("exec-9".to_owned()),
            },
            Call::Attach {
                target: Some("prod".to_owned()),
                run_id: "run-1".to_owned(),
                execution: "exec-9".to_owned(),
            },
            Call::Force {
                target: Some("prod".to_owned()),
                run_id: "run-1".to_owned(),
            },
        ]
    );
}
