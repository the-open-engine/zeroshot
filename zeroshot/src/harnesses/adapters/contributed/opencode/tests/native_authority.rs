//! Native wire regression with a localhost provider and synthetic credentials only.
use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn native_openai_adapter(
    directory: &TestDirectory,
    header: Option<(&str, &str)>,
) -> OpenCodeAdapter {
    let mut adapter = super::process::adapter(directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "openai".to_owned();
    let binary =
        std::env::var("ZEROSHOT_OPENCODE_BINARY").expect("set the explicit native OpenCode binary");
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let headers = match header {
        Some((name, value)) => json!({ "X-Native-Trace": "native-trace", (name): value }),
        None => json!({ "X-Native-Trace": "native-trace" }),
    };
    let config = json!({"provider":{"openai":{"options":{"apiKey":"stored-native-key","headers":headers},"models":{"opaque/model":{"id":"wire-fixture","name":"Wire fixture","limit":{"context":100000,"output":4096}}}}}});
    adapter.local_environment = native_wire_environment(directory, config);
    let script = format!(
        "#!/bin/sh\nexec {} --print-logs --log-level debug \"$@\" 2>>{}\n",
        quote(&binary),
        quote(&directory.child("native-stderr.log").to_string_lossy())
    );
    adapter.config.executable = directory
        .write_executable("native-opencode-debug", &script)
        .to_string_lossy()
        .into_owned();
    adapter
}

#[tokio::test]
#[ignore = "requires installed OpenCode; uses localhost and dummy keys only"]
async fn native_declared_key_reaches_the_wire_without_native_authority() {
    let directory = TestDirectory::new("opencode-native-declared-key-wire");
    let adapter = native_openai_adapter(&directory, None);
    let (listener, endpoint) = native_listener().await;
    let invocation = super::process::invocation(&[
        ("OPENAI_API_KEY", "declared-wire-key"),
        ("OPENAI_BASE_URL", endpoint.as_str()),
    ]);
    let captured = run_native_wire(&adapter, invocation, listener)
        .await
        .to_ascii_lowercase();
    assert!(
        captured.contains("authorization: bearer declared-wire-key"),
        "{captured}"
    );
    assert!(
        captured.contains("x-native-trace: native-trace"),
        "{captured}"
    );
    assert!(!captured.contains("stored-native-key"), "{captured}");
}

#[tokio::test]
#[ignore = "requires installed OpenCode; proves a native auth header conflict stops the request before localhost HTTP"]
async fn native_auth_header_from_user_configuration_stops_the_request() {
    let directory = TestDirectory::new("opencode-native-header-conflict");
    let adapter = native_openai_adapter(&directory, Some(("X-Api-Key", "native-header-secret")));
    let (listener, endpoint) = native_listener().await;
    let invocation = super::process::invocation(&[
        ("OPENAI_API_KEY", "declared-wire-key"),
        ("OPENAI_BASE_URL", endpoint.as_str()),
    ]);
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let execution = adapter.run(invocation, control);
    tokio::pin!(execution);
    let result = tokio::select! {
        _ = listener.accept() =>
            panic!("provider request crossed a native auth header conflict"),
        result = &mut execution => result,
        _ = tokio::time::sleep(std::time::Duration::from_secs(60)) =>
            panic!("native auth header conflict did not stop the request"),
    };
    let error = result.unwrap_err();
    let detail = format!("{error}");
    let text = detail.clone() + &native_stderr(&directory);
    assert!(text.contains("declared provider conflict"), "{text}");
    assert!(text.contains("x-api-key"), "{text}");
    assert!(!detail.contains("native-header-secret"), "{detail}");
    assert!(!detail.contains("declared-wire-key"), "{detail}");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
            .await
            .is_err(),
        "provider request crossed a native auth header conflict"
    );
    drain.await.assert_value();
}

#[tokio::test]
#[ignore = "requires installed OpenCode on Linux; proves cancellation stops the private server"]
#[cfg(target_os = "linux")]
async fn native_cancellation_stops_every_opencode_process_of_the_session() {
    let directory = TestDirectory::new("opencode-native-cancellation");
    let adapter = native_openai_adapter(&directory, None);
    let (listener, endpoint) = native_listener().await;
    let invocation = super::process::invocation(&[
        ("OPENAI_API_KEY", "declared-wire-key"),
        ("OPENAI_BASE_URL", endpoint.as_str()),
    ]);
    let (cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let (sender, received) = tokio::sync::oneshot::channel();
    let held = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.assert_value();
        let mut chunk = [0; 8192];
        let mut bytes = Vec::new();
        loop {
            let read = stream.read(&mut chunk).await.assert_value();
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            if bytes
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
                .is_some()
            {
                break;
            }
        }
        let _ = sender.send(());
        let _ = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                if stream.read(&mut chunk).await.unwrap_or(0) == 0 {
                    break;
                }
            }
        })
        .await;
    });

    let execution = adapter.run(invocation, control);
    tokio::pin!(execution, received);
    tokio::select! {
        read = &mut received => {
            read.assert_value();
            cancel.send(true).assert_value();
        }
        result = &mut execution => panic!("native run ended before cancellation: {result:?}"),
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), execution)
        .await
        .assert_value();
    assert!(
        matches!(result, Err(NodeRunnerError::Cancelled)),
        "{result:?}"
    );
    drain.await.assert_value();
    held.abort();

    let marker = directory.path().to_string_lossy().into_owned();
    let current = std::process::id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let mut remaining = Vec::new();
        for entry in std::fs::read_dir("/proc").assert_value() {
            let Ok(entry) = entry else { continue };
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid == current {
                continue;
            }
            let Ok(bytes) = std::fs::read(format!("/proc/{pid}/environ")) else {
                continue;
            };
            if String::from_utf8_lossy(&bytes).contains(&marker) {
                remaining.push(pid);
            }
        }
        if remaining.is_empty() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("opencode processes still hold the session directory: {remaining:?}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
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

fn native_stderr(directory: &TestDirectory) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(directory.child("native-stderr.log")) else {
        return String::new();
    };
    let mut bytes = Vec::new();
    let _ = file.take(64 * 1024).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}
