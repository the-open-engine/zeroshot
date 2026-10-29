//! Native wire regression with a localhost provider and synthetic credentials only.
use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn native_configuration(header: &str, secret: &str) -> Value {
    json!({"permission":{"*":"allow","edit":"deny"},
        "mcp":{"native":{"type":"remote","url":"http://127.0.0.1:1/mcp","enabled":false,"headers":{"Authorization":"mcp-fixture-key"}}},
        "provider":{"openai":{
        "options":{"apiKey":"stored-native-key","headers":{(header):secret}},
        "models":{"opaque/model":{"id":"wire-fixture","name":"Local fixture",
            "limit":{"context":200000,"output":16000},"headers":{"X-Native-Model":"model-native-secret"}}}
    }}})
}

fn native_adapter(directory: &TestDirectory) -> OpenCodeAdapter {
    let mut adapter = super::process::adapter(directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "openai".to_owned();
    let binary =
        std::env::var("ZEROSHOT_OPENCODE_BINARY").expect("set the explicit native OpenCode binary");
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let script = format!(
        "#!/bin/sh\nexec {} --print-logs --log-level DEBUG \"$@\" 2>>{}\n",
        quote(&binary),
        quote(&directory.child("native-stderr.log").to_string_lossy())
    );
    adapter.config.executable = directory
        .write_executable("native-opencode-debug", &script)
        .to_string_lossy()
        .into_owned();
    let config_root = directory.child("native-config");
    std::fs::create_dir_all(config_root.join("opencode")).assert_value();
    native_project_resources(directory, &adapter.config.workspace, &config_root);
    let data = directory.child("native-data/opencode");
    std::fs::create_dir_all(&data).assert_value();
    std::fs::write(
        data.join("auth.json"),
        json!({"openai":{"type":"api","key":"original-native-login"},
        "native-refresh-fixture/":{"type":"api","key":"old-unrelated-login"}})
        .to_string(),
    )
    .assert_value();
    let managed = directory.child("native-managed");
    std::fs::create_dir_all(&managed).assert_value();
    let managed_plugin = native_managed_headers(directory, false);
    std::fs::write(managed.join("opencode.json"), json!({"permission":{"edit":"deny"},"plugin":[managed_plugin],
        "provider":{"openai":{"options":{"apiKey":"managed-native-key","headers":{"X-Managed-Native":"managed-native-secret"}}}}}).to_string()).assert_value();
    std::fs::write(
        config_root.join("opencode/opencode.json"),
        native_configuration("Authorization", "Bearer global-native-secret").to_string(),
    )
    .assert_value();
    let custom = directory.write(
        "custom.json",
        &native_configuration("X-Native-Custom", "custom-native-secret").to_string(),
    );
    adapter.local_environment = BTreeMap::from([
        (
            "XDG_CONFIG_HOME".to_owned(),
            config_root.to_string_lossy().into_owned(),
        ),
        (
            "XDG_DATA_HOME".to_owned(),
            directory
                .child("native-data")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "XDG_STATE_HOME".to_owned(),
            directory
                .child("native-state")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "XDG_CACHE_HOME".to_owned(),
            directory
                .child("native-cache")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "OPENCODE_CONFIG".to_owned(),
            custom.to_string_lossy().into_owned(),
        ),
        (
            "OPENCODE_CONFIG_CONTENT".to_owned(),
            native_configuration("X-Native-Inline", "inline-native-secret").to_string(),
        ),
        ("OPENCODE_DISABLE_MODELS_FETCH".to_owned(), "1".to_owned()),
        ("OPENCODE_DISABLE_LSP_DOWNLOAD".to_owned(), "1".to_owned()),
        // An injected host fixture selects OpenCode's native managed-config test seam. Declared
        // connections cannot choose this control; production uses its ordinary managed sources.
        (
            "OPENCODE_TEST_MANAGED_CONFIG_DIR".to_owned(),
            managed.to_string_lossy().into_owned(),
        ),
    ]);
    adapter
}

fn native_managed_headers(directory: &TestDirectory, conflict: bool) -> String {
    let source = format!(
        r#"const conflict={conflict};
const selected=(id)=>({{openai:'OPENAI_API_KEY',openrouter:'OPENROUTER_API_KEY'}})[id];
export default async()=>({{
  config:async(c)=>{{
    for(const [id,p] of Object.entries(c.provider??{{}})){{
      const field=selected(id);if(!field)continue;
      const value=`Bearer ${{process.env[field]}}`;
      p.options.headers={{...p.options.headers,Authorization:value,'X-SDK-Metadata':'retained'}};
      for(const model of Object.values(p.models??{{}}))model.headers={{...model.headers,Authorization:value}};
    }}
  }},
  'chat.headers':async(input,output)=>{{
    const field=selected(input.model.providerID);if(!field)return;
    output.headers['X-Managed-Mutation']='retained';
    output.headers={{...output.headers,'X-Managed-Replacement':'retained'}};
    if(conflict)output.headers.Authorization='Bearer different-private-key';
    else output.headers.authorization=`Bearer ${{process.env[field]}}`;
  }}
}});"#
    );
    let path = directory.write("managed-header-plugin.mjs", &source);
    url::Url::from_file_path(path).assert_value().to_string()
}

fn retarget_native_lane(directory: &TestDirectory, adapter: &mut OpenCodeAdapter, provider: &str) {
    adapter.config.provider = provider.to_owned();
    for path in [
        directory.child("native-config/opencode/opencode.json"),
        directory.child("custom.json"),
        directory.child("native-managed/opencode.json"),
    ] {
        let mut config: Value =
            serde_json::from_slice(&std::fs::read(&path).assert_value()).assert_value();
        retarget_configuration(&mut config, provider);
        std::fs::write(path, config.to_string()).assert_value();
    }
    let mut inline: Value =
        serde_json::from_str(&adapter.local_environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    retarget_configuration(&mut inline, provider);
    adapter
        .local_environment
        .insert("OPENCODE_CONFIG_CONTENT".to_owned(), inline.to_string());
    let path = directory.child("native-data/opencode/auth.json");
    let mut auth: Value =
        serde_json::from_slice(&std::fs::read(&path).assert_value()).assert_value();
    let stored = auth
        .as_object_mut()
        .assert_value()
        .remove("openai")
        .assert_value();
    auth[provider] = stored;
    std::fs::write(path, auth.to_string()).assert_value();
}

fn retarget_configuration(config: &mut Value, provider: &str) {
    let providers = config["provider"].as_object_mut().assert_value();
    let mut selected = providers.remove("openai").assert_value();
    selected["options"]["api_keys"] = json!({"openai":"native-upstream-private-key"});
    providers.insert(provider.to_owned(), selected);
}

fn native_project_resources(
    directory: &TestDirectory,
    workspace: &std::path::Path,
    config_root: &std::path::Path,
) {
    for root in [
        config_root.join("opencode/commands"),
        workspace.join(".opencode/commands"),
        workspace.join(".opencode/plugins"),
    ] {
        std::fs::create_dir_all(root).assert_value();
    }
    std::fs::write(
        config_root.join("opencode/commands/global-proof.md"),
        "Global native command.",
    )
    .assert_value();
    std::fs::write(
        workspace.join(".opencode/commands/project-proof.md"),
        "Project native command.",
    )
    .assert_value();
    std::fs::write(
        workspace.join(".opencode/resource-proof.txt"),
        "project-native-resource",
    )
    .assert_value();
    let marker = serde_json::to_string(&directory.child("native-plugin-effects")).assert_value();
    let receipt =
        serde_json::to_string(&directory.child("native-plugin-receipt.json")).assert_value();
    let source = format!(
        r#"import fs from 'node:fs';import path from 'node:path';
export default async ({{directory,client}})=>{{
  fs.appendFileSync({marker},'initialized\n');
  return {{config:async(c)=>{{
    c.permission={{...c.permission,webfetch:'deny'}};
    fs.appendFileSync({marker},'configured\n');
    fs.writeFileSync({receipt},JSON.stringify({{
      mcp:c.mcp.native.headers.Authorization,permission:c.permission,
      globalCommand:!!c.command?.['global-proof'],projectCommand:!!c.command?.['project-proof'],
      wellknown:c.command?.['wellknown-native-proof']?.template==='Discovery wellknown-fixture-token',
      resource:fs.readFileSync(path.join(directory,'.opencode/resource-proof.txt'),'utf8')
    }}));
  }},'chat.params':async()=>{{
    const result=await client.auth.set({{path:{{id:'native-refresh-fixture'}},body:{{type:'oauth',refresh:'refreshed-token',access:'refreshed-access',expires:10}}}});
    if(result.data!==true)throw new Error('native auth refresh fixture failed');
  }}}};
}};"#
    );
    std::fs::write(workspace.join(".opencode/plugins/native-proof.js"), source).assert_value();
}

pub(super) async fn capture_request(listener: tokio::net::TcpListener) -> String {
    let (mut stream, _) = listener.accept().await.assert_value();
    let mut bytes = Vec::new();
    let headers = loop {
        let mut chunk = [0; 8192];
        let read = stream.read(&mut chunk).await.assert_value();
        assert!(
            read > 0,
            "native provider closed before sending HTTP headers"
        );
        bytes.extend_from_slice(&chunk[..read]);
        assert!(
            bytes.len() < 2 * 1024 * 1024,
            "wire fixture request exceeded its bound"
        );
        if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let header_text = String::from_utf8(bytes[..headers].to_vec()).assert_value();
    let body = r#"{"error":{"message":"local wire fixture complete","type":"authentication_error","code":"invalid_api_key"}}"#;
    stream.write_all(format!("HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes())
        .await.assert_value();
    stream.shutdown().await.assert_value();
    header_text
}

#[tokio::test]
#[ignore = "requires an installed OpenCode executable; uses localhost and dummy keys only"]
async fn native_declared_authority_does_not_send_inherited_headers() {
    for stored in [
        json!({"type":"api","key":"original-native-login"}),
        json!({"type":"oauth","refresh":"original-native-refresh","access":"original-native-access","expires":0}),
    ] {
        native_wire_case(stored, "openai").await;
    }
    native_wire_case(
        json!({"type":"api","key":"original-native-login"}),
        "openrouter",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires installed OpenCode; uses localhost and synthetic gateway/Bedrock credentials"]
async fn native_gateway_and_bedrock_use_declared_authentication_on_the_wire() {
    for api in [
        "openai-responses",
        "openai-completions",
        "anthropic-messages",
    ] {
        additional_native_wire("gateway", api).await;
    }
    additional_native_wire("bedrock", "converse").await;
}

async fn additional_native_wire(provider: &str, api: &str) {
    let directory = TestDirectory::new("opencode-additional-native-wire");
    let mut adapter = super::process::adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = provider.to_owned();
    adapter.config.executable =
        std::env::var("ZEROSHOT_OPENCODE_BINARY").expect("explicit native binary");
    let id = command::native_provider(provider);
    let config = json!({"provider":{(id):{"options":{"apiKey":"stored-key"},"models":{"opaque/model":{"id":"wire-fixture","name":"Wire fixture","limit":{"context":100000,"output":4096}}}}}});
    adapter.local_environment = native_wire_environment(&directory, config);
    let (listener, endpoint) = native_listener().await;
    let credential = command::credential_field(provider).assert_value();
    let endpoint_field = command::endpoint_fields(provider)[0];
    let mut fields = vec![
        (credential, "declared-wire-key"),
        (endpoint_field, endpoint.as_str()),
    ];
    fields.push(if provider == "gateway" {
        ("GATEWAY_API", api)
    } else {
        ("AWS_REGION", "us-east-1")
    });
    let invocation = super::process::invocation(&fields);
    let captured = run_native_wire(&adapter, invocation, listener).await;
    let captured = captured.to_ascii_lowercase();
    let header = if api == "anthropic-messages" {
        "x-api-key: declared-wire-key"
    } else {
        "authorization: bearer declared-wire-key"
    };
    assert!(
        captured.contains(header),
        "native {provider}/{api} must use selected auth: {captured}"
    );
    assert!(!captured.contains("stored-key"));
    let suffix = match api {
        "openai-responses" => "/responses",
        "openai-completions" => "/chat/completions",
        "anthropic-messages" => "/messages",
        _ => "/converse-stream",
    };
    assert!(captured.lines().next().assert_value().contains(suffix));
}

pub(super) async fn native_listener() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .assert_value();
    let endpoint = format!("http://{}/v1", listener.local_addr().assert_value());
    (listener, endpoint)
}

pub(super) fn native_wire_environment(
    directory: &TestDirectory,
    config: Value,
) -> BTreeMap<String, String> {
    let mut environment: BTreeMap<String, String> = [
        ("XDG_DATA_HOME", "data"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
    ]
    .into_iter()
    .map(|(name, path)| {
        (
            name.to_owned(),
            directory.child(path).to_string_lossy().into_owned(),
        )
    })
    .collect();
    environment.extend([
        ("OPENCODE_DISABLE_MODELS_FETCH".to_owned(), "1".to_owned()),
        ("OPENCODE_DISABLE_LSP_DOWNLOAD".to_owned(), "1".to_owned()),
        ("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string()),
    ]);
    environment
}

pub(super) async fn run_native_wire(
    adapter: &OpenCodeAdapter,
    invocation: DriverInvocation,
    listener: tokio::net::TcpListener,
) -> String {
    let (cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let execution = adapter.run(invocation, control);
    let wire = tokio::time::timeout(
        std::time::Duration::from_secs(40),
        capture_request(listener),
    );
    tokio::pin!(execution, wire);
    let captured = tokio::select! {
        captured = &mut wire => captured.assert_value_with("native provider wire request"),
        result = &mut execution => panic!("native {} exited before HTTP: {result:?}", adapter.config.provider),
    };
    cancel.send(true).assert_value();
    let _result = tokio::time::timeout(std::time::Duration::from_secs(10), execution)
        .await
        .assert_value();
    drain.await.assert_value();
    captured
}

#[tokio::test]
#[ignore = "requires installed OpenCode; proves later native managed header conflicts before localhost HTTP"]
async fn native_later_managed_header_conflict_is_rejected_before_provider_http() {
    let directory = TestDirectory::new("opencode-native-late-header-conflict");
    let adapter = native_adapter(&directory);
    native_managed_headers(&directory, true);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .assert_value();
    let endpoint = format!("http://{}/v1", listener.local_addr().assert_value());
    let invocation = super::process::invocation(&[
        ("OPENAI_API_KEY", "declared-wire-key"),
        ("OPENAI_BASE_URL", &endpoint),
    ]);
    let (cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let execution = adapter.run(invocation, control);
    tokio::pin!(execution);
    let completed = tokio::select! {
        result = &mut execution => Some(result),
        request = listener.accept() => panic!("provider request crossed a conflicting native header: {request:?}"),
        _ = tokio::time::timeout(std::time::Duration::from_secs(30), wait_native_header_conflict(&directory)) => None,
    };
    let result = if let Some(result) = completed {
        result
    } else {
        cancel.send(true).assert_value();
        tokio::time::timeout(std::time::Duration::from_secs(10), execution)
            .await
            .assert_value()
    };
    drain.await.assert_value();
    assert!(result.is_err());
    assert!(
        native_stderr(&directory)
            .contains("Zeroshot OpenCode declared provider conflict: chat.headers"),
        "{}",
        native_stderr(&directory)
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

async fn wait_native_header_conflict(directory: &TestDirectory) {
    while !native_stderr(directory)
        .contains("Zeroshot OpenCode declared provider conflict: chat.headers")
    {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

async fn native_wire_case(stored: Value, provider: &str) {
    let directory = TestDirectory::new("opencode-native-wire-authority");
    let mut adapter = native_adapter(&directory);
    if provider != "openai" {
        retarget_native_lane(&directory, &mut adapter, provider);
    }
    let discovery = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .assert_value();
    let origin = format!("http://{}", discovery.local_addr().assert_value());
    let auth_path = directory.child("native-data/opencode/auth.json");
    let mut auth: Value =
        serde_json::from_slice(&std::fs::read(&auth_path).assert_value()).assert_value();
    auth[provider] = stored.clone();
    auth[origin] = json!({"type":"wellknown","key":"NATIVE_DISCOVERY_TOKEN","token":"wellknown-fixture-token"});
    std::fs::write(auth_path, auth.to_string()).assert_value();
    let discovered = tokio::spawn(tokio::time::timeout(
        std::time::Duration::from_secs(30),
        serve_wellknown(discovery),
    ));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .assert_value();
    let endpoint = format!("http://{}/v1", listener.local_addr().assert_value());
    let credential = command::credential_field(provider).assert_value();
    let endpoint_field = command::endpoint_fields(provider)[0];
    let invocation = super::process::invocation(&[
        (credential, "declared-wire-key"),
        (endpoint_field, &endpoint),
    ]);
    let (cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let output = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_output = output.clone();
    let drain = tokio::spawn(async move {
        while let Ok(event) = durable.recv_output().await {
            let mut output = captured_output.lock().assert_value();
            if output.len() < 128 {
                output.push(event);
            }
        }
    });
    let execution = adapter.run(invocation, control);
    let request = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        capture_request(listener),
    );
    tokio::pin!(execution, request);
    let captured = tokio::select! {
        biased;
        captured = &mut request => captured,
        result = &mut execution => panic!("native adapter exited before the wire request: {result:?}; output={:?}; stderr={}", output.lock().assert_value(), native_stderr(&directory)),
    };
    cancel.send(true).assert_value();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), execution)
        .await
        .assert_value_with("native adapter must confirm cleanup after wire-fixture cancellation");
    drain.await.assert_value();
    assert!(
        matches!(
            result,
            Err(NodeRunnerError::Cancelled) | Err(NodeRunnerError::DriverDetail(_))
        ),
        "the localhost fixture must cancel or refuse synthetic authentication"
    );
    let captured = captured
        .unwrap_or_else(|_| {
            panic!(
                "native request must reach the local provider; output={:?}; stderr={}",
                output.lock().assert_value(),
                native_stderr(&directory)
            )
        })
        .to_ascii_lowercase();
    discovered.await.assert_value().assert_value();
    assert_native_receipts(&directory, &stored, &captured, provider);
}

fn assert_native_receipts(
    directory: &TestDirectory,
    stored: &Value,
    captured: &str,
    provider: &str,
) {
    let auth: Value = serde_json::from_slice(
        &std::fs::read(directory.child("native-data/opencode/auth.json")).assert_value(),
    )
    .assert_value();
    assert_eq!(&auth[provider], stored);
    assert_eq!(auth["native-refresh-fixture"]["refresh"], "refreshed-token");
    assert!(auth.get("native-refresh-fixture/").is_none());
    assert_eq!(
        std::fs::read_to_string(directory.child("native-plugin-effects")).assert_value(),
        "initialized\nconfigured\n",
        "native startup must retain authored plugins without a separate debug invocation"
    );
    let receipt: Value = serde_json::from_slice(
        &std::fs::read(directory.child("native-plugin-receipt.json")).assert_value(),
    )
    .assert_value();
    assert_eq!(receipt["mcp"], "mcp-fixture-key");
    assert_eq!(receipt["permission"]["edit"], "deny");
    assert_eq!(receipt["permission"]["webfetch"], "deny");
    assert_eq!(receipt["globalCommand"], true);
    assert_eq!(receipt["projectCommand"], true);
    assert_eq!(receipt["resource"], "project-native-resource");
    assert_eq!(receipt["wellknown"], true);
    assert!(
        captured
            .lines()
            .any(|line| line == "authorization: bearer declared-wire-key"),
        "{captured}"
    );
    assert!(captured.contains("x-managed-mutation: retained"));
    assert!(captured.contains("x-managed-replacement: retained"));
    for forbidden in [
        "original-native",
        "global-native-secret",
        "custom-native-secret",
        "inline-native-secret",
        "model-native-secret",
        "managed-native-secret",
        "managed-native-key",
        "x-native-custom",
        "x-native-inline",
        "x-native-model",
        "x-managed-native",
        "native-upstream-private-key",
        "x-provider-api-keys",
    ] {
        assert!(
            !captured.contains(forbidden),
            "native authentication escaped into declared provider headers: {forbidden}"
        );
    }
}

fn native_stderr(directory: &TestDirectory) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(directory.child("native-stderr.log")) else {
        return String::new();
    };
    let mut bytes = Vec::new();
    let _ = file.take(64 * 1024).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn serve_wellknown(listener: tokio::net::TcpListener) {
    let (mut stream, _) = listener.accept().await.assert_value();
    let mut bytes = Vec::new();
    while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).await.assert_value();
        assert!(read > 0 && bytes.len() < 65536);
        bytes.extend_from_slice(&chunk[..read]);
    }
    assert!(String::from_utf8_lossy(&bytes).starts_with("GET /.well-known/opencode "));
    let body = json!({"config":{"command":{"wellknown-native-proof":{"template":"Discovery {env:NATIVE_DISCOVERY_TOKEN}"}}}}).to_string();
    stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.assert_value();
    stream.shutdown().await.assert_value();
}
