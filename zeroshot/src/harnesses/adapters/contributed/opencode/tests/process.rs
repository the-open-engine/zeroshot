use super::*;
use crate::active_memory::hooks::{ActiveMemoryHooks, ActiveMemorySource, HookQuery};
use crate::execution::process::HostedProcessScope;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_runner::test_support::control;

pub(super) fn adapter(directory: &TestDirectory, body: &str) -> OpenCodeAdapter {
    let executable = directory.write_executable("opencode", body);
    let workspace = directory.child("workspace");
    let runtime_home = directory.child("runtime");
    std::fs::create_dir_all(&workspace).assert_value();
    std::fs::create_dir_all(&runtime_home).assert_value();
    OpenCodeAdapter::new_local(OpenCodeConfig {
        provider: "anthropic".to_owned(),
        executable: executable.to_string_lossy().into_owned(),
        prefix_arguments: vec![],
        workspace,
        runtime_home,
        local_user_home: Some(directory.path().to_owned()),
        native_environment: LocalHarnessEnvironment::default(),
        base_environment: OpenCodeProcessEnvironment::new(BTreeMap::from([(
            "PATH".to_owned(),
            "/usr/bin:/bin".to_owned(),
        )]))
        .assert_value(),
        process_pool: HostedProcessPool::hosted_default(),
    })
    .assert_value()
}

pub(super) fn invocation(fields: &[(&str, &str)]) -> DriverInvocation {
    crate::harnesses::adapters::contributed::test_support::invocation(
        "opencode",
        fields,
        Arc::new(OpenCodeSession {
            core: ProviderSessionCore::new(),
            id: Mutex::new(None),
        }),
    )
}

fn script(answer: &str) -> String {
    let text = text_event("t1", "m1", answer).to_string();
    let finish = finish_event("f1", "m1", "stop").to_string();
    format!(
        "#!/bin/sh\nif [ \"$1\" = debug ]; then printf '%s\\n' '{{}}'; exit 0; fi\ncat >/dev/null\nprintf '%s\\n' '{text}' '{finish}'\n"
    )
}

pub(super) async fn run(
    adapter: &OpenCodeAdapter,
    invocation: DriverInvocation,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let (_cancel, control, mut durable) = control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        adapter.run(invocation, control),
    )
    .await
    .assert_value();
    drain.await.assert_value();
    result
}

#[tokio::test]
async fn real_process_turn_completes_and_retains_native_session() {
    let directory = TestDirectory::new("opencode-scripted-success");
    let source =
        script("{\"response\":null}").replace("cat >/dev/null", "cat > received-prompt.txt");
    let adapter = adapter(&directory, &source);
    let mut invocation = invocation(&[]);
    invocation.host_context = Some("first host snapshot".to_owned());
    let session = invocation.session.clone();
    assert!(run(&adapter, invocation.clone()).await.is_ok());
    let first = std::fs::read_to_string(adapter.config.workspace.join("received-prompt.txt"))
        .assert_value();
    assert!(first.starts_with("Runtime-owned execution context:\nfirst host snapshot\n\n"));
    assert_eq!(
        session
            .as_any()
            .downcast_ref::<OpenCodeSession>()
            .assert_value()
            .id
            .lock()
            .await
            .as_deref(),
        Some("ses_fixture")
    );
    invocation.host_context = Some("second host snapshot".to_owned());
    assert!(run(&adapter, invocation.clone()).await.is_ok());
    let second = std::fs::read_to_string(adapter.config.workspace.join("received-prompt.txt"))
        .assert_value();
    assert!(second.starts_with("Runtime-owned execution context:\nsecond host snapshot\n\n"));
    assert!(!second.contains("first host snapshot"));
    assert_eq!(
        invocation.agent_instructions().assert_value().as_str(),
        "Exercise the adapter."
    );
}

#[tokio::test]
async fn malformed_responses_exhaust_shared_corrections() {
    let directory = TestDirectory::new("opencode-scripted-corrections");
    let adapter = adapter(&directory, &script("not a structured answer"));
    assert_eq!(
        run(&adapter, invocation(&[])).await.assert_value(),
        WorkerOutcome::malformed()
    );
}

