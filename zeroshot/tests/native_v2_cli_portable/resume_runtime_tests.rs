use super::{Fixture, success};
use serde_json::{Value, json};
use std::fs;
use std::time::{Duration, Instant};

fn await_success(fixture: &Fixture, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", id]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(
                status["status"]["terminalResult"]["status"], "succeeded",
                "{status}"
            );
            return;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn fixture() -> Fixture {
    let fixture = Fixture::new().outside_workspace();
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    let mut first = graph["root"]["children"][0].clone();
    first["name"] = json!("prerequisite");
    first["worker"] = json!("agent.prerequisite@1");
    first["output"] =
        json!({"kind":"record","fields":{"saved":{"type":{"kind":"string"},"required":true}}});
    first["writeBindings"] = json!([{"target":["saved"],"value":{"node":"prerequisite","channel":"out","path":["saved"]}}]);
    graph["root"]["state"] =
        json!({"kind":"record","fields":{"saved":{"type":{"kind":"string"},"required":true}}});
    graph["initialInput"] = graph["root"]["state"].clone();
    fixture.write("input.json", &json!({"saved":"caller-owned-value"}));
    graph["root"]["children"][1]["state"] = graph["root"]["state"].clone();
    graph["root"]["children"][0]["input"] = graph["root"]["state"].clone();
    graph["root"]["children"][0]["inputBindings"] =
        json!([{"target":["saved"],"value":{"source":"state","path":["saved"]}}]);
    graph["root"]["children"]
        .as_array_mut()
        .unwrap()
        .insert(0, first);
    fixture.write("graph.json", &graph);
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["nodes"]["prerequisite"] = runtime["nodes"]["worker"].clone();
    runtime["nodes"]["worker"]["model"] = json!("retired-model");
    for binding in runtime["nodes"].as_object_mut().unwrap().values_mut() {
        binding["connections"] = json!({"openai":["OPENAI_API_KEY","RESUME_RECEIPTS"]});
    }
    fixture.write("runtime.json", &runtime);
    fixture
}

