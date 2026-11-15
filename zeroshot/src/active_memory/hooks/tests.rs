use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;
use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_runner::NodeSession;

type NativeInstaller = fn(&mut ProcessSessionCommand, &Path) -> std::io::Result<()>;

fn native_installers() -> [NativeInstaller; 3] {
    use crate::harnesses::adapters::core::{claude, codex, pi};
    [
        codex::memory::install,
        claude::memory::install,
        pi::memory::install,
    ]
}

#[derive(Default)]
struct Source {
    queries: Mutex<Vec<HookQuery>>,
    unavailable: Mutex<Vec<String>>,
}

impl ActiveMemorySource for Source {
    fn tool_context(&self, query: &HookQuery) -> String {
        self.queries.lock().assert_value().push(query.clone());
        "[lesson@1] Keep the generated schema synchronized.".to_owned()
    }
    fn graph_context(&self, node: &str, _session: &str) -> String {
        format!("Check all declared outputs for {node}.")
    }
    fn unavailable(&self, reason: &str) {
        self.unavailable
            .lock()
            .assert_value()
            .push(reason.to_owned());
    }
}

struct BrokerFixture {
    source: Arc<Source>,
    // The hooks own the listener; retaining them keeps every issued capability live.
    _hooks: ActiveMemoryHooks,
    capability: super::broker::Capability,
}

impl BrokerFixture {
    fn new() -> Self {
        let source = Arc::new(Source::default());
        let hooks = ActiveMemoryHooks::new(source.clone()).assert_value();
        let capability = hooks
            .broker
            .capability(&invocation(), Path::new("/workspace"))
            .assert_value();
        Self {
            source,
            _hooks: hooks,
            capability,
        }
    }
}

struct Session;

#[async_trait]
impl NodeSession for Session {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn is_live(&self) -> bool {
        true
    }
    async fn close(&self) {}
}

pub(super) fn invocation() -> DriverInvocation {
    let request = crate::native_v2_runner::test_support::request("memory-hooks", "worker", (1, 2));
    crate::native_v2_runner::test_support::null_worker_invocation(request, Arc::new(Session))
}