#[tokio::test]
async fn local_declared_execution_keeps_native_policy_without_debug_or_auto() {
    let directory = TestDirectory::new("opencode-unavailable-native-policy");
    let forbidden = directory.child("forbidden-launch");
    let marker = forbidden.to_string_lossy().replace('\'', "'\\''");
    let body = format!(
        "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in debug|--auto) printf forbidden > '{marker}'; exit 99;; esac; done\n{}",
        script("{\"response\":null}")
    );
    let mut adapter = adapter(&directory, &body);
    adapter.local_environment.insert(
        "OPENCODE_CONFIG_CONTENT".to_owned(),
        json!({"permission":{"bash":"ask"}}).to_string(),
    );
    assert!(
        run(
            &adapter,
            invocation(&[("ANTHROPIC_API_KEY", "declared-key")])
        )
        .await
        .is_ok()
    );
    assert!(
        !forbidden.exists(),
        "local execution must never launch debug or automatic approval"
    );
}

#[test]
fn declared_provider_keeps_native_homes_resources_and_mcp_configuration() {
    let directory = TestDirectory::new("opencode-native-resource-discovery");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    for name in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_MODELS_PATH",
    ] {
        adapter.local_environment.insert(
            name.to_owned(),
            directory.child(name).to_string_lossy().into_owned(),
        );
    }
    adapter.local_environment.insert("OPENCODE_CONFIG_CONTENT".to_owned(), json!({"plugin":["file:///native-plugin.mjs"],
        "mcp":{"native":{"type":"remote","url":"https://mcp.invalid","headers":{"Authorization":"native-mcp-key"}}},
        "permission":{"bash":"deny"}}).to_string());
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[("ANTHROPIC_API_KEY", "declared-key")]);
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    assert_eq!(
        command.environment["HOME"],
        directory.path().to_string_lossy()
    );
    for name in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_MODELS_PATH",
    ] {
        assert_eq!(command.environment[name], adapter.local_environment[name]);
    }
    let inline: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    assert_eq!(inline["plugin"][0], "file:///native-plugin.mjs");
    assert_eq!(
        inline["mcp"]["native"]["headers"]["Authorization"],
        "native-mcp-key"
    );
    assert_eq!(inline["permission"]["bash"], "deny");
}

#[test]
fn native_pure_mode_is_kept_without_declared_authentication() {
    let directory = TestDirectory::new("opencode-native-pure-mode");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter
        .local_environment
        .insert("OPENCODE_PURE".to_owned(), "1".to_owned());
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let native = invocation(&[]);
    let native = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &native,
        files: &files,
        session_id: None,
    })
    .assert_value();
    assert_eq!(native.environment["OPENCODE_PURE"], "1");
    let declared = invocation(&[("ANTHROPIC_API_KEY", "declared-key")]);
    assert!(
        matches!(command::command(command::CommandRequest { adapter: &adapter, invocation: &declared, files: &files, session_id: None }),
        Err(NodeRunnerError::DriverDetail(message)) if message.contains("pure mode disables"))
    );
}

#[test]
fn native_inline_jsonc_preserves_comments_trailing_commas_plugins_and_authored_policy() {
    let directory = TestDirectory::new("opencode-native-inline-jsonc");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    let source = r#"{
        // Native JSONC permits comments and trailing commas.
        "plugin": ["file:///native-plugin.mjs",],
        "permission": {"bash":"ask",},
        "mcp": {"native": {"type":"remote","url":"https://mcp.invalid//path","enabled":false,
            "headers":{"Authorization":"native-mcp-key",},},},
        "provider": {"anthropic":{"models":{"opaque/model":{"options":{"message":"/* keep literal string */",},},},},},
    }"#;
    adapter
        .local_environment
        .insert("OPENCODE_CONFIG_CONTENT".to_owned(), source.to_owned());
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[("ANTHROPIC_API_KEY", "declared-key")]);
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    let inline: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    assert_eq!(inline["plugin"][0], "file:///native-plugin.mjs");
    assert_eq!(inline["permission"]["bash"], "ask");
    assert_eq!(
        inline["mcp"]["native"]["headers"]["Authorization"],
        "native-mcp-key"
    );
    assert_eq!(inline["mcp"]["native"]["url"], "https://mcp.invalid//path");
    assert_eq!(
        inline["provider"]["anthropic"]["models"]["opaque/model"]["options"]["message"],
        "/* keep literal string */"
    );
    for invalid in [
        "{permission:{}}",
        "{'permission':{}}",
        "{\"a\":1 \"b\":2}",
        "{\"a\":NaN}",
    ] {
        adapter
            .local_environment
            .insert("OPENCODE_CONFIG_CONTENT".to_owned(), invalid.to_owned());
        assert!(
            command::command(command::CommandRequest {
                adapter: &adapter,
                invocation: &invocation,
                files: &files,
                session_id: None
            })
            .is_err()
        );
    }
}

