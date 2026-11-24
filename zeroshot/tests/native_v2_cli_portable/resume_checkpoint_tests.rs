//! Failed CLI recovery selects the latest entry, while suspension retains its held history.

use super::{Fixture, success};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn review_loop() -> Fixture {
    let fixture = Fixture::new().outside_workspace();
    let review = json!({
        "kind":"verifier", "name":"review", "worker":"agent.review@1",
        "instructions":"Review this round.", "input":{"kind":"null"}, "output":{"kind":"null"},
        "signals":{"done":["yes","no"]}, "diagnostic":{"kind":"null"},
        "inputBindings":[], "writeBindings":[], "attempts":1
    });
    let done = json!({"kind":"in","value":{"source":"signal","name":"review","field":"done"},"labels":["yes"]});
    fixture.write("graph.json", &json!({
        "profile":"openengine.graph.full/v1", "initialInput":{"kind":"null"},
        "policy":{"policy":"policy.native-v2@1","default":"deny"},
        "root":{"kind":"seq","name":"root","state":{"kind":"null"},"promotedStatePaths":[],"children":[
            {"kind":"loop","name":"reviews","state":{"kind":"null"},"maxIterations":3,
             "until":done,"body":review,"promotedStatePaths":[]},
            {"kind":"choice","name":"result","state":{"kind":"null"},"promotedStatePaths":[],
             "branches":[{"when":done,"node":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}}],
             "otherwise":{"kind":"fail","name":"failed","reason":"reviews_failed"}}
        ]}
    }));
    fixture.write("runtime.json", &json!({
        "harness":"codex","provider":"openai","size":"host",
        "activeMemory":{"repository":false,"graph":false},"globalAgents":{"postmortem":{"enabled":false}},
        "nodes":{"review":{"kind":"agent","model":"fixture-owned-model",
            "connections":{"openai":["OPENAI_API_KEY","FIXTURE_MODE","REVIEW_RECEIPTS"]}}}
    }));
    fixture
}

fn wait_finished(fixture: &Fixture, id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let status = fixture.json(&["status", id]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(
                status["status"]["terminalResult"]["status"], expected,
                "{status}"
            );
            return;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn failed_three_round_loop_defaults_to_latest_entry_and_retries_only_round_three() {
    let fixture = review_loop();
    let receipts = fixture.root.path("reviews.jsonl");
    let started = fixture
        .run_command("review-fail", true)
        .env("REVIEW_RECEIPTS", &receipts)
        .output()
        .unwrap();
    success(&started);
    let started: Value = serde_json::from_slice(&started.stdout).unwrap();
    let id = started["runId"].as_str().unwrap();
    wait_finished(&fixture, id, "failed");
    assert_eq!(fs::read_to_string(&receipts).unwrap().lines().count(), 3);
    let points = fixture.json(&["checkpoints", id]);
    let latest = points["checkpoints"].as_array().unwrap().last().unwrap();
    assert_eq!(latest["node"], "review");
    assert_eq!(latest["loopIterations"], json!([3]));
    let selected = latest["checkpointId"].as_str().unwrap();
    fs::write(fixture.workspace.join("later-edit"), "keep until consent").unwrap();
    let refused = fixture
        .command(&["resume", id])
        .env("FIXTURE_MODE", "review-finish")
        .env("REVIEW_RECEIPTS", &receipts)
        .output()
        .unwrap();
    assert!(
        !refused.status.success(),
        "plain recovery must require local restore consent"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains(selected) && stderr.contains("--discard-workspace-changes"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("later-edit")).unwrap(),
        "keep until consent"
    );
    assert!(
        fixture.json(&["status", id])["workspaceRecovery"]
            .get("successorRunId")
            .is_none()
    );
    let structured = fixture
        .command(&["resume", id])
        .env("ZEROSHOT_ERROR_FORMAT", "json")
        .output()
        .unwrap();
    assert!(!structured.status.success());
    let diagnostic: Value = serde_json::from_slice(&structured.stderr).unwrap();
    assert_eq!(diagnostic["kind"], "invalid_request");
    assert!(diagnostic["message"].as_str().unwrap().contains(selected));
    let resumed = fixture
        .command(&["resume", id, "--discard-workspace-changes"])
        .env("FIXTURE_MODE", "review-finish")
        .env("REVIEW_RECEIPTS", &receipts)
        .output()
        .unwrap();
    success(&resumed);
    assert!(String::from_utf8_lossy(&resumed.stderr).contains(selected));
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    let successor = resumed["runId"].as_str().unwrap();
    wait_finished(&fixture, successor, "succeeded");
    assert!(!fixture.workspace.join("later-edit").exists());
    assert_eq!(fs::read_to_string(receipts).unwrap().lines().count(), 4);
    let ledger = rusqlite::Connection::open_with_flags(
        fixture
            .root
            .path(&format!("state/runs/{successor}/runs.sqlite3")),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let events: Vec<Value> = ledger
        .prepare("SELECT event_json FROM v2_run_events ORDER BY sequence")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str::<Value>(&row.unwrap()).unwrap())
        .collect();
    let starts: Vec<_> = events
        .iter()
        .filter(|event| event["kind"] == "node_started")
        .collect();
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["kind"] == "prior_execution")
            .count(),
        2,
        "{events:?}"
    );
}

