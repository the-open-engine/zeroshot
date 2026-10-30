use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_capsule::provider_process::ProviderExecutionFiles;
use crate::native_v2_runner::NodeRole;
use serde_json::Value;

fn setup(provider: &str) -> (TestDirectory, HermesAdapter, ProviderExecutionFiles) {
    let directory = TestDirectory::new("hermes-command");
    let mut adapter = adapter(
        &directory,
        "#!/usr/bin/env python3\nraise AssertionError('prepare must not launch Hermes')\n",
    );
    adapter.config.provider = provider.to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    (directory, adapter, files)
}

fn prepare(
    adapter: &HermesAdapter,
    files: &ProviderExecutionFiles,
    fields: &[(&str, &str)],
) -> Result<(ProcessSessionCommand, Value), NodeRunnerError> {
    command::prepare(adapter, &invocation(fields), files, "owned-session")
}

#[test]
fn selected_provider_credentials_and_endpoints_stay_in_private_initialization() {
    for (provider, credential, endpoint) in [
        ("openai", "OPENAI_API_KEY", Some("OPENAI_BASE_URL")),
        ("anthropic", "ANTHROPIC_API_KEY", Some("ANTHROPIC_BASE_URL")),
        ("anthropic", "ANTHROPIC_TOKEN", Some("ANTHROPIC_BASE_URL")),
        ("anthropic", "CLAUDE_CODE_OAUTH_TOKEN", None),
        (
            "openrouter",
            "OPENROUTER_API_KEY",
            Some("OPENROUTER_BASE_URL"),
        ),
        ("github", "COPILOT_GITHUB_TOKEN", None),
        ("github", "GH_TOKEN", None),
        ("github", "GITHUB_TOKEN", None),
        ("bedrock", "AWS_BEARER_TOKEN_BEDROCK", None),
    ] {
        let (_directory, adapter, files) = setup(provider);
        let mut fields = vec![
            (credential, "selected-private-key"),
            ("MCP_NATIVE_TOKEN", "native-plugin-value"),
        ];
        if let Some(endpoint) = endpoint {
            fields.push((endpoint, "https://native.example/caller/path"));
        }
        if provider == "bedrock" {
            fields.push(("AWS_REGION", "us-east-1"));
        }
        let (command, init) = prepare(&adapter, &files, &fields).assert_value();
        assert_eq!(init["provider"], provider);
        assert_eq!(init["authority"]["apiKey"], "selected-private-key");
        assert_eq!(
            init["providerEnvironment"][credential],
            "selected-private-key"
        );
        assert!(init["declaredEnvironment"].get(credential).is_none());
        assert!(!command.environment.contains_key(credential));
        assert_eq!(
            command.environment["MCP_NATIVE_TOKEN"],
            "native-plugin-value"
        );
        assert_eq!(
            init["declaredEnvironment"]["MCP_NATIVE_TOKEN"],
            "native-plugin-value"
        );
        if endpoint.is_some() {
            assert_eq!(
                init["authority"]["baseUrl"],
                "https://native.example/caller/path"
            );
        }
    }
}

#[test]
fn gateway_transport_is_authored_and_does_not_change_the_model_or_base_path() {
    for (api, native) in [
        ("openai-responses", "codex_responses"),
        ("openai-completions", "chat_completions"),
        ("anthropic-messages", "anthropic_messages"),
    ] {
        let (_directory, adapter, files) = setup("gateway");
        let (command, init) = prepare(
            &adapter,
            &files,
            &[
                ("GATEWAY_API_KEY", "selected-private-key"),
                ("GATEWAY_BASE_URL", "https://native.example/caller/path"),
                ("GATEWAY_API", api),
            ],
        )
        .assert_value();
        assert_eq!(init["authority"]["apiMode"], native);
        assert_eq!(
            init["authority"]["baseUrl"],
            "https://native.example/caller/path"
        );
        assert_eq!(init["model"], "opaque/model");
        assert!(!command.environment.contains_key("GATEWAY_API_KEY"));
    }
}