#[test]
fn native_context_captures_managed_location_and_project_discovery_controls() {
    let directory = TestDirectory::new("opencode-native-context-controls");
    let mut config = adapter(&directory, "#!/bin/sh\nexit 0\n").config;
    config.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([
        (
            "ProgramData".to_owned(),
            directory
                .child("native-program-data")
                .to_string_lossy()
                .into_owned(),
        ),
        ("OPENCODE_DISABLE_PROJECT_CONFIG".to_owned(), "1".to_owned()),
    ]));
    let adapter = OpenCodeAdapter::new_local(config).assert_value();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[]);
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    assert_eq!(
        command.environment["ProgramData"],
        directory.child("native-program-data").to_string_lossy()
    );
    assert_eq!(command.environment["OPENCODE_DISABLE_PROJECT_CONFIG"], "1");
}

#[test]
fn a_declared_key_suppresses_stored_auth_and_uses_environment_interpolation() {
    let directory = TestDirectory::new("opencode-declared-auth");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.local_environment.insert(
        "OPENCODE_AUTH_CONTENT".to_owned(),
        r#"{"anthropic":{"type":"api","key":"native-secret"}}"#.to_owned(),
    );
    adapter
        .local_environment
        .insert("OPENAI_API_KEY".to_owned(), "foreign-secret".to_owned());
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[("ANTHROPIC_API_KEY", "declared-secret")]);
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    assert_eq!(
        command
            .environment
            .get("OPENCODE_AUTH_CONTENT")
            .map(String::as_str),
        Some("{}")
    );
    assert!(!command.environment.contains_key("OPENAI_API_KEY"));
    let config: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    assert_eq!(
        config["provider"]["anthropic"]["options"]["apiKey"],
        "{env:ANTHROPIC_API_KEY}"
    );
    assert!(!config.to_string().contains("declared-secret"));
    assert_eq!(command.argv[4], "anthropic/opaque/model");
}

#[test]
fn a_declared_endpoint_never_inherits_native_credentials() {
    let directory = TestDirectory::new("opencode-declared-endpoint");
    let adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[("ANTHROPIC_BASE_URL", "https://example.invalid/messages")]);
    assert!(
        command::command(command::CommandRequest {
            adapter: &adapter,
            invocation: &invocation,
            files: &files,
            session_id: None
        })
        .is_err()
    );
}

#[test]
fn a_declared_key_does_not_promote_ambient_endpoint_over_native_configuration() {
    let directory = TestDirectory::new("opencode-native-endpoint-precedence");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.local_environment.insert(
        "ANTHROPIC_BASE_URL".to_owned(),
        "https://ambient-endpoint.invalid".to_owned(),
    );
    adapter.local_environment.insert(
        "OPENCODE_CONFIG_CONTENT".to_owned(),
        json!({"provider":{"anthropic":{"options":{"baseURL":"https://native-endpoint.invalid"}}}})
            .to_string(),
    );
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[("ANTHROPIC_API_KEY", "declared-key")]);
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    let inline: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    assert_eq!(
        inline["provider"]["anthropic"]["options"]["baseURL"],
        "https://native-endpoint.invalid"
    );
    assert_eq!(
        command.environment["ANTHROPIC_BASE_URL"],
        "https://ambient-endpoint.invalid"
    );
}