fn command(path: &Path) -> ProcessSessionCommand {
    ProcessSessionCommand {
        program: "native-harness".to_owned(),
        argv: vec!["exec".to_owned(), "--model".to_owned(), "opaque".to_owned()],
        environment: BTreeMap::new(),
        workspace: WorkspaceCapability {
            current_dir: path.to_owned(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: None,
    }
}

fn callback(capability: &super::broker::Capability, token: &str, event: Value) -> String {
    let (context, mut stream) = callback_response(capability, token, event);
    if !context.is_empty() {
        stream.write_all(&[1]).assert_value();
    }
    context
}

fn callback_response(
    capability: &super::broker::Capability,
    token: &str,
    event: Value,
) -> (String, TcpStream) {
    let bytes = serde_json::to_vec(&json!({"token":token,"event":event})).assert_value();
    let mut stream = TcpStream::connect(capability.endpoint).assert_value();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .assert_value();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .assert_value();
    stream.write_all(&bytes).assert_value();
    let mut header = [0; 4];
    stream.read_exact(&mut header).assert_value();
    let mut body = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut body).assert_value();
    (String::from_utf8(body).assert_value(), stream)
}

#[test]
fn unacknowledged_carriers_make_delivery_unknown_without_stopping_the_broker() {
    let fixture = BrokerFixture::new();
    let source = &fixture.source;
    let cap = &fixture.capability;
    for (expected, receipt) in [(1, None), (2, Some(0))] {
        let (context, mut stream) = callback_response(cap, &cap.token, json!({"tool_name":"Read"}));
        assert!(!context.is_empty());
        if let Some(receipt) = receipt {
            stream.write_all(&[receipt]).assert_value();
        }
        drop(stream);
        // The next accepted callback is a barrier after the previous carrier's receipt.
        assert!(!callback(cap, &cap.token, json!({"tool_name":"Read"})).is_empty());
        assert_eq!(source.unavailable.lock().assert_value().len(), expected);
    }
}

#[test]
fn a_carrier_that_stops_responding_has_a_bounded_receipt_wait() {
    let fixture = BrokerFixture::new();
    let source = &fixture.source;
    let cap = &fixture.capability;
    let (_context, held) = callback_response(cap, &cap.token, json!({"tool_name":"Read"}));
    let started = std::time::Instant::now();
    assert!(!callback(cap, &cap.token, json!({"tool_name":"Read"})).is_empty());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(source.unavailable.lock().assert_value().len(), 1);
    drop(held);
}

#[test]
fn broker_shutdown_does_not_wait_for_a_peer_dripping_frame_bytes() {
    let hooks = ActiveMemoryHooks::new(Arc::new(Source::default())).assert_value();
    let cap = hooks
        .broker
        .capability(&invocation(), Path::new("/workspace"))
        .assert_value();
    // Start the listener before introducing the deliberately incomplete frame.
    assert!(!callback(&cap, &cap.token, json!({"tool_name":"Read"})).is_empty());
    let mut stream = TcpStream::connect(cap.endpoint).assert_value();
    stream.write_all(&[0]).assert_value();
    let writer = std::thread::spawn(move || {
        // Every byte arrives faster than the socket's 250 ms syscall timeout.
        // A syscall-only timeout would wait over 1.4 seconds for this entire frame.
        let rest = [0, 0, 16].into_iter().chain([b'x'; 16]);
        for byte in rest {
            std::thread::sleep(std::time::Duration::from_millis(75));
            if stream.write_all(&[byte]).is_err() {
                break;
            }
        }
    });
    // Allow the listener to enter its header read before asking it to shut down.
    std::thread::sleep(std::time::Duration::from_millis(40));
    let started = std::time::Instant::now();
    drop(hooks);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    writer.join().assert_value();
}

#[test]
fn broker_scopes_queries_to_issued_capabilities_and_normalizes_native_fields() {
    let source = Arc::new(Source::default());
    let hooks = ActiveMemoryHooks::new(source.clone()).assert_value();
    let invocation = invocation();
    let cap = hooks
        .broker
        .capability(&invocation, Path::new("/workspace"))
        .assert_value();
    assert!(callback(&cap, "invalid", json!({"tool_name":"Read"})).is_empty());
    assert!(source.queries.lock().assert_value().is_empty());
    assert!(
        callback(
            &cap,
            &cap.token,
            json!({
                "tool_name":"Read","tool_input":{"file_path":"schema.json"},
                "cwd":"subfolder","session_id":"native-session","node_id":"forged"
            })
        )
        .contains("generated schema")
    );
    let queries = source.queries.lock().assert_value();
    assert_eq!(queries[0].node_id, "worker");
    assert_eq!(queries[0].execution_id, 2);
    assert_eq!(queries[0].session_key, "execution/2");
    assert_eq!(queries[0].cwd, Path::new("/workspace/subfolder"));
    assert_eq!(
        queries[0].native_session_id.as_deref(),
        Some("native-session")
    );
}

#[test]
fn harness_configuration_preserves_arguments_and_uses_private_capabilities() {
    let directory = tempfile::tempdir().assert_value();
    let hooks = ActiveMemoryHooks::new(Arc::new(Source::default())).assert_value();
    let invocation = invocation();
    for install in native_installers() {
        let mut command = command(directory.path());
        hooks.configure_with(
            &mut command,
            HookInstallation {
                invocation: &invocation,
                home: directory.path(),
            },
            install,
        );
        assert!(
            command
                .argv
                .windows(2)
                .any(|pair| pair == ["--model", "opaque"])
        );
        assert!(command.environment.is_empty());
        let cap: super::broker::Capability = serde_json::from_slice(
            &std::fs::read(directory.path().join("active-memory-capability.json")).assert_value(),
        )
        .assert_value();
        assert!(
            !command
                .argv
                .iter()
                .any(|argument| argument.contains(&cap.token))
        );
        assert!(callback(&cap, &cap.token, json!({"tool_name":"Read"})).contains("lesson@1"));
    }
    let extension = std::fs::read_to_string(directory.path().join("active-memory-extension.mjs"))
        .assert_value();
    assert!(extension.contains("tool_result"));
    assert!(extension.contains("event.structuredContent"));
}

#[test]
fn graph_guidance_supplements_host_context_without_changing_authored_instructions() {
    let hooks = ActiveMemoryHooks::new(Arc::new(Source::default())).assert_value();
    let mut original = invocation();
    original.host_context = Some("Existing host facts".into());
    let supplemented = hooks.invocation(original.clone());
    assert_eq!(original.node.reference, supplemented.node.reference);
    assert_eq!(original.node.binding, supplemented.node.binding);
    assert_eq!(original.response, supplemented.response);
    assert_eq!(original.node.instructions, supplemented.node.instructions);
    assert_eq!(
        supplemented.host_context.as_deref(),
        Some("Existing host facts\n\nCheck all declared outputs for worker."),
    );
    assert!(
        supplemented
            .agent_prompt()
            .assert_value()
            .contains("Check all declared outputs for worker.")
    );
    assert!(
        !original
            .agent_prompt()
            .assert_value()
            .contains("Check all declared outputs for worker.")
    );
}

#[test]
fn supported_large_context_crosses_the_native_callback_without_truncation() {
    struct LargeSource;
    impl ActiveMemorySource for LargeSource {
        fn tool_context(&self, _query: &HookQuery) -> String {
            "x".repeat(super::native::MAX_CONTEXT_BYTES)
        }
        fn graph_context(&self, _node: &str, _session: &str) -> String {
            String::new()
        }
    }
    let hooks = ActiveMemoryHooks::new(Arc::new(LargeSource)).assert_value();
    let cap = hooks
        .broker
        .capability(&invocation(), Path::new("/workspace"))
        .assert_value();
    let context = callback(&cap, &cap.token, json!({"tool_name":"Read"}));
    assert_eq!(context.len(), super::native::MAX_CONTEXT_BYTES);
    assert!(context.len() > 32_768);
}

#[test]
fn auxiliary_roles_never_recall_even_when_a_main_node_has_the_same_name() {
    use crate::native_v2_contract::{ExecutionId, NodeInstanceId};
    use openengine_cluster_protocol::NodeName;

    let source = Arc::new(Source::default());
    let hooks = ActiveMemoryHooks::new(source.clone()).assert_value();
    let home = tempfile::tempdir().assert_value();
    let offset = crate::native_v2_runner::AUXILIARY_EXECUTION_OFFSET;
    for name in [
        "active_memory_repository",
        "active_memory_graph",
        "active_memory_graph_maintenance",
        "active_memory_repository_maintenance",
    ] {
        let mut main = invocation();
        main.node.reference.node = NodeName::new(name).assert_value();
        main.node.reference.execution = ExecutionId::new(offset).assert_value();
        main.provider_session_slot = NodeInstanceId::new(offset).assert_value();
        assert_ne!(
            hooks.invocation(main.clone()).host_context,
            main.host_context
        );
        let mut auxiliary = main;
        auxiliary.auxiliary = true;
        auxiliary.node.reference.execution = ExecutionId::new(offset + 1).assert_value();
        auxiliary.node.reference.node_instance = NodeInstanceId::new(offset + 1).assert_value();
        auxiliary.provider_session_slot = NodeInstanceId::new(offset + 1).assert_value();
        assert_eq!(
            hooks.invocation(auxiliary.clone()).host_context,
            auxiliary.host_context
        );
        assert!(
            hooks
                .tool_context(&auxiliary, &json!({"toolName":"Read"}), home.path())
                .is_empty()
        );
        for install in native_installers() {
            let mut command = command(home.path());
            let original = command.argv.clone();
            hooks.configure_with(
                &mut command,
                HookInstallation {
                    invocation: &auxiliary,
                    home: home.path(),
                },
                install,
            );
            assert_eq!(command.argv, original);
        }
    }
    assert!(!home.path().join("active-memory-capability.json").exists());
    assert!(source.queries.lock().assert_value().is_empty());
}

#[test]
fn unknown_native_events_and_oversized_context_do_not_control_tools() {
    assert!(native::bounded_context("x".repeat(native::MAX_CONTEXT_BYTES + 1)).is_empty());
    assert!(native::bounded_context("bad\0text".to_owned()).is_empty());
    let fixture = BrokerFixture::new();
    let source = &fixture.source;
    let cap = &fixture.capability;
    assert!(callback(cap, &cap.token, json!({"hook_event_name":"FutureEvent"})).is_empty());
    assert!(source.queries.lock().assert_value().is_empty());
}

fn capability_file(fixture: &BrokerFixture, directory: &Path) -> std::path::PathBuf {
    let path = directory.join("capability.json");
    std::fs::write(&path, serde_json::to_vec(&fixture.capability).unwrap()).unwrap();
    path
}

#[test]
fn helper_returns_native_context_and_acknowledges_only_after_output_is_flushed() {
    let fixture = BrokerFixture::new();
    let directory = tempfile::tempdir().unwrap();
    let path = capability_file(&fixture, directory.path());
    let event = serde_json::to_vec(&json!({"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"schema.json"}})).unwrap();
    let mut output = Vec::new();
    super::broker::run_helper_io(&path, event.as_slice(), &mut output).unwrap();
    let output: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert!(
        output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("generated schema")
    );
    callback(
        &fixture.capability,
        &fixture.capability.token,
        json!({"tool_name":"Read"}),
    );
    assert!(fixture.source.unavailable.lock().unwrap().is_empty());
}

#[test]
fn helper_failures_return_empty_context_without_running_or_controlling_a_tool() {
    let fixture = BrokerFixture::new();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("capability.json");
    for contents in [
        b"not json".to_vec(),
        vec![b'x'; 16 * 1024 + 1],
        serde_json::to_vec(&json!({"endpoint":"203.0.113.1:9", "token":"unused"})).unwrap(),
    ] {
        std::fs::write(&path, contents).unwrap();
        let mut output = Vec::new();
        super::broker::run_helper_io(&path, b"{}".as_slice(), &mut output).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap(), json!({}));
    }
    let path = capability_file(&fixture, directory.path());
    for input in [
        b"not json".to_vec(),
        vec![b'x'; 256 * 1024 + 1],
        b"{}".to_vec(),
    ] {
        let mut output = Vec::new();
        super::broker::run_helper_io(&path, input.as_slice(), &mut output).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap(), json!({}));
    }
    assert!(fixture.source.queries.lock().unwrap().is_empty());
}

struct FailingFlush;
impl Write for FailingFlush {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::ErrorKind::BrokenPipe.into())
    }
}