#[test]
fn invalid_credentials_endpoints_and_gateway_modes_refuse_before_bootstrap() {
    let cases = [
        (
            "openai",
            vec![("OPENAI_API_KEY", " ")],
            "Hermes declared credential is empty",
        ),
        (
            "openai",
            vec![("ANTHROPIC_TOKEN", "foreign-key")],
            "Hermes credential conflicts with the selected provider",
        ),
        (
            "openai",
            vec![("OPENAI_BASE_URL", " ")],
            "Hermes declared endpoints are empty or disagree",
        ),
        (
            "openai",
            vec![
                ("OPENAI_BASE_URL", "https://first.example"),
                ("OPENAI_API_BASE", "https://second.example"),
            ],
            "Hermes declared endpoints are empty or disagree",
        ),
        (
            "gateway",
            vec![
                ("GATEWAY_API_KEY", "key"),
                ("GATEWAY_BASE_URL", "https://native.example/path"),
            ],
            "Hermes gateway requires a supported GATEWAY_API",
        ),
        (
            "gateway",
            vec![
                ("GATEWAY_API_KEY", "key"),
                ("GATEWAY_BASE_URL", "https://native.example/path"),
                ("GATEWAY_API", "guessed-transport"),
            ],
            "Hermes gateway requires a supported GATEWAY_API",
        ),
        (
            "gateway",
            vec![
                ("GATEWAY_API_KEY", "key"),
                (
                    "GATEWAY_BASE_URL",
                    "https://user:password@native.example/path",
                ),
                ("GATEWAY_API", "openai-responses"),
            ],
            "GATEWAY_BASE_URL must be an absolute HTTP(S) base URL without credentials, query, or fragment",
        ),
        ("unsupported", vec![], "Hermes provider is unsupported"),
    ];
    for (provider, fields, message) in cases {
        let (_directory, adapter, files) = setup(provider);
        assert_eq!(
            prepare(&adapter, &files, &fields).err().assert_value(),
            NodeRunnerError::DriverDetail(message.to_owned())
        );
        assert!(!files.home().join("native_runtime.py").exists());
    }
    let (_directory, adapter, files) = setup("openai");
    let (_, init) = prepare(
        &adapter,
        &files,
        &[
            ("OPENAI_BASE_URL", "https://same.example/path"),
            ("OPENAI_API_BASE", "https://same.example/path"),
        ],
    )
    .assert_value();
    assert_eq!(init["authority"]["baseUrl"], "https://same.example/path");
}

#[test]
fn declared_aws_authentication_replaces_ambient_auth_but_keeps_native_discovery() {
    let (_directory, mut adapter, files) = setup("bedrock");
    adapter.config.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([
        ("AWS_PROFILE".to_owned(), "ambient-profile".to_owned()),
        ("AWS_ACCESS_KEY_ID".to_owned(), "ambient-key".to_owned()),
        (
            "AWS_WEB_IDENTITY_TOKEN_FILE".to_owned(),
            "/native/ambient-token".to_owned(),
        ),
        (
            "AWS_CONFIG_FILE".to_owned(),
            "/native/aws-config".to_owned(),
        ),
        ("ANTHROPIC_API_KEY".to_owned(), "unrelated-login".to_owned()),
        ("ZEROSHOT_INTERNAL_VALUE".to_owned(), "host-only".to_owned()),
    ]));
    for fields in [
        vec![
            ("AWS_ACCESS_KEY_ID", "declared-id"),
            ("AWS_SECRET_ACCESS_KEY", "declared-secret"),
            ("AWS_SESSION_TOKEN", "declared-session"),
            ("AWS_REGION", "us-east-1"),
        ],
        vec![
            ("AWS_PROFILE", "declared-profile"),
            ("AWS_REGION", "us-east-1"),
        ],
        vec![
            ("AWS_BEARER_TOKEN_BEDROCK", "declared-bearer"),
            ("AWS_REGION", "us-east-1"),
        ],
    ] {
        let (command, init) = prepare(&adapter, &files, &fields).assert_value();
        for name in [
            "AWS_PROFILE",
            "AWS_ACCESS_KEY_ID",
            "AWS_WEB_IDENTITY_TOKEN_FILE",
            "ZEROSHOT_INTERNAL_VALUE",
        ] {
            assert!(init["nativeEnvironment"].get(name).is_none());
        }
        assert_eq!(
            init["nativeEnvironment"]["AWS_CONFIG_FILE"],
            "/native/aws-config"
        );
        assert_eq!(
            init["nativeEnvironment"]["ANTHROPIC_API_KEY"],
            "unrelated-login"
        );
        assert_eq!(init["authority"]["aws"]["AWS_REGION"], "us-east-1");
        for (name, value) in fields {
            assert_eq!(init["authority"]["aws"][name], value);
            if matches!(
                name,
                "AWS_ACCESS_KEY_ID"
                    | "AWS_SECRET_ACCESS_KEY"
                    | "AWS_SESSION_TOKEN"
                    | "AWS_BEARER_TOKEN_BEDROCK"
            ) {
                assert!(!command.environment.contains_key(name));
            }
        }
    }
}