#[test]
fn contained_configuration_discards_native_homes_and_authentication() {
    let directory = TestDirectory::new("opencode-contained-native-state");
    let mut configuration = adapter(&directory, "#!/bin/sh\nexit 0\n").config;
    configuration.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([
        ("ANTHROPIC_API_KEY".to_owned(), "ambient-secret".to_owned()),
        (
            "OPENCODE_AUTH_CONTENT".to_owned(),
            r#"{"anthropic":{"type":"api","key":"stored-secret"}}"#.to_owned(),
        ),
    ]));
    let contained = OpenCodeAdapter::new(configuration).assert_value();
    assert!(contained.config.local_user_home.is_none());
    assert!(contained.local_environment.is_empty());
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let request = |invocation| {
        command::command(command::CommandRequest {
            adapter: &contained,
            invocation,
            files: &files,
            session_id: None,
        })
    };
    let no_credentials = invocation(&[]);
    assert!(
        request(&no_credentials).is_err(),
        "contained execution must not reuse native login"
    );
    let invocation = invocation(&[("ANTHROPIC_API_KEY", "declared-secret")]);
    let mut command = request(&invocation).assert_value();
    contained.apply_permission_default(&mut command);
    assert!(command.argv.iter().any(|argument| argument == "--auto"));
    assert_eq!(command.environment["ANTHROPIC_API_KEY"], "declared-secret");
    assert_eq!(command.environment["OPENCODE_AUTH_CONTENT"], "{}");
    assert_eq!(command.environment["HOME"], files.home().to_string_lossy());
}

struct RecallMemory;

impl ActiveMemorySource for RecallMemory {
    fn tool_context(&self, query: &HookQuery) -> String {
        if query.tool_name == "read" {
            assert_eq!(query.tool_name, "read");
            assert_eq!(query.tool_input["filePath"], "README.md");
        } else {
            assert_eq!(query.tool_name, "nativeproof_record");
            assert_eq!(query.tool_input["marker"], "mcp-fixture");
        }
        " recalled-marker".to_owned()
    }
    fn graph_context(&self, _: &str, _: &str) -> String {
        String::new()
    }
}

fn memory_callback_receipts(
    files: &ProviderExecutionFiles,
    context: String,
    tools: &'static [&'static str],
) -> std::thread::JoinHandle<Vec<Option<u8>>> {
    use std::io::{ErrorKind, Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).assert_value();
    listener.set_nonblocking(true).assert_value();
    let capability_path = files.home().join("active-memory-capability.json");
    let mut capability: Value =
        serde_json::from_slice(&std::fs::read(&capability_path).assert_value()).assert_value();
    capability["endpoint"] = json!(listener.local_addr().assert_value().to_string());
    std::fs::write(
        &capability_path,
        serde_json::to_vec(&capability).assert_value(),
    )
    .assert_value();
    let workspace = files.workspace.clone();
    std::thread::spawn(move || {
        let mut receipts = Vec::new();
        for &tool in tools {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "plugin did not request context");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("callback accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .assert_value();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .assert_value();
            let mut header = [0; 4];
            stream.read_exact(&mut header).assert_value();
            let length = u32::from_be_bytes(header) as usize;
            assert!(length <= 256 * 1024);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).assert_value();
            let request: Value = serde_json::from_slice(&body).assert_value();
            assert_eq!(request["token"], capability["token"]);
            assert_eq!(request["event"]["toolName"], tool);
            assert_eq!(request["event"]["cwd"], json!(workspace));
            assert!(matches!(
                request["event"]["sessionId"].as_str(),
                Some("ses_fixture" | "ses_refused")
            ));
            assert_eq!(
                request["event"]["input"],
                if tool == "read" {
                    json!({"filePath":"README.md"})
                } else {
                    json!({"marker":"mcp-fixture"})
                }
            );
            let context = context.as_bytes();
            let mut frame = (context.len() as u32).to_be_bytes().to_vec();
            frame.extend_from_slice(context);
            stream.write_all(&frame).assert_value();
            receipts.push(std::thread::spawn(move || {
                let mut receipt = [0];
                match stream.read_exact(&mut receipt) {
                    Ok(()) => Some(receipt[0]),
                    Err(error) if error.kind() == ErrorKind::UnexpectedEof => None,
                    Err(error) => panic!("callback receipt failed: {error}"),
                }
            }));
        }
        receipts
            .into_iter()
            .map(|receipt| receipt.join().assert_value())
            .collect()
    })
}