#[test]
fn full_authored_instructions_and_failed_hook_setup_preserve_the_primary_invocation() {
    let source = Arc::new(Source::default());
    let hooks = ActiveMemoryHooks::new(source.clone()).unwrap();
    let mut original = invocation();
    original.node.instructions = Some(
        openengine_cluster_protocol::NodeInstructions::new(
            "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES),
        )
        .unwrap(),
    );
    let supplemented = hooks.invocation(original.clone());
    assert_eq!(supplemented.node.instructions, original.node.instructions);
    assert!(
        supplemented
            .host_context
            .as_deref()
            .unwrap()
            .contains("Check all declared outputs for worker.")
    );
    assert!(source.unavailable.lock().unwrap().is_empty());
    let invalid_home = tempfile::NamedTempFile::new().unwrap();
    for install in native_installers() {
        let mut command = command(invalid_home.path().parent().unwrap());
        let arguments = command.argv.clone();
        hooks.configure_with(
            &mut command,
            HookInstallation {
                invocation: &original,
                home: invalid_home.path(),
            },
            install,
        );
        assert_eq!(command.argv, arguments);
    }
    hooks.execution_failed();
    assert_eq!(source.unavailable.lock().unwrap().len(), 4);
}

#[test]
fn a_helper_output_failure_does_not_acknowledge_undelivered_context() {
    let fixture = BrokerFixture::new();
    let directory = tempfile::tempdir().unwrap();
    let path = capability_file(&fixture, directory.path());
    let event = serde_json::to_vec(&json!({"tool_name":"Read"})).unwrap();
    assert!(super::broker::run_helper_io(&path, event.as_slice(), FailingFlush).is_err());
    callback(
        &fixture.capability,
        &fixture.capability.token,
        json!({"tool_name":"Read"}),
    );
    let failures = fixture.source.unavailable.lock().unwrap();
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("acknowledge"));
}