#[test]
fn empty_or_mixed_aws_authority_refuses_before_native_bootstrap() {
    for (fields, expected) in [
        (
            vec![("AWS_PROFILE", " ")],
            "Hermes declared AWS field is empty",
        ),
        (
            vec![("AWS_REGION", " ")],
            "Hermes declared AWS field is empty",
        ),
        (
            vec![("AWS_BEARER_TOKEN_BEDROCK", "bearer")],
            "Hermes declared Bedrock bearer requires AWS_REGION",
        ),
        (
            vec![
                ("AWS_BEARER_TOKEN_BEDROCK", "bearer"),
                ("AWS_REGION", "us-east-1"),
                ("AWS_PROFILE", "foreign-profile"),
            ],
            "Hermes Bedrock bearer and IAM credentials conflict",
        ),
        (
            vec![
                ("AWS_BEARER_TOKEN_BEDROCK", "bearer"),
                ("AWS_REGION", "us-east-1"),
                ("AWS_ACCESS_KEY_ID", "foreign-id"),
            ],
            "Hermes Bedrock bearer and IAM credentials conflict",
        ),
    ] {
        let (_directory, adapter, files) = setup("bedrock");
        assert_eq!(
            prepare(&adapter, &files, &fields).err().assert_value(),
            NodeRunnerError::DriverDetail(expected.to_owned())
        );
        assert!(!files.home().join("sitecustomize.py").exists());
    }
}

#[test]
fn native_login_paths_permissions_and_python_resources_survive_local_preparation() {
    let (directory, mut adapter, files) = setup("openai");
    let native_python = directory.child("native-python");
    adapter.config.prefix_arguments = vec!["--profile".to_owned(), "native profile".to_owned()];
    adapter.config.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([
        (
            "PYTHONPATH".to_owned(),
            native_python.to_string_lossy().into_owned(),
        ),
        (
            "HERMES_HOME".to_owned(),
            directory
                .child("native-profile")
                .to_string_lossy()
                .into_owned(),
        ),
        ("HERMES_EXEC_ASK".to_owned(), "native-policy".to_owned()),
        (
            "OPENAI_API_KEY".to_owned(),
            "native-selected-login".to_owned(),
        ),
        ("ANTHROPIC_API_KEY".to_owned(), "unrelated-login".to_owned()),
    ]));
    adapter.local_environment = adapter
        .config
        .native_environment
        .selected(command::LOCAL_ENVIRONMENT);
    let mut invocation = invocation(&[("HERMES_EXEC_ASK", "declared-policy")]);
    invocation.node.binding = serde_json::from_value(
        json!({"kind":"agent","model":"caller/opaque-model","effort":"high"}),
    )
    .assert_value();
    invocation.role = NodeRole::Verifier;
    let (command, init) =
        command::prepare(&adapter, &invocation, &files, "native-session").assert_value();
    assert_eq!(
        command.argv,
        ["--profile", "native profile", "--version", "--help"]
    );
    assert_eq!(
        command.environment["HOME"],
        directory.path().to_string_lossy()
    );
    assert_eq!(
        command.environment["HERMES_HOME"],
        directory.child("native-profile").to_string_lossy()
    );
    assert_eq!(command.environment["HERMES_EXEC_ASK"], "declared-policy");
    assert!(!command.environment.contains_key("OPENAI_API_KEY"));
    assert_eq!(
        init["nativeEnvironment"]["OPENAI_API_KEY"],
        "native-selected-login"
    );
    assert_eq!(init["model"], "caller/opaque-model");
    assert_eq!(init["effort"], "high");
    assert_eq!(init["sessionId"], "native-session");
    assert_eq!(init["contained"], false);
    let python_path = std::env::split_paths(&command.environment["PYTHONPATH"]).collect::<Vec<_>>();
    assert_eq!(python_path, [files.home(), native_python.as_path()]);
    assert_eq!(
        command.workspace.mode,
        crate::execution::WorkspaceAccessMode::ReadWrite
    );
    for filename in [
        "sitecustomize.py",
        "native_runtime.py",
        "native_auth.py",
        "native_providers.py",
        "native_memory.py",
        "native_environment.py",
    ] {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(files.home().join(filename)).assert_value();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn contained_preparation_discards_native_state_and_uses_its_private_home() {
    let (_directory, mut local, files) = setup("anthropic");
    local.config.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([
        ("ANTHROPIC_API_KEY".to_owned(), "ambient-login".to_owned()),
        ("HERMES_EXEC_ASK".to_owned(), "ambient-policy".to_owned()),
    ]));
    let contained = HermesAdapter::new(local.config);
    let (command, init) =
        prepare(&contained, &files, &[("ANTHROPIC_API_KEY", "declared-key")]).assert_value();
    assert!(contained.local_environment.is_empty());
    assert_eq!(init["nativeEnvironment"], json!({}));
    assert_eq!(init["contained"], true);
    assert_eq!(init["authority"]["apiKey"], "declared-key");
    assert_eq!(command.environment["HOME"], files.home().to_string_lossy());
    assert!(!command.environment.contains_key("HERMES_EXEC_ASK"));
    assert!(!command.environment.contains_key("ANTHROPIC_API_KEY"));
}