#[test]
fn native_plugin_stores_recall_as_a_message_and_leaves_tool_results_unchanged() {
    exercise_native_plugin_messages(" recalled-marker", "");
}

#[test]
fn steer_opencode_carrier_stores_maximum_text_whole_as_a_message_before_the_recall() {
    let steer = format!(
        "<steer>\n[id] {}\n</steer>",
        "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES)
    );
    exercise_native_plugin_messages(" recalled-marker", &steer);
}

#[test]
fn a_session_write_that_never_settles_does_not_hold_the_tool_call() {
    let plugin = NativePlugin::new("opencode-native-memory-unsettled");
    let reply = json!({"context":" recalled-marker","steer":"<steer>\n[id] go\n</steer>"});
    let receipts = memory_callback_receipts(&plugin.files, reply.to_string(), &["read"]);
    let source_code = r#"const {default:create}=await import(process.argv[1]);
let writes=0;
const client={session:{prompt:()=>{ writes+=1; return new Promise(()=>{}); }}};
const plugin=await create({directory:process.argv[2],client});
await plugin['tool.execute.after']({tool:'read',sessionID:'ses_fixture',callID:'call1',args:{filePath:'README.md'}},{output:'original'});
console.log(JSON.stringify({writes}));"#;
    let output = node_command()
        .args(["--input-type=module", "-e", source_code, &plugin.url])
        .arg(&plugin.files.workspace)
        .output()
        .assert_value();
    let receipts = receipts.join().assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let returned: Value = serde_json::from_slice(&output.stdout).assert_value();
    // The steer's write did not settle: the hook returned without it and wrote no lesson after it.
    assert_eq!(returned["writes"], 1);
    assert_eq!(
        receipts,
        vec![None],
        "a message that was not stored is not acknowledged"
    );
}

/// The memory plugin as a real command installs it, with the files its capability lives in.
struct NativePlugin {
    _directory: TestDirectory,
    // The adapter's hooks own the broker the capability was issued by.
    _adapter: OpenCodeAdapter,
    files: ProviderExecutionFiles,
    url: String,
}

impl NativePlugin {
    fn new(name: &str) -> Self {
        let hooks = Arc::new(ActiveMemoryHooks::new(Arc::new(RecallMemory)).assert_value());
        let directory = TestDirectory::new(name);
        let adapter = adapter(&directory, "#!/bin/sh\nexit 0\n").with_active_memory(Some(hooks));
        let files = ProviderExecutionFiles::for_test(
            directory.path(),
            HostedProcessScope::WriterExecution(1),
        )
        .assert_value();
        let mut invocation = invocation(&[]);
        crate::harnesses::native_config::tests::set(
            &mut invocation.node.binding,
            json!({"options":{"permission":{"edit":"deny"}}}),
        );
        let command = command::command(command::CommandRequest {
            adapter: &adapter,
            invocation: &invocation,
            files: &files,
            session_id: None,
        })
        .assert_value();
        let config: Value =
            serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
        assert_eq!(config["permission"]["edit"], "deny");
        let url = config["plugin"]
            .as_array()
            .assert_value()
            .last()
            .assert_value()
            .as_str()
            .assert_value()
            .to_owned();
        Self {
            _directory: directory,
            _adapter: adapter,
            files,
            url,
        }
    }
}