#[test]
fn checkpointless_failed_resume_explains_explicit_restart_and_keeps_later_work() {
    let fixture = Fixture::new().outside_workspace();
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["workspaceCheckpoints"] = json!(false);
    fixture.write("runtime.json", &runtime);
    let started = fixture.run("fail", true);
    success(&started);
    let started: Value = serde_json::from_slice(&started.stdout).unwrap();
    let id = started["runId"].as_str().unwrap();
    wait_finished(&fixture, id, "failed");
    fs::write(fixture.workspace.join("later-edit"), "keep").unwrap();
    let refused = fixture.command(&["resume", id]).output().unwrap();
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("no entry checkpoints")
            && stderr.contains(&format!("zeroshot resume {id} --restart")),
        "{stderr}"
    );
    let resumed = fixture
        .command(&["resume", id, "--restart"])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    wait_finished(&fixture, resumed["runId"].as_str().unwrap(), "succeeded");
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("later-edit")).unwrap(),
        "keep"
    );
}

#[test]
fn suspended_resume_keeps_completed_history_and_current_checkout_without_consent() {
    assert_suspended_resume(false, false);
}

#[test]
fn python_suspended_resume_keeps_completed_history_and_current_checkout_without_consent() {
    assert_suspended_resume(true, false);
}

#[test]
fn python_suspended_resume_recovers_an_interrupted_ledger_without_a_temporary_image() {
    assert_suspended_resume(true, true);
}

fn assert_suspended_resume(sdk: bool, interrupted: bool) {
    let fixture = Fixture::new().outside_workspace();
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    let first = graph["root"]["children"][0].clone();
    let mut second = first.clone();
    second["name"] = json!("second");
    graph["root"]["children"] = json!([first, second, {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]);
    fixture.write("graph.json", &graph);
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["nodes"]["worker"]["connections"]["openai"] =
        json!(["OPENAI_API_KEY", "FIXTURE_MODE", "FIXTURE_RECEIPTS"]);
    runtime["nodes"]["second"] = runtime["nodes"]["worker"].clone();
    fixture.write("runtime.json", &runtime);
    let receipts = fixture.root.path("calls.txt");
    let started = fixture
        .run_command("suspend-complete", true)
        .env("FIXTURE_RECEIPTS", &receipts)
        .output()
        .unwrap();
    success(&started);
    let started: Value = serde_json::from_slice(&started.stdout).unwrap();
    let id = started["runId"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while !fixture.workspace.join("suspend-ready").exists() {
        assert!(Instant::now() < deadline, "worker never became ready");
        std::thread::sleep(Duration::from_millis(30));
    }
    fixture.json(&["suspend", id]);
    fs::write(fixture.workspace.join("release"), "release").unwrap();
    loop {
        let status = fixture.json(&["status", id]);
        if status["status"]["phase"] == "suspended" {
            assert_eq!(status["status"]["node"], "second");
            break;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(30));
    }
    fs::write(fixture.workspace.join("later-edit"), "preserve").unwrap();
    let mut resume = if sdk {
        sdk_resume(&fixture, id)
    } else {
        fixture.command(&["resume", id])
    };
    if interrupted {
        crate::inspection::wait_released(&fixture.root.path(&format!("state/runs/{id}")));
        interrupt_ledger_without_temporary_image(&fixture, id, &mut resume);
    }
    let resumed = resume
        .env("FIXTURE_MODE", "finish")
        .env("FIXTURE_RECEIPTS", &receipts)
        .output()
        .unwrap();
    success(&resumed);
    assert!(!String::from_utf8_lossy(&resumed.stderr).contains("Selected checkpoint"));
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    wait_finished(&fixture, resumed["runId"].as_str().unwrap(), "succeeded");
    assert_eq!(fs::read_to_string(receipts).unwrap().lines().count(), 2);
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("later-edit")).unwrap(),
        "preserve"
    );
}

