#![cfg(unix)]

//! `zeroshot watch` reports the result of the run it follows through its exit status.
//!
//! Scripts and CI jobs start a run with `zeroshot run --detach` and wait for it with
//! `zeroshot watch RUN_ID`, or resume that wait with `--after CURSOR`. A failed foreground
//! `zeroshot run` exits unsuccessfully and prints a failure summary on stderr. `watch` must do the
//! same when the run it follows ended `failed`, and exit successfully when that run `succeeded`.

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use openengine_cluster_testkit::assertions::{AssertAt, AssertValue, JsonAt};
use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;

#[allow(
    dead_code,
    reason = "this target uses only the shared fixture's run submission surface"
)]
#[path = "native_v2_cli_local/fixture.rs"]
mod fixture;
use fixture::*;

const CLI_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread")]
async fn watch_of_a_failed_run_exits_and_reports_like_the_failed_foreground_run() {
    let fixture = LocalFixture::new();
    let run = fixture.run("Failed foreground run", "fail", false).await;
    let summary = String::from_utf8_lossy(&run.stderr).trim().to_owned();
    assert!(
        !run.status.success() && summary.contains("worker_failed"),
        "precondition: the failed foreground run must exit unsuccessfully with its summary\n{}",
        describe(&run)
    );
    let run_id = json_lines(&run.stdout)
        .as_slice()
        .assert_at(0)
        .assert_key("runId")
        .as_str()
        .assert_value_with("run receipt identity")
        .to_owned();

    let watch = run_watch(&fixture, &run_id, None).await;

    assert_eq!(
        last_terminal_result(&watch).assert_key("status"),
        "failed",
        "precondition: watch must replay the failed terminal snapshot\n{}",
        describe(&watch)
    );
    assert_eq!(
        watch.status.code(),
        run.status.code(),
        "`zeroshot watch` of a failed run must exit like the failed foreground run\n\
         --- zeroshot run ---\n{}\n--- zeroshot watch ---\n{}",
        describe(&run),
        describe(&watch)
    );
    assert!(
        String::from_utf8_lossy(&watch.stderr).contains(&summary),
        "`zeroshot watch` of a failed run must print the foreground run's failure summary\n\
         --- expected summary ---\n{summary}\n--- zeroshot watch ---\n{}",
        describe(&watch)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_of_a_detached_run_that_fails_exits_unsuccessfully_and_names_the_failure() {
    let fixture = LocalFixture::new();
    let run_id = fixture.submit_detached("fail").await;

    let watch = run_watch(&fixture, &run_id, None).await;

    let terminal = last_terminal_result(&watch);
    assert_eq!(
        terminal.assert_key("status"),
        "failed",
        "precondition: watch must follow the injected failure to its terminal snapshot\n{}",
        describe(&watch)
    );
    assert_eq!(terminal.assert_key("reason"), "worker_failed");
    assert_failed_watch(&watch, "`zeroshot watch` of a failed run");

    // A script resuming its wait from the last cursor it consumed gets no further events, but
    // the run it follows still failed.
    let resumed = run_watch(&fixture, &run_id, Some(&last_cursor(&watch))).await;
    assert_failed_watch(
        &resumed,
        "`zeroshot watch --after` the failed terminal snapshot",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_of_a_detached_run_that_succeeds_exits_successfully() {
    let fixture = LocalFixture::new();
    let run_id = fixture.submit_detached("finish").await;

    let watch = run_watch(&fixture, &run_id, None).await;

    assert_eq!(
        last_terminal_result(&watch).assert_key("status"),
        "succeeded",
        "precondition: watch must follow the run to its successful terminal snapshot\n{}",
        describe(&watch)
    );
    assert_success(&watch, "watch of a succeeded run");
    let resumed = run_watch(&fixture, &run_id, Some(&last_cursor(&watch))).await;
    assert_success(&resumed, "watch --after the succeeded terminal snapshot");
}

fn assert_failed_watch(watch: &Output, context: &str) {
    assert!(
        !watch.status.success(),
        "{context} exited successfully although the run it followed failed\n{}",
        describe(watch)
    );
    assert!(
        String::from_utf8_lossy(&watch.stderr).contains("worker_failed"),
        "{context} must name the run's failure on stderr\n{}",
        describe(watch)
    );
}

/// Runs `zeroshot watch RUN_ID [--after CURSOR]` against the fixture's local state until it exits.
async fn run_watch(fixture: &LocalFixture, run_id: &str, after: Option<&str>) -> Output {
    let storage = fixture.run_storage(run_id);
    let state = storage
        .parent()
        .and_then(Path::parent)
        .assert_value_with("local state root");
    let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
    command.args(["watch", run_id]);
    if let Some(cursor) = after {
        command.args(["--after", cursor]);
    }
    command
        .current_dir(&fixture.repository)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("ZEROSHOT_STATE_DIR", state)
        .env("ZEROSHOT_CONFIG_DIR", &fixture.config_blocker)
        .kill_on_drop(true);
    timeout(CLI_TIMEOUT, command.output())
        .await
        .assert_value_with("local watch deadline")
        .assert_value_with("start local watch")
}

fn last_event(watch: &Output) -> Value {
    json_lines(&watch.stdout)
        .last()
        .assert_value_with("watch emitted no status events")
        .clone()
}

fn last_terminal_result(watch: &Output) -> Value {
    last_event(watch)
        .assert_key("status")
        .assert_key("terminalResult")
        .clone()
}

fn last_cursor(watch: &Output) -> String {
    last_event(watch)
        .assert_key("cursor")
        .as_str()
        .assert_value_with("watch event cursor")
        .to_owned()
}

fn describe(output: &Output) -> String {
    format!(
        "exit: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