/// The plugin against a broker that hands `context` and `steer` to each of three tool calls, and
/// a session client that refuses to store anything for the third.
fn exercise_native_plugin_messages(context: &str, steer: &str) {
    let NativePlugin {
        _directory,
        _adapter: adapter,
        files,
        url: plugin_url,
    } = NativePlugin::new("opencode-native-memory-plugin");
    // Check the carrier's bytes directly. The production broker's 250 ms receipt deadline may
    // correctly mark an otherwise returned result unavailable when a loaded CI host delays it.
    let reply = json!({"context":context,"steer":steer}).to_string();
    let receipts = memory_callback_receipts(
        &files,
        reply,
        &["read", "nativeproof_record", "nativeproof_record"],
    );
    // Like OpenCode's client, `session.prompt` resolves to data or to an error; it does not throw.
    let source_code = r#"const {default:create}=await import(process.argv[1]);
const stored=[];
const client={session:{prompt:async (request)=>{
  if (request.path.id==='ses_refused') return {error:{name:'NotFoundError'}};
  stored.push(request); return {data:{info:{id:'msg_'+stored.length}}};
}}};
const plugin=await create({directory:process.argv[2],client});
const output={title:'read',output:'original',metadata:{}};
const builtin=structuredClone(output);
await plugin['tool.execute.after']({tool:'read',sessionID:'ses_fixture',callID:'call1',args:{filePath:'README.md'}},output);
const mcp={content:[
  {type:'text',text:'original MCP text',annotations:{priority:0.5}},
  {type:'image',mimeType:'image/png',data:'AAAA'},
  {type:'resource',resource:{uri:'file:///native.pdf',mimeType:'application/pdf',blob:'AA=='}},
  {type:'resource_link',uri:'file:///native.txt',name:'native attachment'}
],output:'MCP extension output',structuredContent:{result:{count:2}},metadata:{native:true},_meta:{native:'unchanged'},isError:false};
const before=structuredClone(mcp);
await plugin['tool.execute.after']({tool:'nativeproof_record',sessionID:'ses_fixture',callID:'call2',args:{marker:'mcp-fixture'}},mcp);
await plugin['tool.execute.after']({tool:'nativeproof_record',sessionID:'ses_refused',callID:'call3',args:{marker:'mcp-fixture'}},null);
console.log(JSON.stringify({output,builtin,mcp,before,stored}));"#;
    let output = node_command()
        .args(["--input-type=module", "-e", source_code, &plugin_url])
        .arg(&files.workspace)
        .output()
        .assert_value();
    let receipts = receipts.join().assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let returned: Value = serde_json::from_slice(&output.stdout).assert_value();
    // Nothing is placed inside a tool result, which a model may refuse to follow.
    assert_eq!(returned["output"], returned["builtin"]);
    assert_eq!(returned["mcp"], returned["before"]);
    // Each text is a message of its own, stored without starting a model turn, the steer first.
    let message = |text: &str| json!({"path":{"id":"ses_fixture"},"body":{"noReply":true,"parts":[{"type":"text","text":text}]}});
    let expected: Vec<Value> = [steer, context]
        .into_iter()
        .filter(|text| !text.is_empty())
        .map(message)
        .collect();
    let stored = returned["stored"].as_array().assert_value();
    assert_eq!(stored.len(), 2 * expected.len());
    assert_eq!(&stored[..expected.len()], expected.as_slice());
    assert_eq!(&stored[expected.len()..], expected.as_slice());
    drop(adapter);
    assert_eq!(
        receipts,
        vec![Some(1), Some(1), None],
        "stored messages are acknowledged; a session that stores nothing acknowledges nothing"
    );
}

