//! Failed fresh submissions remove only storage that was never admitted.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::process::Stdio;

use serde_json::{Value, json};

use super::fixture::{Fixture, success};

fn run_directories(fixture: &Fixture) -> BTreeSet<OsString> {
    std::fs::read_dir(fixture.root.path("state/runs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

#[test]
fn busy_workspace_refusal_removes_only_the_new_run_directory() {
    let fixture = Fixture::new();
    let (id, _) = fixture.start_blocked();
    let legacy = fixture
        .root
        .path("state/runs/0199f33f-3b44-7d21-9000-000000000001");
    std::fs::create_dir(&legacy).unwrap();
    std::fs::write(legacy.join("retained"), b"older unadmitted run").unwrap();
    let before = run_directories(&fixture);

    let refused = fixture.run("finish", true);
    let after = run_directories(&fixture);
    // Retire the running fixture before any regression assertion can fail.
    success(&fixture.command(&["force-stop", &id]).output().unwrap());

    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains(
            "controller exited before becoming ready: another controller owns this lease"
        ),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(after, before);
    assert_eq!(
        std::fs::read(legacy.join("retained")).unwrap(),
        b"older unadmitted run"
    );
}

#[test]
fn oversized_bootstrap_and_admission_refusals_remove_the_run_directory() {
    let fixture = Fixture::new();
    let mut leftovers = Vec::new();
    for megabytes in [3, 5] {
        let mut command = fixture.run_command("volume", true);
        fixture.write(
            "input.json",
            &json!({"blob": "x".repeat(megabytes * 1024 * 1024)}),
        );
        let refused = command.output().unwrap();
        assert!(!refused.status.success());
        let expected = if megabytes == 3 {
            "controller exited before becoming ready: admitted run exceeds the storage bound"
        } else {
            "portable controller bootstrap is malformed or too large"
        };
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        leftovers.push((megabytes, run_directories(&fixture).len()));
    }
    assert_eq!(leftovers, [(3, 0), (5, 0)]);
}

#[test]
fn controller_startup_failure_keeps_its_recorded_reason_without_storage() {
    let fixture = Fixture::new();
    let refused = fixture
        .run_command("finish", true)
        .env("ZEROSHOT_RESTIC", fixture.root.path("missing-restic"))
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let error = String::from_utf8_lossy(&refused.stderr);
    assert!(
        error.contains(
            "controller exited before becoming ready: portable controller I/O failed: \
             ZEROSHOT_RESTIC does not name a restic executable:"
        ),
        "{error}"
    );
    assert!(run_directories(&fixture).is_empty());
}

#[test]
fn admitted_run_failure_keeps_the_run_directory() {
    let fixture = Fixture::new();
    let failed = fixture.run("fail", false);
    assert!(!failed.status.success());
    let receipt: Value =
        serde_json::from_slice(failed.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
    let id = receipt["runId"].as_str().unwrap();
    assert!(
        fixture
            .root
            .path(&format!("state/runs/{id}/runs.sqlite3"))
            .is_file()
    );
    assert_eq!(
        fixture.json(&["status", id])["status"]["terminalResult"]["status"],
        "failed"
    );
}

#[test]
fn interrupted_cli_keeps_an_admitted_run_directory() {
    let fixture = Fixture::new();
    let mut cli = fixture
        .run_command("block", false)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut receipt = String::new();
    let mut stdout = BufReader::new(cli.stdout.take().unwrap());
    stdout.read_line(&mut receipt).unwrap();
    let receipt: Value = serde_json::from_str(&receipt).unwrap();
    let id = receipt["runId"].as_str().unwrap();
    fixture.await_child();
    cli.kill().unwrap();
    cli.wait().unwrap();
    let retained = fixture
        .root
        .path(&format!("state/runs/{id}/runs.sqlite3"))
        .is_file();
    let status = fixture.json(&["status", id]);
    success(&fixture.command(&["force-stop", id]).output().unwrap());

    assert!(retained);
    assert_eq!(status["status"]["phase"], "running");
}
