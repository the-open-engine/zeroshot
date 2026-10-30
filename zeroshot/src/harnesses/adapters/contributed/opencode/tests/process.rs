use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
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

struct RecallMemory {
    callbacks: AtomicUsize,
    failures: AtomicUsize,
}

impl ActiveMemorySource for RecallMemory {
    fn tool_context(&self, query: &HookQuery) -> String {
        if query.tool_name == "read" {
            assert_eq!(query.tool_name, "read");
            assert_eq!(query.tool_input["filePath"], "README.md");
        } else {
            assert_eq!(query.tool_name, "nativeproof_record");
            assert_eq!(query.tool_input["marker"], "mcp-fixture");
        }
        self.callbacks.fetch_add(1, Ordering::SeqCst);
        " recalled-marker".to_owned()
    }
    fn graph_context(&self, _: &str, _: &str) -> String {
        String::new()
    }
    fn unavailable(&self, _: &str) {
        self.failures.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn native_plugin_acknowledges_builtin_and_mcp_recall_without_changing_attachments() {
    let source = Arc::new(RecallMemory {
        callbacks: AtomicUsize::new(0),
        failures: AtomicUsize::new(0),
    });
    let hooks = Arc::new(ActiveMemoryHooks::new(source.clone()).assert_value());
    let directory = TestDirectory::new("opencode-native-memory-plugin");
    let adapter = adapter(&directory, "#!/bin/sh\nexit 0\n").with_active_memory(Some(hooks));
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
    let config: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    let plugin_url = config["plugin"]
        .as_array()
        .assert_value()
        .last()
        .assert_value()
        .as_str()
        .assert_value();
    let source_code = r#"const {default:create}=await import(process.argv[1]);
const plugin=await create({directory:process.argv[2]});
const output={title:'read',output:'original',metadata:{}};
await plugin['tool.execute.after']({tool:'read',sessionID:'ses_fixture',callID:'call1',args:{filePath:'README.md'}},output);
const mcp={content:[
  {type:'text',text:'original MCP text',annotations:{priority:0.5}},
  {type:'image',mimeType:'image/png',data:'AAAA'},
  {type:'resource',resource:{uri:'file:///native.pdf',mimeType:'application/pdf',blob:'AA=='}},
  {type:'resource_link',uri:'file:///native.txt',name:'native attachment'}
],output:'MCP extension output',structuredContent:{result:{count:2}},metadata:{native:true},_meta:{native:'unchanged'},isError:false};
const before=structuredClone(mcp);
await plugin['tool.execute.after']({tool:'nativeproof_record',sessionID:'ses_fixture',callID:'call2',args:{marker:'mcp-fixture'}},mcp);
await plugin['tool.execute.after']({tool:'nativeproof_record',sessionID:'ses_fixture',callID:'call3',args:{marker:'mcp-fixture'}},null);
console.log(JSON.stringify({output,mcp,before}));"#;
    let output = node_command()
        .args(["--input-type=module", "-e", source_code, plugin_url])
        .arg(&files.workspace)
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let returned: Value = serde_json::from_slice(&output.stdout).assert_value();
    assert_eq!(returned["output"]["output"], "original recalled-marker");
    let before = &returned["before"];
    let contents = returned["mcp"]["content"].as_array().assert_value();
    assert_eq!(&contents[..4], before["content"].as_array().assert_value());
    assert_eq!(
        contents[4],
        serde_json::json!({"type":"text","text":" recalled-marker"})
    );
    for field in [
        "output",
        "structuredContent",
        "metadata",
        "_meta",
        "isError",
    ] {
        assert_eq!(returned["mcp"][field], before[field]);
    }
    drop(adapter);
    assert_eq!(source.callbacks.load(Ordering::SeqCst), 3);
    assert_eq!(
        source.failures.load(Ordering::SeqCst),
        1,
        "unsupported null output must remain unavailable rather than being acknowledged"
    );
}
