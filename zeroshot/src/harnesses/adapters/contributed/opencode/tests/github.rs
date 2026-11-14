use super::process::{adapter, invocation, run};
use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;

#[test]
fn declared_github_token_is_refused_before_launch() {
    let directory = TestDirectory::new("opencode-github-token-refused");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "github".to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[("COPILOT_GITHUB_TOKEN", "declared-github-token")]);
    let error = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .err()
    .assert_value();
    let message = error.to_string();
    assert!(
        message.contains("cannot take a declared GitHub Copilot token"),
        "{message}"
    );
    assert!(!message.contains("declared-github-token"), "{message}");
}

#[tokio::test]
async fn declared_github_token_never_starts_opencode() {
    let directory = TestDirectory::new("opencode-github-token-never-starts");
    let mut adapter = adapter(
        &directory,
        "#!/bin/sh\nprintf 'launch\\n' >> launches\nexit 0\n",
    );
    adapter.config.provider = "github".to_owned();
    let invocation = invocation(&[("COPILOT_GITHUB_TOKEN", "declared-github-token")]);
    assert!(run(&adapter, invocation).await.is_err());
    assert!(!adapter.config.workspace.join("launches").exists());
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
