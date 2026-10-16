#![cfg(unix)]

//! A failed foreground `zeroshot run` must tell the user why the run failed.
//!
//! The foreground run is the surface a user watches. When the run fails, its output must carry the
//! actual cause the product already knows: the provider error that failed a worker, or the runtime
//! error that failed the run before any node started. The bare terminal reason and "run finished
//! unsuccessfully" are not enough. Each test first proves that the durable `zeroshot logs RUN_ID`
//! replay holds the cause, then requires the foreground `zeroshot run` output to name it.

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

/// The error a Codex turn reports in `turn.failed`. It holds no declared connection value, so log
/// redaction leaves it intact.
const PROVIDER_ERROR: &str =
    "model gpt-5.6 is not supported when using Codex with a ChatGPT account";
/// A leftover git-ignored socket that the first workspace checkpoint cannot capture.
const SOCKET: &str = "dev-server.sock";
const CLI_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread")]
async fn foreground_run_with_a_failed_worker_names_the_provider_error() {
    let fixture = LocalFixture::new();
    install_failing_codex_turn(&fixture);

    let run = fixture
        .run("Worker failure diagnostics", "finish", false)
        .await;

    assert_foreground_run_names_cause(&fixture, &run, Some("worker_failed"), PROVIDER_ERROR).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn foreground_run_failing_its_first_checkpoint_names_the_runtime_error() {
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

    assert_foreground_run_names_cause(&fixture, &run, None, SOCKET).await;
}

/// Keeps the fixture's fake Codex for the configuration probe and makes every model turn fail with
/// [`PROVIDER_ERROR`], the way Codex reports a rejected turn.
fn install_failing_codex_turn(fixture: &LocalFixture) {
    let bin = fixture
        .repository
        .parent()
        .assert_value_with("fixture root")
        .join("bin");
    let probe = bin.join("codex-configuration-probe");
    fs::rename(bin.join("codex"), &probe).assert_value_with("keep the fixture's fake Codex");
    let probe = probe.to_str().assert_value_with("fake Codex path");
    assert!(
        !probe.contains('\''),
        "fake Codex path must not need shell quoting"
    );
    let failed = serde_json::json!({"type":"turn.failed","error":{"message":PROVIDER_ERROR}});
    openengine_cluster_testkit::fixture::write_executable(
        &bin.join("codex"),
        format!(
            r#"#!/bin/sh
if test "${{1-}}" = app-server; then
  exec '{probe}' "$@"
fi
cat >/dev/null
printf '%s\n' '{{"type":"thread.started","thread_id":"local-thread"}}'
printf '%s\n' '{{"type":"turn.started"}}'
printf '%s\n' '{failed}'
exit 1
"#
        ),
        0o755,
    )
    .assert_value_with("write failing fake Codex");
}

async fn assert_foreground_run_names_cause(
    fixture: &LocalFixture,
    run: &Output,
    reason: Option<&str>,
    cause: &str,
) {
    let run_id = json_lines(&run.stdout)
        .as_slice()
        .assert_at(0)
        .assert_key("runId")
        .as_str()
        .assert_value_with("run receipt identity")
        .to_owned();
    let status = finished_status(fixture, &run_id).await;
    let terminal = status.assert_key("status").assert_key("terminalResult");
    assert_eq!(
        terminal.assert_key("status"),
        "failed",
        "the injected fault must fail the run: {status}"
    );
    if let Some(reason) = reason {
        assert_eq!(
            terminal.assert_key("reason"),
            reason,
            "unexpected terminal reason: {status}"
        );
    }

    let logs = replay_logs(fixture, &run_id).await;
    assert_success(&logs, "local logs replay");
    let logs = String::from_utf8_lossy(&logs.stdout);
    assert!(
        logs.contains(cause),
        "precondition: the durable run log must already hold the cause {cause:?}\n\
         --- final status ---\n{status}\n--- zeroshot logs stdout ---\n{logs}"
    );

    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stdout.contains(cause) || stderr.contains(cause),
        "the foreground run failed without its cause: neither stdout nor stderr of `zeroshot run` \
         names {cause:?}, although `zeroshot logs {run_id}` does\n\
         --- zeroshot run exit ---\n{}\n--- zeroshot run stdout ---\n{stdout}\n\
         --- zeroshot run stderr ---\n{stderr}\n--- final status ---\n{status}",
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
