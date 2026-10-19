#![cfg(unix)]

//! A local run that fails before any node starts must tell the user why through the CLI.
//!
//! The first workspace checkpoint cannot capture a leftover Unix socket, so the supervisor fails the
//! run before dispatch: usually `runtime_failed`, occasionally `runtime_lost` when controller exit
//! wins the race. The user's surfaces are the foreground `zeroshot run` output and the durable
//! `zeroshot logs RUN_ID` replay, which the UI history shares. Status, foreground output and replay
//! must name the workspace entry that stopped the run instead of only the bare terminal reason.

use std::fs;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

use openengine_cluster_testkit::assertions::{AssertAt, AssertValue, JsonAt};
use serde_json::Value;
use tokio::process::Command;
use tokio::time::{Instant, sleep, timeout};

#[allow(
    dead_code,
    reason = "this target uses only the shared fixture's foreground run surface"
)]
#[path = "native_v2_cli_local/fixture.rs"]
mod fixture;
use fixture::*;

const SOCKET: &str = "dev-server.sock";
const CLI_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread")]
async fn local_run_failing_its_first_checkpoint_preserves_the_cause_across_cli_surfaces() {
    let fixture = LocalFixture::new();
    fs::write(fixture.repository.join(".gitignore"), format!("{SOCKET}\n"))
        .assert_value_with("write ignore rules");
    git(&fixture.repository, &["add", ".gitignore"]);
    git(
        &fixture.repository,
        &["commit", "-m", "ignore the development server socket"],
    );
    let _socket = UnixListener::bind(fixture.repository.join(SOCKET))
        .assert_value_with("bind leftover development server socket");

    let run = fixture
        .run("Runtime failure diagnostics", "finish", false)
        .await;
    let run_id = json_lines(&run.stdout)
        .as_slice()
        .assert_at(0)
        .assert_key("runId")
        .as_str()
        .assert_value_with("run receipt identity")
        .to_owned();
    let status = finished_status(&fixture, &run_id).await;
    assert_eq!(
        status
            .assert_key("status")
            .assert_key("terminalResult")
            .assert_key("status"),
        "failed",
        "a workspace with a socket must fail its first checkpoint: {status}"
    );
    let context = status
        .assert_key("status")
        .assert_key("terminalResult")
        .assert_key("context");
    assert!(
        context.to_string().contains(SOCKET),
        "status lost the checkpoint failure detail: {status}"
    );
    let foreground = format!(
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        foreground.contains(SOCKET),
        "foreground output lost the checkpoint failure detail: {foreground}"
    );

    let logs = replay_logs(&fixture, &run_id).await;
    assert_success(&logs, "local logs replay");
    assert!(
        String::from_utf8_lossy(&logs.stdout).contains(SOCKET),
        "durable logs lost the checkpoint failure detail: {logs:?}"
    );
    let surfaces = [
        ("zeroshot run stdout", &run.stdout),
        ("zeroshot run stderr", &run.stderr),
        ("zeroshot logs stdout", &logs.stdout),
        ("zeroshot logs stderr", &logs.stderr),
    ]
    .map(|(name, bytes)| format!("--- {name} ---\n{}", String::from_utf8_lossy(bytes)))
    .join("\n");
    assert!(
        surfaces.contains(SOCKET),
        "the run failed without a cause: no CLI surface names {SOCKET:?}, the workspace entry \
         the checkpoint could not capture\n--- zeroshot run exit ---\n{}\n--- final status ---\n\
         {status}\n{surfaces}",
        run.status
    );
}

async fn finished_status(fixture: &LocalFixture, run_id: &str) -> Value {
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        let status = fixture.json(&["status", run_id], "finish").await;
        if status.assert_key("status").assert_key("phase") == "finished" {
            return status;
        }
        assert!(Instant::now() < deadline, "run did not finish: {status}");
        sleep(Duration::from_millis(30)).await;
    }
}

async fn replay_logs(fixture: &LocalFixture, run_id: &str) -> Output {
    let storage = fixture.run_storage(run_id);
    let state = storage
        .parent()
        .and_then(Path::parent)
        .assert_value_with("local state root");
    let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
    command
        .args(["logs", run_id])
        .current_dir(&fixture.repository)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("ZEROSHOT_STATE_DIR", state)
        .env("ZEROSHOT_CONFIG_DIR", &fixture.config_blocker)
        .kill_on_drop(true);
    timeout(CLI_TIMEOUT, command.output())
        .await
        .assert_value_with("local logs deadline")
        .assert_value_with("start local logs")
}
