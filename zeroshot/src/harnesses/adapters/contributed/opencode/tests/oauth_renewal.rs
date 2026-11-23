//! Native OAuth renewal in the user's profile, with a loopback token endpoint and provider.
use super::process::{adapter, invocation};
use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Seen = Arc<StdMutex<Vec<String>>>;

// Test-only OAuth method of the `anthropic` integration; its refresh asks the loopback endpoint.
const STUB_OAUTH: &str = r#"export default {
  id: "zeroshot-test-oauth",
  async setup(ctx) {
    await ctx.integration.transform((editor) => {
      editor.method.update({
        integrationID: "anthropic",
        method: { id: "zs-stub", type: "oauth", label: "Stub" },
        authorize: async () => { throw new Error("stub has no sign-in"); },
        refresh: async (credential) => {
          const response = await fetch(TOKEN_URL, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ refresh: credential.refresh }) });
          const body = await response.json();
          return { type: "oauth", methodID: "zs-stub", refresh: body.refresh, access: body.access, expires: Date.now() + body.lifetime, metadata: credential.metadata };
        },
      });
    });
  },
};
"#;

/// Reads one HTTP request; returns its lower-case header block and its body.
async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut bytes = Vec::new();
    loop {
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                let body = String::from_utf8_lossy(&bytes[end + 4..end + 4 + length]).into_owned();
                return Some((head, body));
            }
        }
        let mut chunk = [0; 8192];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 || bytes.len() > 8 * 1024 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// Every refresh gets `renewed-access-<n>` and `renewed-refresh-<n>` with `lifetime` ms; the
/// refresh tokens it received are recorded in order.
async fn token_server(listener: tokio::net::TcpListener, lifetime: u64, seen: Seen) {
    let issued = std::sync::atomic::AtomicUsize::new(0);
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let Some((_, body)) = read_request(&mut stream).await else {
            continue;
        };
        let request: Value = serde_json::from_str(&body).unwrap_or_default();
        seen.lock()
            .assert_value()
            .push(request["refresh"].as_str().unwrap_or("<none>").to_owned());
        // Numbered independently of `seen`, which the test drains between turns.
        let count = issued.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let reply = json!({"access": format!("renewed-access-{count}"), "refresh": format!("renewed-refresh-{count}"), "lifetime": lifetime}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    }
}