#[test]
fn helper_rejects_invalid_utf8_and_oversized_callback_frames() {
    for (length, body) in [
        (1_u32, vec![0xff]),
        ((native::MAX_CONTEXT_BYTES + 1) as u32, vec![]),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut header = [0; 4];
            stream.read_exact(&mut header).unwrap();
            let mut request = vec![0; u32::from_be_bytes(header) as usize];
            stream.read_exact(&mut request).unwrap();
            stream.write_all(&length.to_be_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });
        let path = directory.path().join("capability.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({"endpoint":endpoint,"token":"fixture"})).unwrap(),
        )
        .unwrap();
        let mut output = Vec::new();
        super::broker::run_helper_io(&path, b"{}".as_slice(), &mut output).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap(), json!({}));
        server.join().unwrap();
    }
}

#[test]
fn live_test_hook_executable_is_explicit_absolute_and_does_not_change_production_selection() {
    let current = std::env::current_exe().assert_value();
    assert_eq!(native::test_hook_executable(None).assert_value(), current);
    assert_eq!(
        native::test_hook_executable(Some(current.clone().into_os_string())).assert_value(),
        current
    );
    let error = native::test_hook_executable(Some("relative-cli".into())).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn primary_retries_keep_recall_and_fresh_homes_get_fresh_recall_budgets() {
    use crate::native_v2_contract::{ExecutionId, NodeInstanceId};
    let hooks = ActiveMemoryHooks::new(Arc::new(Source::default())).assert_value();
    for execution in [131074, crate::native_v2_runner::AUXILIARY_EXECUTION_OFFSET] {
        let mut primary = invocation();
        if let NodeRuntimeBinding::Agent { session_scope, .. } = &mut primary.node.binding {
            *session_scope = SessionScope::NodeInstance;
        }
        primary.node.reference.execution = ExecutionId::new(execution).assert_value();
        let supplemented = hooks.invocation(primary.clone());
        assert_eq!(supplemented.node.instructions, primary.node.instructions);
        assert!(
            supplemented
                .host_context
                .assert_value()
                .contains("Check all declared outputs for worker.")
        );
        let previous = super::provider_session_key(&primary);
        primary.provider_home_slot = NodeInstanceId::new(3).assert_value();
        assert_ne!(previous, super::provider_session_key(&primary));
    }
}

pub(super) async fn steer_record() -> (
    Arc<dyn crate::v2_run_ledger::RunLedger>,
    openengine_cluster_protocol::RunId,
) {
    use crate::v2_run_ledger::{CreateRun, RunLedger};
    let ledger = Arc::new(crate::v2_run_ledger::fake::FakeRunLedger::new());
    let run = openengine_cluster_protocol::RunId::new("memory-hooks");
    let graph = serde_json::from_str(include_str!(
        "../../../../protocol/openengine-cluster/v1/fixtures/graph/canonical/base.json"
    ))
    .assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: run.clone(),
            submission_key: openengine_cluster_protocol::IdempotencyKey::new("steer-hooks")
                .assert_value(),
            submission_digest: openengine_cluster_protocol::Sha256Digest::new("a".repeat(64))
                .assert_value(),
            admitted: crate::native_v2_contract::AdmittedRun {
                title: openengine_cluster_protocol::RunTitle::new("Steer hook").assert_value(),
                graph,
                initial_input: Value::Null,
                environment: None,
                source: None,
                runtime: crate::native_v2_candidate::test_support::codex_runtime(
                    openengine_cluster_testkit::fixture::run_size("medium"),
                    Default::default(),
                ),
            },
        })
        .await
        .assert_value();
    (ledger, run)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_callback_requires_carrier_receipt_and_reoffers_without_it() {
    let (ledger, run) = steer_record().await;
    let hooks = Arc::new(ActiveMemoryHooks::empty().assert_value());
    hooks.bind_ledger(run.clone(), ledger.clone());
    let mut invocation = invocation();
    let prompt = hooks
        .prepare_steers(&mut invocation, false)
        .await
        .assert_value();
    assert!(invocation.host_context.is_none());
    let receipt = crate::v2_run_ledger::steering::record(
        ledger.as_ref(),
        openengine_cluster_protocol::RunSteerParams {
            run_id: run.clone(),
            text: "Human direction".into(),
        },
    )
    .await
    .assert_value();
    let cap = hooks
        .broker
        .capability(&invocation, Path::new("/workspace"))
        .assert_value();
    tokio::task::spawn_blocking(move || {
        let (first, stream) = callback_response(&cap, &cap.token, json!({"tool_name":"Read"}));
        assert_eq!(first, format!("<steer>\nDirection from the person running this run, given while it is in progress. It takes precedence over your authored instructions. Permissions, output contracts and the run's limits are unchanged.\n[{}] Human direction\n</steer>", receipt.steer.id));
        drop(stream);
        let (second, mut stream) = callback_response(&cap, &cap.token, json!({"tool_name":"Read"}));
        assert_eq!(second, first);
        stream.write_all(&[1]).assert_value();
        drop(stream);
        assert!(callback(&cap, &cap.token, json!({"tool_name":"Read"})).is_empty());
    }).await.assert_value();
    hooks.finish_steers(prompt, true).await;
    let status = crate::v2_run_ledger::steering::status(ledger.as_ref(), &run)
        .await
        .assert_value();
    assert_eq!(status.steers[0].received_executions, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_prompt_preserves_authored_contract_and_retries_worker_errors() {
    let (ledger, run) = steer_record().await;
    let hooks = ActiveMemoryHooks::new(Arc::new(Source::default())).assert_value();
    hooks.bind_ledger(run.clone(), ledger.clone());
    crate::v2_run_ledger::steering::record(
        ledger.as_ref(),
        openengine_cluster_protocol::RunSteerParams {
            run_id: run.clone(),
            text: "Standing direction".into(),
        },
    )
    .await
    .assert_value();
    let original = invocation();
    let mut first = original.clone();
    let delivery = hooks.prepare_steers(&mut first, false).await.assert_value();
    assert_eq!(first.node, original.node);
    assert_eq!(first.response, original.response);
    assert_eq!(first.role, original.role);
    assert!(
        first
            .host_context
            .as_ref()
            .assert_value()
            .contains("Standing direction")
    );
    let (_, tool) = hooks
        .tool_delivery(
            &first,
            &json!({"tool_name":"Read"}),
            Path::new("/workspace"),
        )
        .await;
    assert!(!tool.context.contains("Standing direction"));
    hooks.finish_steers(delivery, false).await;
    assert_eq!(
        crate::v2_run_ledger::steering::status(ledger.as_ref(), &run)
            .await
            .assert_value()
            .steers[0]
            .received_executions,
        0
    );
    let mut retry = original.clone();
    retry.node.reference.execution = crate::native_v2_contract::ExecutionId::new(3).assert_value();
    let delivery = hooks.prepare_steers(&mut retry, false).await.assert_value();
    assert!(
        retry
            .host_context
            .as_ref()
            .assert_value()
            .contains("Standing direction")
    );
    hooks.finish_steers(delivery, true).await;
    assert_eq!(
        crate::v2_run_ledger::steering::status(ledger.as_ref(), &run)
            .await
            .assert_value()
            .steers[0]
            .received_executions,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_maximum_callback_batches_whole_entries_before_memory_in_record_order() {
    let (ledger, run) = steer_record().await;
    let hooks = Arc::new(ActiveMemoryHooks::new(Arc::new(Source::default())).assert_value());
    hooks.bind_ledger(run.clone(), ledger.clone());
    let mut invocation = invocation();
    let prompt = hooks
        .prepare_steers(&mut invocation, false)
        .await
        .assert_value();
    let mut expected = Vec::new();
    for index in 0..6 {
        let text = format!(
            "{index}{}",
            "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES - 1)
        );
        let steer = crate::v2_run_ledger::steering::record(
            ledger.as_ref(),
            openengine_cluster_protocol::RunSteerParams {
                run_id: run.clone(),
                text: text.clone(),
            },
        )
        .await
        .assert_value();
        expected.push(format!("[{}] {text}", steer.steer.id));
    }
    let cap = hooks
        .broker
        .capability(&invocation, Path::new("/workspace"))
        .assert_value();
    tokio::task::spawn_blocking(move || {
        let mut received = String::new();
        for _ in 0..3 {
            let context = callback(&cap, &cap.token, json!({"tool_name":"Read"}));
            assert!(context.len() <= super::native::MAX_CONTEXT_BYTES);
            if context.contains("<steer>") {
                assert!(
                    context.find("</steer>").assert_value()
                        < context.find("[lesson@1]").assert_value()
                );
            }
            received.push_str(&context);
        }
        let mut prior = 0;
        for entry in expected {
            assert_eq!(received.matches(&entry).count(), 1);
            let position = received.find(&entry).assert_value();
            assert!(position >= prior);
            prior = position;
        }
    })
    .await
    .assert_value();
    hooks.finish_steers(prompt, true).await;
    assert!(
        crate::v2_run_ledger::steering::status(ledger.as_ref(), &run)
            .await
            .assert_value()
            .steers
            .iter()
            .all(|s| s.received_executions == 1)
    );
}

#[test]
fn steer_empty_memory_installs_claude_codex_pi_carriers_and_helper_preserves_maximum_text() {
    let directory = tempfile::tempdir().assert_value();
    let hooks = ActiveMemoryHooks::empty().assert_value();
    for install in native_installers() {
        let mut command = command(directory.path());
        hooks.configure_with(
            &mut command,
            HookInstallation {
                invocation: &invocation(),
                home: directory.path(),
            },
            install,
        );
        assert!(
            directory
                .path()
                .join("active-memory-capability.json")
                .is_file()
        );
    }
    struct MaximumSteer;
    impl ActiveMemorySource for MaximumSteer {
        fn graph_context(&self, _: &str, _: &str) -> String {
            String::new()
        }
        fn tool_context(&self, _: &HookQuery) -> String {
            format!(
                "<steer>\n[id] {}\n</steer>",
                "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES)
            )
        }
    }
    let hooks = ActiveMemoryHooks::new(Arc::new(MaximumSteer)).assert_value();
    let cap = hooks
        .broker
        .capability(&invocation(), Path::new("/workspace"))
        .assert_value();
    let path = directory.path().join("maximum-steer.json");
    std::fs::write(&path, serde_json::to_vec(&cap).assert_value()).assert_value();
    let mut output = Vec::new();
    super::broker::run_helper_io(&path, br#"{"tool_name":"Read"}"#.as_slice(), &mut output)
        .assert_value();
    let output: Value = serde_json::from_slice(&output).assert_value();
    assert!(
        output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .assert_value()
            .contains(&"x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_failed_prompt_lookup_retries_at_next_tool_callback() {
    use crate::v2_run_ledger::{CreateRun, RunLedger};
    let (source, run) = steer_record().await;
    let stored = source.get(&run).await.assert_value().assert_value();
    let ledger = Arc::new(crate::v2_run_ledger::fake::FakeRunLedger::new());
    let hooks = ActiveMemoryHooks::empty().assert_value();
    hooks.bind_ledger(run.clone(), ledger.clone());
    let mut invocation = invocation();
    assert!(hooks.prepare_steers(&mut invocation, false).await.is_none());
    assert!(invocation.host_context.is_none());
    ledger
        .create_or_get(CreateRun {
            run_id: run.clone(),
            submission_key: stored.submission_key,
            submission_digest: stored.submission_digest,
            admitted: stored.admitted,
        })
        .await
        .assert_value();
    crate::v2_run_ledger::steering::record(
        ledger.as_ref(),
        openengine_cluster_protocol::RunSteerParams {
            run_id: run.clone(),
            text: "Retry at the next callback".into(),
        },
    )
    .await
    .assert_value();
    let (query, delivery) = hooks
        .tool_delivery(
            &invocation,
            &json!({"tool_name":"Read"}),
            Path::new("/workspace"),
        )
        .await;
    assert!(delivery.context.contains("Retry at the next callback"));
    hooks.acknowledge(&query, &delivery).await;
    assert_eq!(
        crate::v2_run_ledger::steering::status(ledger.as_ref(), &run)
            .await
            .assert_value()
            .steers[0]
            .received_executions,
        1
    );
}