#[test]
fn invalid_binding_and_reserved_declared_control_never_install_a_runtime() {
    let (_directory, adapter, files) = setup("openai");
    let mut delivery = invocation(&[]);
    delivery.node.binding =
        serde_json::from_value(json!({"kind":"comment_delivery"})).assert_value();
    assert_eq!(
        command::prepare(&adapter, &delivery, &files, "unused")
            .err()
            .assert_value(),
        NodeRunnerError::InvalidRole
    );
    for name in [
        "HOME",
        "HERMES_HOME",
        "PYTHONPATH",
        "XDG_CONFIG_HOME",
        "ZEROSHOT_INTERNAL_VALUE",
    ] {
        assert!(prepare(&adapter, &files, &[(name, "foreign-control")]).is_err());
    }
    assert!(!files.home().join("native_runtime.py").exists());
}

#[test]
fn unsafe_bootstrap_paths_and_installation_errors_fail_closed() {
    use std::os::unix::ffi::OsStringExt;
    let (_directory, mut adapter, files) = setup("openai");
    adapter.config.local_user_home = Some(PathBuf::from(std::ffi::OsString::from_vec(
        b"/non-utf8-\xff".to_vec(),
    )));
    assert_eq!(
        prepare(&adapter, &files, &[]).err().assert_value(),
        NodeRunnerError::DriverDetail("Hermes home is not UTF-8".to_owned())
    );
    adapter.config.local_user_home = None;
    std::fs::create_dir(files.home().join("sitecustomize.py")).assert_value();
    assert_eq!(
        prepare(&adapter, &files, &[]).err().assert_value(),
        NodeRunnerError::DriverDetail("Hermes private runtime could not be installed".to_owned())
    );
    let directory = TestDirectory::new("hermes:python-path");
    let adapter = super::adapter(
        &directory,
        "#!/usr/bin/env python3\nraise AssertionError('unused')\n",
    );
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    assert_eq!(
        prepare(&adapter, &files, &[]).err().assert_value(),
        NodeRunnerError::DriverDetail("Hermes PYTHONPATH is invalid".to_owned())
    );
}

struct MemoryFailures(std::sync::atomic::AtomicUsize);

impl crate::active_memory::hooks::ActiveMemorySource for MemoryFailures {
    fn tool_context(&self, _: &crate::active_memory::hooks::HookQuery) -> String {
        String::new()
    }
    fn graph_context(&self, _: &str, _: &str) -> String {
        String::new()
    }
    fn unavailable(&self, _: &str) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn active_memory_capability_is_private_and_failed_installation_reports_unknown_delivery() {
    let (_directory, adapter, files) = setup("openai");
    let source = Arc::new(MemoryFailures(std::sync::atomic::AtomicUsize::new(0)));
    let hooks = crate::active_memory::hooks::ActiveMemoryHooks::new(source.clone()).assert_value();
    let adapter = adapter.with_active_memory(Some(Arc::new(hooks)));
    let (_, init) = prepare(&adapter, &files, &[]).assert_value();
    let path = PathBuf::from(init["memoryCapability"].as_str().assert_value());
    assert_eq!(path, files.home().join("active-memory-capability.json"));
    let capability: Value =
        serde_json::from_slice(&std::fs::read(&path).assert_value()).assert_value();
    assert!(capability.is_object());
    assert_eq!(source.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    std::fs::remove_file(&path).assert_value();
    std::fs::create_dir(&path).assert_value();
    let (_, init) = prepare(&adapter, &files, &[]).assert_value();
    assert!(init["memoryCapability"].is_null());
    assert_eq!(source.0.load(std::sync::atomic::Ordering::SeqCst), 1);
}