#[tokio::test]
async fn native_args_and_config_are_isolated_and_survive_reused_sessions() {
    let directory = TestDirectory::new("opencode-native-config");
    let source=script("{\"response\":null}").replace("cat >/dev/null","printf '%s\\n' \"$@\" >> native-args\nprintf '%s\\n' \"$OPENCODE_CONFIG_CONTENT\" >> native-options\ncat >/dev/null");
    let adapter = adapter(&directory, &source);
    let mut configured = invocation(&[]);
    crate::harnesses::native_config::tests::set(
        &mut configured.node.binding,
        json!({"args":["--title","native title"],"options":{"permission":{"edit":"deny"},"futureNativeKey":true}}),
    );
    run(&adapter, configured.clone()).await.assert_value();
    run(&adapter, configured).await.assert_value();
    run(&adapter, invocation(&[])).await.assert_value();
    let args = std::fs::read_to_string(adapter.config.workspace.join("native-args")).unwrap();
    assert_eq!(args.matches("native title").count(), 2);
    let configs = std::fs::read_to_string(adapter.config.workspace.join("native-options"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(configs.len(), 3);
    assert_eq!(configs[0]["permission"]["edit"], "deny");
    assert_eq!(configs[1]["permission"]["edit"], "deny");
    assert!(configs[2].get("permission").is_none());
}

#[tokio::test]
async fn native_configuration_rejection_is_terminal_with_native_stderr() {
    let directory = TestDirectory::new("opencode-native-rejection");
    let adapter = adapter(
        &directory,
        "#!/bin/sh\nprintf 'launch\\n' >> launches\nprintf '%s\\n' 'invalid --title native-value' >&2\nexit 2\n",
    );
    let mut invocation = invocation(&[]);
    crate::harnesses::native_config::tests::set(
        &mut invocation.node.binding,
        json!({"args":["--title","native-value"]}),
    );
    let detail = run(&adapter, invocation).await.unwrap_err().to_string();
    assert!(
        detail.contains("opencode") && detail.contains("node `") && detail.contains("--title"),
        "{detail}"
    );
    assert_eq!(
        std::fs::read_to_string(adapter.config.workspace.join("launches"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[tokio::test]
async fn per_node_pure_mode_is_rejected_before_launch_with_or_without_owned_plugins() {
    let directory = TestDirectory::new("opencode-pure-native-config");
    for memory in [false, true] {
        for fields in [&[][..], &[("ANTHROPIC_API_KEY", "declared-key")][..]] {
            let mut adapter = adapter(
                &directory,
                "#!/bin/sh\nprintf 'launch\\n' >> launches\nexit 0\n",
            );
            if memory {
                let source = Arc::new(RecallMemory);
                adapter = adapter.with_active_memory(Some(Arc::new(
                    ActiveMemoryHooks::new(source).assert_value(),
                )));
            }
            for flag in ["--pure", "--pure=true", "--pure=1", "--pure=false"] {
                let mut visit = invocation(fields);
                crate::harnesses::native_config::tests::set(
                    &mut visit.node.binding,
                    json!({"args":["--title","native title",flag]}),
                );
                let detail = run(&adapter, visit).await.unwrap_err().to_string();
                for expected in ["node `", "opencode", "harnessConfig.args", "--pure"] {
                    assert!(detail.contains(expected), "{detail}");
                }
                assert!(!detail.contains("declared-key"));
                assert!(!adapter.config.workspace.join("launches").exists());
            }
        }
    }
}

#[tokio::test]
async fn dispatch_metadata_reaches_opencode_correction_processes() {
    use crate::native_v2_candidate::test_support::{
        CAPTURE_EXECUTION_ENVIRONMENT, assert_execution_environment,
    };
    use crate::native_v2_runner::NodeDispatchMetadata;
    let directory = TestDirectory::new("opencode-dispatch-metadata");
    let source = script("invalid answer").replace(
        "cat >/dev/null",
        &format!("{CAPTURE_EXECUTION_ENVIRONMENT}\ncat >/dev/null"),
    );
    let adapter = adapter(&directory, &source);
    let mut visit = invocation(&[("ZEROSHOT_NODE_NAME", "spoofed")]);
    visit.dispatch = NodeDispatchMetadata {
        attempt: 2,
        iteration: 3,
    };
    assert_eq!(
        run(&adapter, visit.clone()).await.assert_value(),
        WorkerOutcome::malformed()
    );
    let capture =
        std::fs::read_to_string(adapter.config.workspace.join("execution-context")).assert_value();
    let entries = capture.lines().collect::<Vec<_>>();
    assert_eq!(entries.len(), 21);
    for chunk in entries.chunks_exact(7) {
        assert_execution_environment(
            &(chunk.join("\n") + "\n"),
            &visit.node.reference,
            visit.dispatch,
            "step",
        );
    }
}
