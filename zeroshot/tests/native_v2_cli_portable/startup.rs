//! Fresh and resumed controller startup preserves admitted run state.

#[path = "resume_checkpoint_tests.rs"]
mod resume_checkpoint;
#[path = "resume_runtime_tests.rs"]
mod resume_runtime;
#[path = "resume_runtime_refusal_tests.rs"]
mod resume_runtime_refusal;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::fixture::{Fixture, success};

#[path = "runtime_sizes.rs"]
mod runtime_sizes;

fn run_directories(fixture: &Fixture) -> BTreeSet<OsString> {
    let entries = match std::fs::read_dir(fixture.root.path("state/runs")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return BTreeSet::new(),
        Err(error) => panic!("cannot inspect run storage: {error}"),
    };
    entries.map(|entry| entry.unwrap().file_name()).collect()
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
fn oversized_submission_refusals_leave_no_run_directory() {
    let fixture = Fixture::new();
    let mut leftovers = Vec::new();
    for megabytes in [3, 5] {
        let mut command = fixture.run_command("volume", true);
        command.env("ZEROSHOT_ERROR_FORMAT", "json");
        fixture.write(
            "input.json",
            &json!({"blob": "x".repeat(megabytes * 1024 * 1024)}),
        );
        let refused = command.output().unwrap();
        assert!(!refused.status.success());
        let error: Value = serde_json::from_slice(&refused.stderr).unwrap();
        assert_eq!(error["code"], "submission.too_large", "{error}");
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

#[test]
fn profile_imports_validate_nested_local_catalog_and_refuse_untrusted_or_named_inline_graphs() {
    let fixture = Fixture::new();
    let graph = |root: Value| {
        let root = if root["kind"] == "profile" {
            json!({"kind":"seq","name":"scope","state":{"kind":"null"},"promotedStatePaths":[],"children":[root,
                {"kind":"succeed","name":"complete","output":{"kind":"null"},"bindings":[]}
            ]})
        } else {
            root
        };
        json!({
            "profile":"openengine.graph.full/v1", "initialInput":{"kind":"null"},
            "policy":{"policy":"policy.native-v2@1","default":"deny"}, "root":root
        })
    };
    let reference = |name: &str, child: &str| json!({"kind":"profile","name":name,"profile":child});
    let leaf =
        graph(json!({"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}));
    let middle = graph(reference("inner", "leaf"));
    let parent = graph(reference("outer", "middle"));
    fixture.write(
        "empty-runtime.json",
        &json!({"harness":"codex","provider":"openai","size":"host","nodes":{},"workspaceCheckpoints":false,"activeMemory":{"repository":false,"graph":false},"globalAgents":{"postmortem":{"enabled":false}}}),
    );
    fixture.write("null.json", &Value::Null);
    let save = |name: &str, value: &Value| {
        fixture.write("profile-graph.json", value);
        let output = fixture
            .command(&["profile", "set", name, "--graph"])
            .arg(fixture.root.path("profile-graph.json"))
            .arg("--runtime-config")
            .arg(fixture.root.path("empty-runtime.json"))
            .output()
            .unwrap();
        success(&output);
    };
    let validate = |target: bool| {
        let mut command = fixture.command(&[
            "run",
            "--title",
            "Profile import validation",
            "--profile",
            "local:parent",
            "--validate-only",
            "--input",
        ]);
        command.arg(fixture.root.path("null.json"));
        if target {
            command.args(["--target", "unconfigured"]);
        }
        command.output().unwrap()
    };
    save("leaf", &leaf);
    save("middle", &middle);
    save("parent", &parent);
    success(&validate(false));
    let executed = fixture
        .command(&[
            "run",
            "--title",
            "Nested local profile",
            "--profile",
            "local:parent",
            "--detach",
            "--input",
        ])
        .arg(fixture.root.path("null.json"))
        .output()
        .unwrap();
    success(&executed);
    let receipt: Value = serde_json::from_slice(&executed.stdout).unwrap();
    let id = receipt["runId"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", id]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "imported local run did not finish: {status}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    let admitted = run_directories(&fixture);
    assert_eq!(admitted.len(), 1);
    let shown = fixture.json(&["profile", "show", "parent"]);
    assert_eq!(
        shown["graph"], parent,
        "authoring keeps references, never saved expansions"
    );
    let refused = validate(true);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("profile imports require a local run")
    );
    let inline = fixture
        .command(&[
            "run",
            "--title",
            "Inline import refusal",
            "--target",
            "unconfigured",
            "--validate-only",
            "--graph",
        ])
        .arg(fixture.root.path("profile-graph.json"))
        .arg("--runtime-config")
        .arg(fixture.root.path("empty-runtime.json"))
        .arg("--input")
        .arg(fixture.root.path("null.json"))
        .output()
        .unwrap();
    assert!(!inline.status.success());
    assert!(
        String::from_utf8_lossy(&inline.stderr).contains("profile imports require a local run")
    );
    let replace_middle = |child: &str| {
        let path = fixture.root.path("config/profiles.json");
        let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        stored["profiles"]["middle"]["graph"] = graph(reference("inner", child));
        std::fs::write(path, serde_json::to_vec(&stored).unwrap()).unwrap();
    };
    replace_middle("missing");
    let missing = validate(false);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("missing profile missing"));
    replace_middle("parent");
    let cycle = validate(false);
    assert!(!cycle.status.success());
    assert!(String::from_utf8_lossy(&cycle.stderr).contains("cycle parent → middle → parent"));
    // Bypass authoring validation to model an untrusted stored expansion.
    let path = fixture.root.path("config/profiles.json");
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["profiles"]["parent"]["graph"]["root"]["children"][0]["body"] = leaf["root"].clone();
    std::fs::write(path, serde_json::to_vec(&stored).unwrap()).unwrap();
    let injected = validate(false);
    assert!(!injected.status.success());
    assert!(String::from_utf8_lossy(&injected.stderr).contains("must omit its expansion"));
    assert!(
        run_directories(&fixture) == admitted,
        "validation must not allocate runs"
    );
}

#[test]
fn contained_local_policy_reaches_submission_and_resume() {
    let fixture = Fixture::new();
    let failed = fixture
        .run_command("fail", false)
        .args(["--controller", "contained"])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    let receipt: Value =
        serde_json::from_slice(failed.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    let id = receipt["runId"].as_str().unwrap();
    let resumed = fixture
        .command(&["resume", id, "--restart", "--controller", "contained"])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let receipt: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    let successor = receipt["runId"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", successor]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "contained successor did not finish: {status}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn contained_force_stop_survives_restart() {
    let fixture = Fixture::new();
    let submitted = fixture
        .run_command("block", true)
        .args(["--controller", "contained"])
        .output()
        .unwrap();
    success(&submitted);
    let receipt: Value = serde_json::from_slice(&submitted.stdout).unwrap();
    let id = receipt["runId"].as_str().unwrap();
    let worker = fixture.await_child();
    success(&fixture.command(&["force-stop", id]).output().unwrap());
    assert_terminal_restart(&fixture, id, worker, "force_stopped");
}

// Shared with the native Windows Job fixture so portable CI exercises the same restart checks.
pub(super) fn assert_terminal_restart(fixture: &Fixture, id: &str, worker: u32, reason: &str) {
    let terminal = fixture.json(&["status", id]);
    assert_eq!(terminal["status"]["phase"], "finished");
    assert_eq!(terminal["status"]["terminalResult"]["status"], "failed");
    assert_eq!(terminal["status"]["terminalResult"]["reason"], reason);
    assert!(
        !super::process_exists(worker),
        "stopped descendant survived"
    );
    std::fs::remove_file(fixture.workspace.join("mutation.txt")).unwrap();
    let resumed = fixture
        .command(&["resume", id, "--restart", "--controller", "contained"])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let receipt: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    let successor = receipt["runId"].as_str().unwrap();
    assert_ne!(successor, id);
    assert_eq!(receipt["resumedFrom"], id);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", successor]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "restart did not finish: {status}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("mutation.txt")).unwrap(),
        "native worker ran"
    );
    assert_eq!(
        fixture.json(&["status", id])["status"]["terminalResult"],
        terminal["status"]["terminalResult"]
    );
    assert!(
        !super::process_exists(worker),
        "stopped descendant restarted"
    );
}
