use super::process::{adapter, invocation};
use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;

#[test]
fn a_declared_github_token_runs_the_guarded_copilot_model() {
    let directory = TestDirectory::new("opencode-github-token-declared");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "github".to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[("COPILOT_GITHUB_TOKEN", "declared-github-token")]);
    let command: ProcessSessionCommand = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    assert!(
        command
            .argv
            .iter()
            .any(|arg| arg == "github-copilot/zeroshot-guarded"),
        "{:?}",
        command.argv
    );
    let inline = &command.environment["OPENCODE_CONFIG_CONTENT"];
    let config: Value = serde_json::from_str(inline).assert_value();
    assert_eq!(
        config["provider"]["github-copilot"]["options"]["apiKey"],
        "{env:COPILOT_GITHUB_TOKEN}"
    );
    assert!(!inline.contains("declared-github-token"), "{inline}");
    assert_eq!(
        command.environment["COPILOT_GITHUB_TOKEN"],
        "declared-github-token"
    );
    owned_plugin(&command, "zeroshot-provider-auth");
}

#[test]
fn github_native_login_runs_without_an_owned_auth_plugin() {
    let directory = TestDirectory::new("opencode-github-native-login");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "github".to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[]);
    let command: ProcessSessionCommand = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    assert!(
        command
            .argv
            .iter()
            .any(|arg| arg == "github-copilot/opaque/model"),
        "{:?}",
        command.argv
    );
    let inline: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    assert!(inline.get("plugins").is_none());
    assert!(!command.environment.contains_key("COPILOT_GITHUB_TOKEN"));
}

#[tokio::test]
#[ignore = "requires installed OpenCode 2.4.0 or newer; uses localhost and a dummy token only"]
async fn native_declared_copilot_token_reaches_every_route_as_bearer() {
    for (model, path) in [
        ("claude-haiku-5.5", "/messages"),
        ("gpt-5.4-nano", "/responses"),
        ("gemini-3.8-flash", "/chat/completions"),
    ] {
        declared_copilot_route(model, path).await;
    }
}

async fn declared_copilot_route(model: &str, path: &str) {
    let directory = TestDirectory::new("opencode-native-copilot-token");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "github".to_owned();
    adapter.config.executable =
        std::env::var("ZEROSHOT_OPENCODE_BINARY").expect("explicit native binary");
    let (listener, endpoint) = super::native_authority::native_listener().await;
    let config = json!({"provider":{"github-copilot":{"options":{"baseURL":endpoint},"models":{"opaque/model":{"id":model}}}}});
    adapter.local_environment =
        super::native_authority::native_wire_environment(&directory, config);
    let invocation = invocation(&[("COPILOT_GITHUB_TOKEN", "declared-copilot-token")]);
    let (cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let execution = adapter.run(invocation, control);
    tokio::pin!(execution);
    let mut requests = Vec::new();
    let mut finished = false;
    loop {
        let wait = if requests.is_empty() { 40 } else { 5 };
        tokio::select! {
            accepted = tokio::time::timeout(std::time::Duration::from_secs(wait), listener.accept()) => {
                let Ok(accepted) = accepted else { break };
                let (stream, _) = accepted.assert_value();
                requests.push(super::native_authority::capture_stream(stream).await);
                if requests.len() >= 3 {
                    break;
                }
            }
            result = &mut execution => {
                assert!(!requests.is_empty(), "native github/{model} exited before HTTP: {result:?}");
                finished = true;
                break;
            }
        }
    }
    let _ = cancel.send(true);
    if !finished {
        let _result =
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut execution).await;
    }
    drain.await.assert_value();
    assert!(!requests.is_empty(), "no request for github/{model}");
    for request in &requests {
        let lower = request.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: bearer declared-copilot-token"),
            "{model}: {request}"
        );
        assert!(!lower.contains("x-api-key"), "{model}: {request}");
    }
    assert!(
        requests.iter().any(|request| {
            request
                .lines()
                .next()
                .is_some_and(|line| line.contains(path))
                && request.contains(&format!("\"model\":\"{model}\""))
        }),
        "{model}: {requests:?}"
    );
}