#[test]
fn python_resume_restarts_a_lost_controller_on_its_first_call_with_or_without_checkpoints() {
    for checkpoints in [true, false] {
        assert_python_resume_after_controller_loss(checkpoints, false);
    }
}

#[test]
fn python_resume_recovers_an_interrupted_ledger_without_a_temporary_image() {
    assert_python_resume_after_controller_loss(false, true);
}

fn assert_python_resume_after_controller_loss(checkpoints: bool, interrupted: bool) {
    let fixture = Fixture::new().outside_workspace();
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["workspaceCheckpoints"] = json!(checkpoints);
    fixture.write("runtime.json", &runtime);
    let started = fixture.run("block-quiet", true);
    success(&started);
    let started: Value = serde_json::from_slice(&started.stdout).unwrap();
    let id = started["runId"].as_str().unwrap();
    fixture.await_child();
    let points = fixture.json(&["checkpoints", id]);
    assert_eq!(
        !points["checkpoints"].as_array().unwrap().is_empty(),
        checkpoints
    );
    let ready: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .path(&format!("state/runs/{id}/controller.ready.json")),
        )
        .unwrap(),
    )
    .unwrap();
    crate::inspection::kill_controller(u32::try_from(ready["pid"].as_u64().unwrap()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = fixture.json(&["status", id]);
        if status["controller"]["state"] == "lost" {
            // Status must not reconcile the durable phase before the SDK chooses its selector.
            assert_eq!(status["status"]["phase"], "running", "{status}");
            break;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(30));
    }
    fs::write(fixture.workspace.join("later-edit"), "keep").unwrap();
    let storage = fixture.root.path(&format!("state/runs/{id}"));
    let mut resume = sdk_resume(&fixture, id);
    if interrupted {
        interrupt_ledger_without_temporary_image(&fixture, id, &mut resume);
    }
    let resumed = resume.env("FIXTURE_MODE", "finish").output().unwrap();
    success(&resumed);
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    let successor = resumed["runId"].as_str().unwrap();
    assert_ne!(successor, id);
    wait_finished(&fixture, successor, "succeeded");
    assert_eq!(
        fixture.json(&["status", id])["status"]["terminalResult"]["reason"],
        "runtime_lost"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("later-edit")).unwrap(),
        "keep"
    );
    assert!(!storage.join("runs.sqlite3-journal").exists());
}

fn interrupt_ledger_without_temporary_image(fixture: &Fixture, id: &str, resume: &mut Command) {
    let storage = fixture.root.path(&format!("state/runs/{id}"));
    crate::inspection::sqlite_crash::interrupt(&storage.join("runs.sqlite3"));
    let missing = fixture.root.path("missing-temporary-directory");
    let mut status = fixture.command(&["status", id]);
    for variable in ["TMPDIR", "TMP", "TEMP"] {
        resume.env(variable, &missing);
        status.env(variable, &missing);
    }
    let refused = status.output().unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("run ledger storage is unavailable"),
        "{refused:?}"
    );
    assert!(storage.join("runs.sqlite3-journal").is_file());
}

fn sdk_resume(fixture: &Fixture, id: &str) -> Command {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let python = root.join(if cfg!(windows) {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python"
    });
    let python = if python.is_file() {
        python
    } else {
        PathBuf::from(if cfg!(windows) { "python" } else { "python3" })
    };
    let environment = fixture.command(&[]);
    let mut command = Command::new(python);
    command
        .env_clear()
        .envs(
            environment
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .env("PYTHONPATH", root.join("sdks/python/src"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env(
            "ZEROSHOT_PYTHON_NATIVE_BINARY",
            env!("CARGO_BIN_EXE_zeroshot"),
        )
        .current_dir(&fixture.workspace)
        .args([
            "-c",
            r#"import asyncio, json, sys
from zeroshot import Client, LocalTarget
async def main():
    async with Client(target=LocalTarget(state_dir=sys.argv[1])) as client:
        successor = await client.get_run(sys.argv[2]).resume()
        print(json.dumps({"runId": successor.id}))
asyncio.run(main())
"#,
        ])
        .arg(fixture.root.path("state"))
        .arg(id);
    command
}
