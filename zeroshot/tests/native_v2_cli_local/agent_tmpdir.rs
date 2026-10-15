//! Project tests that create Unix sockets under the agent's `TMPDIR`.

use std::os::unix::net::{UnixListener, UnixStream};

use super::*;

/// Go's `t.TempDir()` layout from the report: `$TMPDIR/<test name><random>/001`.
const PROJECT_TEMP_DIR: &str = "TestListenOnUnixSocketFreshPath371951039/001";
const PROJECT_SOCKET: &str = "sidecar-ipc";
const PROJECT_TEST: &str = "project_test_listens_on_unix_socket_in_temp_dir";
const PROJECT_TEST_LOG: &str = "project-socket-test.log";

#[tokio::test(flavor = "multi_thread")]
async fn codex_agent_project_test_binds_unix_socket_in_tmpdir() {
    exercise_project_socket_test("codex").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_agent_project_test_binds_unix_socket_in_tmpdir() {
    exercise_project_socket_test("claude").await;
}

/// The project test. The stub harness runs it as a child of the agent, like `go test`.
#[test]
#[ignore = "run inside a Zeroshot agent by the agent TMPDIR socket tests"]
fn project_test_listens_on_unix_socket_in_temp_dir() {
    let directory = std::env::temp_dir().join(PROJECT_TEMP_DIR);
    fs::create_dir_all(&directory).assert_value_with("create project test temp directory");
    let socket = directory.join(PROJECT_SOCKET);
    let listener = UnixListener::bind(&socket).unwrap_or_else(|error| {
        panic!(
            "bind {} ({} bytes): {error}",
            socket.display(),
            socket.as_os_str().len()
        )
    });
    let _client = UnixStream::connect(&socket).assert_value_with("connect project socket");
    listener
        .accept()
        .assert_value_with("accept project socket connection");
}

async fn exercise_project_socket_test(harness: &str) {
    let mut fixture = LocalFixture::new();
    // The default local state root, `$HOME/.local/state/zeroshot`, under an ordinary home.
    let home = fixture.root.path("home");
    fs::create_dir_all(&home).assert_value_with("create user home");
    fixture
        .harness_environment
        .insert("HOME".to_owned(), home.display().to_string());
    fixture.state = home.join(".local").join("state").join("zeroshot");
    let mut runtime = local_runtime();
    if harness == "claude" {
        runtime["harness"] = json!("claude");
        runtime["provider"] = json!("anthropic");
        runtime["nodes"]["worker"]["model"] = json!("proxy-model");
        runtime["nodes"]["worker"]["connections"] = json!({"anthropic":["ANTHROPIC_API_KEY"]});
        fixture
            .harness_environment
            .insert("ANTHROPIC_API_KEY".to_owned(), DECLARED_KEY.to_owned());
    }
    write_json(&fixture.runtime, &runtime);
    openengine_cluster_testkit::fixture::write_executable(
        &fixture.root.path("bin").join(harness),
        with_configuration_probe(
            project_test_harness(harness).as_bytes(),
            harness,
            &ProbeResponse::Configured(json!({})),
        ),
        0o755,
    )
    .assert_value_with("write stub harness");

    let output = fixture
        .run("Agent TMPDIR socket project test", "finish", false)
        .await;
    assert_success(&output, "local run with a socket project test");
    let log = fs::read_to_string(fixture.repository.join(PROJECT_TEST_LOG))
        .assert_value_with("project socket test log");
    assert!(
        log.contains("test result: ok. 1 passed") && log.ends_with("exit=0\n"),
        "a project test inside the {harness} agent could not bind a Unix socket under its \
         TMPDIR:\n{log}"
    );
}

fn project_test_harness(harness: &str) -> String {
    let executable = std::env::current_exe().assert_value_with("test executable");
    let executable = executable
        .to_str()
        .assert_value_with("test executable path");
    let test = format!(
        "{}::{PROJECT_TEST}",
        module_path!()
            .split_once("::")
            .assert_value_with("test module path")
            .1
    );
    let run_project_test = format!(
        "{} --exact {} --ignored --test-threads=1",
        shell_literal(executable),
        shell_literal(&test)
    );
    let answer = match harness {
        "codex" => CODEX_ANSWER,
        "claude" => CLAUDE_ANSWER,
        _ => unreachable!(),
    };
    format!(
        r#"#!/bin/sh
set -eu
cat >/dev/null
printf 'TMPDIR=%s\n' "${{TMPDIR-}}" >> {PROJECT_TEST_LOG}
if {run_project_test} >> {PROJECT_TEST_LOG} 2>&1; then
  printf 'exit=0\n' >> {PROJECT_TEST_LOG}
else
  printf 'exit=%s\n' "$?" >> {PROJECT_TEST_LOG}
fi
{answer}"#
    )
}

const CODEX_ANSWER: &str = r#"printf '%s\n' '{"type":"thread.started","thread_id":"local-thread"}'
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":null}"}}'
printf '%s\n' '{"type":"turn.completed"}'
"#;

const CLAUDE_ANSWER: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"local-session"}'
printf '%s%s\n' '{"type":"result","subtype":"success","is_error":false,' \
  '"result":"{\"response\":null}","session_id":"local-session"}'
"#;