fn predecessor(fixture: &Fixture) -> String {
    let output = fixture
        .run_command("finish", false)
        .env("RESUME_RECEIPTS", fixture.root.path("receipts.jsonl"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        !output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value =
        serde_json::from_slice(output.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
    let id = receipt["runId"].as_str().unwrap().to_owned();
    assert_eq!(
        fixture.json(&["status", &id])["status"]["terminalResult"]["status"],
        "failed"
    );
    id
}

#[test]
fn resume_checkpoint_keeps_worktree_identity_with_recorded_runtime() {
    let fixture = Fixture::new().outside_workspace();
    let git = |workspace: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(workspace)
            .args(args)
            .output()
            .unwrap();
        success(&output);
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let original_head = git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let output = fixture
        .run_command("fail", true)
        .arg("--worktree")
        .output()
        .unwrap();
    success(&output);
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = receipt["currentPartId"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", id]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(status["status"]["terminalResult"]["status"], "failed");
            break;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(30));
    }
    let recovery: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .path(&format!("state/runs/{id}/workspace-recovery.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let workspace = std::path::Path::new(recovery["workspace"].as_str().unwrap());
    let branch = recovery["localBranch"].as_str().unwrap();
    let points = fixture.json(&["checkpoints", id]);
    let point = points["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|point| point["node"] == "worker")
        .unwrap()["checkpointId"]
        .as_str()
        .unwrap();
    fs::write(workspace.join("seed"), "later committed recovery").unwrap();
    git(workspace, &["commit", "-am", "later recovery"]);
    let live_head = git(workspace, &["rev-parse", "HEAD"]);
    git(workspace, &["checkout", "--detach", &live_head]);
    let output = fixture
        .command(&[
            "resume",
            id,
            "--from-checkpoint",
            point,
            "--discard-workspace-changes",
        ])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&output);
    let resumed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(resumed["runId"], receipt["runId"]);
    let successor = resumed["currentPartId"].as_str().unwrap();
    assert_ne!(successor, id);
    await_success(&fixture, successor);
    assert_eq!(
        git(workspace, &["symbolic-ref", "HEAD"]),
        format!("refs/heads/{branch}")
    );
    assert_eq!(git(workspace, &["rev-parse", "HEAD"]), live_head);
    assert_eq!(fs::read_to_string(workspace.join("seed")).unwrap(), "seed");
    assert_eq!(
        fs::read_to_string(workspace.join("mutation.txt")).unwrap(),
        "native worker ran"
    );
    assert_eq!(
        git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original_head
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("seed")).unwrap(),
        "seed"
    );
    assert!(git(&fixture.workspace, &["status", "--porcelain=v1"]).is_empty());
}

#[test]
fn resume_runtime_replaces_both_file_forms_and_preserves_checkpoint_outputs() {
    for uniform in [false, true] {
        let fixture = fixture();
        let id = predecessor(&fixture);
        let points = fixture.json(&["checkpoints", &id]);
        let point = points["checkpoints"]
            .as_array()
            .unwrap()
            .iter()
            .find(|point| point["node"] == "worker")
            .unwrap()["checkpointId"]
            .as_str()
            .unwrap();
        let original: Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .path(&format!("state/runs/{id}/workspace-recovery.json")),
            )
            .unwrap(),
        )
        .unwrap();
        let mut exact: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
        for binding in exact["nodes"].as_object_mut().unwrap().values_mut() {
            binding["model"] = json!("corrected-model");
            binding["effort"] = json!("high");
            binding["harnessConfig"] = json!({"args":["--config","web_search=\"disabled\""]});
        }
        let replacement = if uniform {
            json!({"harness":"codex","provider":"openai","model":"corrected-model","effort":"high","workspaceCheckpoints":false, "harnessConfig":{"args":["--config","web_search=\"disabled\""]},"connections":{"openai":["OPENAI_API_KEY","RESUME_RECEIPTS"]},"activeMemory":{"repository":false,"graph":false}})
        } else {
            exact
        };
        fixture.write("replacement.json", &replacement);
        let output = fixture
            .command(&[
                "resume",
                &id,
                "--from-checkpoint",
                point,
                "--discard-workspace-changes",
                if uniform {
                    "--uniform-runtime-config"
                } else {
                    "--runtime-config"
                },
            ])
            .arg(fixture.root.path("replacement.json"))
            .env("RESUME_RECEIPTS", fixture.root.path("receipts.jsonl"))
            .env("OPENAI_API_KEY", "fresh-fixture-key")
            .output()
            .unwrap();
        success(&output);
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        let successor = receipt["currentPartId"].as_str().unwrap();
        await_success(&fixture, successor);
        let recovery: Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .path(&format!("state/runs/{successor}/workspace-recovery.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            recovery["submission"]["graph"],
            original["submission"]["graph"]
        );
        assert_eq!(
            recovery["submission"]["initialInput"],
            original["submission"]["initialInput"]
        );
        assert_eq!(
            recovery["submission"]["source"],
            original["submission"]["source"]
        );
        assert_eq!(
            recovery["submission"]["runtime"]["nodes"]["worker"]["model"],
            "corrected-model"
        );
        if uniform {
            assert_eq!(
                recovery["submission"]["runtime"]["workspaceCheckpoints"],
                false
            );
        }
        let predecessor: Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .path(&format!("state/runs/{id}/workspace-recovery.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(predecessor["submission"], original["submission"]);
        let calls: Vec<Value> = fs::read_to_string(fixture.root.path("receipts.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["node"] == "prerequisite")
                .count(),
            1
        );
        let last = calls.last().unwrap();
        assert_eq!(last["model"], "corrected-model");
        assert_eq!(last["credential"], "fresh-fixture-key");
        assert!(
            last["args"]
                .as_array()
                .unwrap()
                .contains(&json!("web_search=\"disabled\""))
        );
        assert!(
            last["args"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg.as_str().unwrap().contains("high"))
        );
    }
}

#[test]
fn resume_runtime_invalid_plan_leaves_workspace_and_predecessor_unchanged() {
    let fixture = fixture();
    let id = predecessor(&fixture);
    let storage = fixture.root.path(&format!("state/runs/{id}"));
    let original = fs::read(storage.join("workspace-recovery.json")).unwrap();
    let points = fixture.json(&["checkpoints", &id]);
    let point = points["checkpoints"][0]["checkpointId"].as_str().unwrap();
    fs::write(fixture.workspace.join("keep-live"), "operator edits").unwrap();
    let receipts = fs::read(fixture.root.path("receipts.jsonl")).unwrap();
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["nodes"].as_object_mut().unwrap().remove("worker");
    fixture.write("invalid.json", &runtime);
    let output = fixture
        .command(&[
            "resume",
            &id,
            "--from-checkpoint",
            point,
            "--discard-workspace-changes",
            "--runtime-config",
        ])
        .arg(fixture.root.path("invalid.json"))
        .env("RESUME_RECEIPTS", fixture.root.path("receipts.jsonl"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no binding"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("keep-live")).unwrap(),
        "operator edits"
    );
    assert_eq!(
        fs::read(storage.join("workspace-recovery.json")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(fixture.root.path("receipts.jsonl")).unwrap(),
        receipts
    );
    assert_eq!(fixture.json(&["checkpoints", &id]), points);
}

#[test]
fn resume_runtime_uses_the_harness_restored_from_the_selected_snapshot() {
    for uniform in [false, true] {
        let fixture = Fixture::new().outside_workspace();
        let bin = fixture.workspace.join("snapshot-bin");
        fs::create_dir(&bin).unwrap();
        for entry in fs::read_dir(fixture.root.path("bin space-é")).unwrap() {
            let entry = entry.unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                openengine_cluster_testkit::fixture::write_executable(
                    &bin.join(entry.file_name()),
                    fs::read(entry.path()).unwrap(),
                    entry.metadata().unwrap().permissions().mode(),
                )
                .unwrap();
            }
            #[cfg(windows)]
            fs::copy(entry.path(), bin.join(entry.file_name())).unwrap();
        }
        // Keep the fixture harness as the only Codex executable, including on provisioned hosts.
        let inherited_path = std::env::var_os("PATH").unwrap();
        let paths = std::iter::once(bin.clone()).chain(
            std::env::split_paths(&inherited_path).filter(|directory| {
                ["codex", "codex.exe", "codex.cmd", "codex.com", "codex.bat"]
                    .iter()
                    .all(|name| !directory.join(name).is_file())
            }),
        );
        let path = std::env::join_paths(paths).unwrap();
        let failed = fixture
            .run_command("fail", false)
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(!failed.status.success());
        let receipt: Value =
            serde_json::from_slice(failed.stdout.split(|byte| *byte == b'\n').next().unwrap())
                .unwrap();
        let id = receipt["runId"].as_str().unwrap();
        let checkpoints = fixture.json(&["checkpoints", id]);
        let checkpoint = checkpoints["checkpoints"][0]["checkpointId"]
            .as_str()
            .unwrap();
        fs::remove_dir_all(&bin).unwrap();
        let replacement = if uniform {
            json!({"harness":"codex","provider":"openai","model":"replacement-model",
                "activeMemory":{"repository":false,"graph":false},
                "connections":{"openai":["OPENAI_API_KEY","FIXTURE_MODE"]}})
        } else {
            let mut runtime: Value =
                serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap())
                    .unwrap();
            runtime["nodes"]["worker"]["model"] = json!("replacement-model");
            runtime
        };
        fixture.write("replacement.json", &replacement);
        let resumed = fixture
            .command(&[
                "resume",
                id,
                "--from-checkpoint",
                checkpoint,
                "--discard-workspace-changes",
                if uniform {
                    "--uniform-runtime-config"
                } else {
                    "--runtime-config"
                },
            ])
            .arg(fixture.root.path("replacement.json"))
            .env("PATH", &path)
            .env("FIXTURE_MODE", "finish")
            .output()
            .unwrap();
        success(&resumed);
        let receipt: Value = serde_json::from_slice(&resumed.stdout).unwrap();
        let successor = receipt["currentPartId"].as_str().unwrap();
        await_success(&fixture, successor);
        assert!(
            bin.join(if cfg!(windows) { "codex.cmd" } else { "codex" })
                .is_file()
        );
    }
}
