use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openengine_cluster_protocol::PayloadType;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;
use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_runner::{NodeResponseContract, NodeRole, NodeSession};

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

fn invocation() -> DriverInvocation {
    let request = crate::native_v2_runner::test_support::request("memory-hooks", "worker", (1, 2));
    DriverInvocation {
        host_context: None,
        provider_session_slot: request.invocation.reference.node_instance,
        node: request.invocation,
        role: NodeRole::Worker,
        response: NodeResponseContract::Worker {
            output: PayloadType::Null,
        },
        environment: request.environment,
        session: Arc::new(Session),
    }
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
    for harness in [Harness::Codex, Harness::Claude, Harness::Pi] {
        let mut command = command(directory.path());
        hooks.configure(
            harness,
            &mut command,
            HookInstallation {
                invocation: &invocation,
                home: directory.path(),
            },
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
fn codex_trust_is_scoped_to_the_owned_session_handler_and_exact_definition() {
    // Independently cross-checked against native Codex 0.163 hooks/list and the
    // 0.155 source. Existing user/project hooks retain their own trust state.
    let handler = json!({
        "type":"command",
        "command":"'/opt/zeroshot' __active-memory-hook '/runtime/capability.json'",
        "timeout":1,
        "additionalContextLimit":32768,
    });
    let configuration = native::codex_configuration(&handler).assert_value();
    assert_eq!(configuration[0], "--config");
    assert_eq!(configuration[2], "--config");
    assert!(configuration[1].starts_with("hooks.PreToolUse="));
    let source = if cfg!(windows) {
        r"C:\<session-flags>\config.toml:pre_tool_use:0:0"
    } else {
        "/<session-flags>/config.toml:pre_tool_use:0:0"
    };
    assert_eq!(
        configuration[3],
        format!(
            "hooks.state={{{}={{\"trusted_hash\"=\"sha256:7a01b70421fb3d85a31fa6255d46a1a35f1d34043464aef81d33afb463d38ee8\"}}}}",
            serde_json::to_string(source).assert_value(),
        ),
    );
    assert!(
        !configuration
            .iter()
            .any(|argument| argument.contains("bypass"))
    );
    let mut changed = handler;
    changed["command"] = json!("'/opt/zeroshot' __active-memory-hook '/runtime/other.json'");
    assert_ne!(
        configuration[3],
        native::codex_configuration(&changed).assert_value()[3]
    );
}

#[test]
fn graph_guidance_changes_only_the_private_instruction_copy() {
    let hooks = ActiveMemoryHooks::new(Arc::new(Source::default())).assert_value();
    let original = invocation();
    let supplemented = hooks.invocation(original.clone());
    assert_eq!(original.node.reference, supplemented.node.reference);
    assert_eq!(original.node.binding, supplemented.node.binding);
    assert_eq!(original.response, supplemented.response);
    let original_instructions = original.node.instructions.assert_value();
    let instructions = supplemented.node.instructions.assert_value();
    assert!(
        instructions
            .as_str()
            .starts_with(original_instructions.as_str())
    );
    assert!(
        instructions
            .as_str()
            .contains("Check all declared outputs for worker.")
    );
    assert!(!original_instructions.as_str().contains("Active Memory"));
}

#[test]
fn auxiliary_roles_never_recall_even_when_a_main_node_has_the_same_name() {
    use crate::native_v2_contract::{ExecutionId, NodeInstanceId};
    use openengine_cluster_protocol::NodeName;

    let source = Arc::new(Source::default());
    let hooks = ActiveMemoryHooks::new(source.clone()).assert_value();
    let home = tempfile::tempdir().assert_value();
    let offset = crate::native_v2_runner::AUXILIARY_PROVIDER_SLOT_OFFSET;
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
            hooks.invocation(main.clone()).node.instructions,
            main.node.instructions
        );
        let mut auxiliary = main;
        auxiliary.node.reference.execution = ExecutionId::new(offset + 1).assert_value();
        auxiliary.node.reference.node_instance = NodeInstanceId::new(offset + 1).assert_value();
        auxiliary.provider_session_slot = NodeInstanceId::new(offset + 1).assert_value();
        assert_eq!(
            hooks.invocation(auxiliary.clone()).node.instructions,
            auxiliary.node.instructions
        );
        assert!(
            hooks
                .copilot_context(&auxiliary, &json!({"toolName":"Read"}), home.path())
                .is_empty()
        );
        for harness in [Harness::Codex, Harness::Claude, Harness::Pi] {
            let mut command = command(home.path());
            let original = command.argv.clone();
            hooks.configure(
                harness,
                &mut command,
                HookInstallation {
                    invocation: &auxiliary,
                    home: home.path(),
                },
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
    assert!(source.unavailable.lock().unwrap()[0].contains("instruction bounds"));
    let invalid_home = tempfile::NamedTempFile::new().unwrap();
    for harness in [Harness::Codex, Harness::Claude, Harness::Pi] {
        let mut command = command(invalid_home.path().parent().unwrap());
        let arguments = command.argv.clone();
        hooks.configure(
            harness,
            &mut command,
            HookInstallation {
                invocation: &original,
                home: invalid_home.path(),
            },
        );
        assert_eq!(command.argv, arguments);
    }
    hooks.execution_failed();
    assert_eq!(source.unavailable.lock().unwrap().len(), 5);
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