/// Answers every model request with the node's expected response and records its
/// authorization header.
async fn model_server(listener: tokio::net::TcpListener, authorizations: Seen) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let authorizations = Arc::clone(&authorizations);
        tokio::spawn(async move {
            let Some((head, _)) = read_request(&mut stream).await else {
                return;
            };
            let authorization = head
                .lines()
                .find_map(|line| line.strip_prefix("authorization:"))
                .unwrap_or("<none>")
                .trim()
                .to_owned();
            authorizations.lock().assert_value().push(authorization);
            let payload = super::session_tools::sse(
                &[(
                    json!({"type":"text","text":""}),
                    json!({"type":"text_delta","text":"{\"response\":null}"}),
                )],
                "end_turn",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

async fn turn(adapter: &OpenCodeAdapter, invocation: DriverInvocation) {
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        adapter.run(invocation, control),
    )
    .await
    .assert_value();
    drain.await.assert_value();
    assert!(result.is_ok(), "{result:?}");
}

fn taken(seen: &Seen) -> Vec<String> {
    std::mem::take(&mut *seen.lock().assert_value())
}

#[tokio::test]
#[ignore = "requires installed OpenCode 2.4.0 or newer; loopback OAuth and provider only"]
async fn native_oauth_renews_once_in_the_user_profile_and_resumes_after_expiry() {
    let binary =
        std::env::var("ZEROSHOT_OPENCODE_BINARY").expect("set the explicit native OpenCode binary");
    let user = TestDirectory::new("opencode-native-oauth-user");
    let (token_listener, token_endpoint) = super::native_authority::native_listener().await;
    let refreshes: Seen = Arc::default();
    // The renewed login outlives OpenCode's five-minute renewal window by eight seconds.
    let tokens = tokio::spawn(token_server(
        token_listener,
        5 * 60_000 + 8_000,
        Arc::clone(&refreshes),
    ));
    let (model_listener, model_endpoint) = super::native_authority::native_listener().await;
    let authorizations: Seen = Arc::default();
    let models = tokio::spawn(model_server(model_listener, Arc::clone(&authorizations)));

    let plugin = user.child("stub-oauth");
    std::fs::create_dir_all(&plugin).assert_value();
    std::fs::write(plugin.join("package.json"), br#"{"type":"module"}"#).assert_value();
    let token_url = serde_json::to_string(&format!("{token_endpoint}/token")).assert_value();
    std::fs::write(
        plugin.join("index.js"),
        STUB_OAUTH.replace("TOKEN_URL", &token_url),
    )
    .assert_value();
    let plugin_url = url::Url::from_directory_path(&plugin)
        .assert_value()
        .to_string();
    let config = json!({"plugins":[plugin_url],"provider":{"anthropic":{"options":{"baseURL":model_endpoint},"models":{"opaque/model":{"id":"probe-model","name":"Probe","limit":{"context":100000,"output":4096}}}}}});
    let environment = super::native_authority::native_wire_environment(&user, config);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .assert_value()
        .as_millis() as u64;
    let seed = json!([{"id":"cred_stub","integrationID":"anthropic","label":"stub","active":true,"value":{"type":"oauth","methodID":"zs-stub","refresh":"user-refresh-0","access":"user-access-0","expires":now + 60_000,"metadata":{"accountID":"account-stub"}}}]);
    let seed_file = user.child("seed.json");
    std::fs::write(&seed_file, seed.to_string()).assert_value();
    let status = std::process::Command::new(&binary)
        .args(["auth", "import", "--standalone"])
        .arg(&seed_file)
        .envs(&environment)
        .current_dir(user.path())
        .status()
        .assert_value();
    assert!(status.success(), "seeding the user's profile failed");

    let node = |directory: &TestDirectory| {
        let mut node = adapter(directory, "#!/bin/sh\nexit 0\n");
        node.config.provider = "anthropic".to_owned();
        node.config.executable = binary.clone();
        node.local_environment = environment.clone();
        node
    };
    let first_directory = TestDirectory::new("opencode-native-oauth-first");
    let second_directory = TestDirectory::new("opencode-native-oauth-second");
    let first = node(&first_directory);
    let second = node(&second_directory);
    let first_session = invocation(&[]);
    let second_session = invocation(&[]);
    tokio::join!(
        turn(&first, first_session.clone()),
        turn(&second, second_session)
    );
    assert_eq!(taken(&refreshes), vec!["user-refresh-0".to_owned()]);
    let sent = taken(&authorizations);
    assert!(sent.len() >= 2, "{sent:?}");
    assert!(
        sent.iter()
            .all(|header| header == "bearer renewed-access-1"),
        "{sent:?}"
    );
    let exported = std::process::Command::new(&binary)
        .args(["auth", "export", "--standalone", "anthropic"])
        .envs(&environment)
        .current_dir(user.path())
        .output()
        .assert_value();
    let exported = String::from_utf8_lossy(&exported.stdout).into_owned();
    assert!(exported.contains("renewed-refresh-1"), "{exported}");

    // Nine seconds later the renewed login is inside the renewal window again.
    tokio::time::sleep(std::time::Duration::from_secs(9)).await;
    turn(&first, first_session).await;
    assert_eq!(taken(&refreshes), vec!["renewed-refresh-1".to_owned()]);
    let resumed = taken(&authorizations);
    assert!(!resumed.is_empty());
    assert!(
        resumed
            .iter()
            .all(|header| header == "bearer renewed-access-2"),
        "{resumed:?}"
    );
    tokens.abort();
    models.abort();
}
