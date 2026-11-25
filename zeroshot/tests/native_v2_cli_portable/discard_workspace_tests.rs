use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Fixture, success};

fn receipt(output: &std::process::Output) -> String {
    let value: Value =
        serde_json::from_slice(output.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    value["currentPartId"].as_str().unwrap().to_owned()
}

fn finished(fixture: &Fixture, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while fixture.json(&["status", id])["status"]["phase"] != "finished" {
        assert!(Instant::now() < deadline, "controller did not finish");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn assert_no_checkpoints(fixture: &Fixture, run: &str, part: &str) {
    let result = fixture.json(&["checkpoints", run, "--part", part]);
    assert_eq!(result["partId"], part);
    assert!(result["checkpoints"].as_array().unwrap().is_empty());
}

fn assert_discard_refused(fixture: &Fixture, id: &str, message: &str) {
    let output = fixture
        .command(&["discard-workspace", id])
        .env("ZEROSHOT_ERROR_FORMAT", "json")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["kind"], "invalid_request");
    assert_eq!(diagnostic["code"], "request.invalid");
    assert!(diagnostic["message"].as_str().unwrap().contains(message));

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
    let output = std::process::Command::new(python)
        .env_clear()
        .envs(
            environment
                .get_envs()
                .filter_map(|(k, v)| v.map(|v| (k, v))),
        )
        .env("PYTHONPATH", root.join("sdks/python/src"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env(
            "ZEROSHOT_PYTHON_NATIVE_BINARY",
            env!("CARGO_BIN_EXE_zeroshot"),
        )
        .current_dir(fixture.root.path(""))
        .args([
            "-c",
            r#"import asyncio, sys
from zeroshot import Client, LocalTarget, InvalidRequestError
async def main():
    async with Client(target=LocalTarget(state_dir=sys.argv[1])) as client:
        try:
            await client.get_run(sys.argv[2]).discard_workspace()
        except InvalidRequestError as error:
            assert sys.argv[3] in str(error), str(error)
        else:
            raise AssertionError("discard unexpectedly succeeded")
asyncio.run(main())
"#,
        ])
        .arg(fixture.root.path("state"))
        .args([id, message])
        .output()
        .unwrap();
    success(&output);
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.insert(path.clone(), Vec::new());
            files.extend(tree(&path));
        } else {
            files.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    files
}

#[test]
fn local_discard_frees_failed_lineage_preserves_checkout_and_allows_explicit_restart() {
    let fixture = Fixture::new().outside_workspace();
    fs::write(
        fixture.workspace.join(".git/info/exclude"),
        ".env\n*.profraw\n",
    )
    .unwrap();
    fs::write(fixture.workspace.join(".env"), "old ignored bytes").unwrap();
    let failed = fixture.run("fail", false);
    assert!(!failed.status.success());
    let original = receipt(&failed);
    let first_points = fixture.json(&["checkpoints", &original]);
    assert!(!first_points["checkpoints"].as_array().unwrap().is_empty());
    let selected = first_points["checkpoints"][0]["checkpointId"]
        .as_str()
        .unwrap();
    let next = fixture
        .command(&["resume", &original, "--restart"])
        .env("FIXTURE_MODE", "fail")
        .output()
        .unwrap();
    success(&next);
    let successor = receipt(&next);
    finished(&fixture, &successor);
    let points = fixture.json(&["checkpoints", &successor]);
    assert!(!points["checkpoints"].as_array().unwrap().is_empty());
    let storage = fixture.root.path(&format!("state/runs/{original}"));
    let config: Value =
        serde_json::from_slice(&fs::read(storage.join("checkpoints/storage.json")).unwrap())
            .unwrap();
    let repository = Path::new(config["repository"].as_str().unwrap());
    assert!(repository.join("repository/data").is_dir());
    fs::write(fixture.workspace.join(".env"), "new ignored bytes").unwrap();
    fs::write(fixture.workspace.join("seed"), "live checkout edits").unwrap();
    let before = tree(&fixture.workspace);
    let statuses =
        [&original, &successor].map(|id| fixture.json(&["status", &original, "--part", id]));
    let listing = fixture.json(&["list"]);
    let logs = fixture.command(&["logs", &original]).output().unwrap();
    assert_eq!(
        fixture.json(&["discard-workspace", &successor])["discarded"],
        true
    );
    assert!(!repository.join("repository").exists());
    assert_eq!(tree(&fixture.workspace), before);
    for (id, status) in [&original, &successor].into_iter().zip(statuses) {
        assert_eq!(status["partId"], id.as_str());
        assert_eq!(fixture.json(&["status", &original, "--part", id]), status);
        assert_no_checkpoints(&fixture, &original, id);
        assert!(
            fixture
                .root
                .path(&format!("state/runs/{id}/runs.sqlite3"))
                .is_file()
        );
        assert_eq!(fixture.json(&["discard-workspace", id])["discarded"], false);
    }
    assert_eq!(fixture.json(&["list"]), listing);
    assert_eq!(
        fixture
            .command(&["logs", &original])
            .output()
            .unwrap()
            .stdout,
        logs.stdout
    );
    for selection in [
        vec!["--from-checkpoint", selected],
        vec!["--from-checkpoint", selected, "--discard-workspace-changes"],
        vec!["--discard-workspace-changes"],
    ] {
        let output = fixture
            .command(&["resume", &successor])
            .args(selection)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("checkpoints"),
            "{output:?}"
        );
        assert_eq!(tree(&fixture.workspace), before);
    }
    let resumed = fixture
        .command(&["resume", &successor, "--restart"])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let final_id = receipt(&resumed);
    finished(&fixture, &final_id);
    assert_eq!(
        fixture.json(&["status", &final_id])["status"]["terminalResult"]["status"],
        "succeeded"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("seed")).unwrap(),
        "live checkout edits"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join(".env")).unwrap(),
        "new ignored bytes"
    );
    assert!(
        fixture.json(&["checkpoints", &final_id])["checkpoints"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn local_discard_refuses_a_running_successor_and_never_checkpointed_runs_are_noops() {
    let fixture = Fixture::new().outside_workspace();
    let failed = fixture.run("fail", false);
    let original = receipt(&failed);
    let next = fixture
        .command(&["resume", &original, "--restart"])
        .env("FIXTURE_MODE", "block")
        .output()
        .unwrap();
    success(&next);
    let successor = receipt(&next);
    fixture.await_child();
    let storage = fixture
        .root
        .path(&format!("state/runs/{original}/checkpoints"));
    let before = tree(&storage);
    let checkout = tree(&fixture.workspace);
    for id in [&original, &successor] {
        assert_discard_refused(&fixture, id, "force-stop");
        assert_eq!(tree(&storage), before);
        assert_eq!(tree(&fixture.workspace), checkout);
    }
    success(
        &fixture
            .command(&["force-stop", &successor])
            .output()
            .unwrap(),
    );
    let fixture = Fixture::new().outside_workspace();
    let runtime = fixture.root.path("runtime.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&runtime).unwrap()).unwrap();
    value["workspaceCheckpoints"] = false.into();
    fixture.write("runtime.json", &value);
    let failed = fixture.run("fail", false);
    let id = receipt(&failed);
    assert_eq!(
        fixture.json(&["discard-workspace", &id])["discarded"],
        false
    );
}

#[test]
fn local_discard_preserves_submission_key_replay() {
    let fixture = Fixture::new().outside_workspace();
    let run = || {
        fixture
            .run_command("fail", false)
            .args(["--submission-key", "discard-replay"])
            .output()
            .unwrap()
    };
    let original = run();
    assert!(!original.status.success());
    let id = receipt(&original);
    let replay = run();
    assert_eq!(receipt(&replay), id);
    assert_eq!(fixture.json(&["discard-workspace", &id])["discarded"], true);
    let replay = run();
    assert_eq!(receipt(&replay), id);
    assert!(!String::from_utf8_lossy(&replay.stderr).contains("conflict"));
}

#[test]
fn local_discard_preserves_request_continuations_without_new_checkpoints() {
    use serde_json::json;

    let fixture = Fixture::new().outside_workspace();
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    let worker = graph["root"]["children"][0].clone();
    let state = json!({"kind":"record","fields":{
        "text":{"type":{"kind":"string"},"required":true}}});
    let ask = |name| {
        json!({"kind":"verifier","name":name,"worker":"builtin.request.ask@1",
        "input":state,"output":{"kind":"record","fields":{}},"signals":{},
        "diagnostic":{"kind":"record","fields":{"message":{"type":{"kind":"string"},"required":true}}},
        "attempts":1,"inputBindings":[{"target":["text"],"value":{"source":"state","path":["text"]}}],"writeBindings":[]})
    };
    graph["initialInput"] = state.clone();
    graph["root"]["state"] = state.clone();
    graph["root"]["children"] = json!([ask("approval"), worker, ask("review"),
        {"kind":"fail","name":"failed","reason":"intentional_failure"}]);
    fixture.write("graph.json", &graph);
    fixture.write("input.json", &json!({"text":"Approve?"}));
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["nodes"]["approval"] = json!({"kind":"request"});
    runtime["nodes"]["review"] = json!({"kind":"request"});
    runtime["workspaceCheckpoints"] = false.into();
    fixture.write("runtime.json", &runtime);
    // Fresh request graphs still require checkpoints; only local continuations can opt out.
    let refused = fixture.run("finish", false);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("request nodes require"));
    runtime["workspaceCheckpoints"] = true.into();
    fixture.write("runtime.json", &runtime);
    fixture.write("answer.json", &json!({"signals":{},"output":{}}));
    let answer = |id: &str, mode: &str| {
        let output = fixture
            .command(&["answer", id, "--input"])
            .arg(fixture.root.path("answer.json"))
            .env("FIXTURE_MODE", mode)
            .output()
            .unwrap();
        success(&output);
        receipt(&output)
    };
    let suspended = fixture.run("finish", false);
    success(&suspended);
    let original = receipt(&suspended);
    let checkout = tree(&fixture.workspace);
    let checkpoints = fixture.json(&["checkpoints", &original]);
    assert_discard_refused(&fixture, &original, "run is suspended");
    assert_eq!(tree(&fixture.workspace), checkout);
    assert_eq!(fixture.json(&["checkpoints", &original]), checkpoints);
    let failed = answer(&original, "fail");
    finished(&fixture, &failed);
    fs::write(
        fixture.workspace.join("seed"),
        "keep current request workspace",
    )
    .unwrap();
    let before = tree(&fixture.workspace);
    assert_eq!(
        fixture.json(&["discard-workspace", &failed])["discarded"],
        true
    );
    assert_eq!(tree(&fixture.workspace), before);
    let resumed = fixture
        .command(&["resume", &failed, "--restart"])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let next = receipt(&resumed);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", &next]);
        if status["status"]["phase"] == "suspended" {
            assert_eq!(status["status"]["node"], "review");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "request did not suspend: {status}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    let answered = answer(&next, "finish");
    finished(&fixture, &answered);
    let resumed = fixture
        .command(&["resume", &answered, "--restart"])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let final_id = receipt(&resumed);
    finished(&fixture, &final_id);
    for id in [&original, &failed, &next, &answered, &final_id] {
        assert_no_checkpoints(&fixture, &original, id);
        assert_eq!(fixture.json(&["discard-workspace", id])["discarded"], false);
    }
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("seed")).unwrap(),
        "keep current request workspace"
    );
}

#[test]
fn local_discard_reconciles_the_restore_marker_left_by_an_interrupted_launcher() {
    use zeroshot_engine::native_v2_supervisor::checkpoints::{
        self, CheckpointRestore, CheckpointRestoreSelection,
    };
    let fixture = Fixture::new().outside_workspace();
    let original = receipt(&fixture.run("fail", false));
    let points = fixture.json(&["checkpoints", &original]);
    let selected = points["checkpoints"][0]["checkpointId"].as_str().unwrap();
    let storage = fixture.root.path(&format!("state/runs/{original}"));
    let restore = CheckpointRestore {
        directory: storage.join("checkpoints"),
        selection: CheckpointRestoreSelection::Checkpoint {
            checkpoint_id: openengine_cluster_protocol::CheckpointId::new(selected).unwrap(),
        },
    };
    let snapshot = checkpoints::pin(&restore).unwrap();
    fs::write(fixture.workspace.join("seed"), "must be restored").unwrap();
    let resumed = fixture
        .command(&[
            "resume",
            &original,
            "--from-checkpoint",
            selected,
            "--discard-workspace-changes",
        ])
        .env("FIXTURE_MODE", "fail")
        .output()
        .unwrap();
    success(&resumed);
    let successor = receipt(&resumed);
    finished(&fixture, &successor);
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("seed")).unwrap(),
        "seed"
    );
    let path = storage.join("workspace-recovery.json");
    let mut recovery: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // Recreate the durable record left if the launcher exits after controller admission,
    // before its best-effort write clearing the pending restore reaches disk.
    recovery["pendingRestore"] = serde_json::json!({
        "selection": restore.selection, "snapshot": snapshot, "attempt": successor,
    });
    fs::write(&path, serde_json::to_vec(&recovery).unwrap()).unwrap();
    let before = tree(&fixture.workspace);
    assert_eq!(
        fixture.json(&["discard-workspace", &successor])["discarded"],
        true
    );
    assert_eq!(tree(&fixture.workspace), before);
    let recovery: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(recovery.get("pendingRestore").is_none());
    assert_eq!(recovery["successorRunId"], successor);
    for id in [&original, &successor] {
        assert_no_checkpoints(&fixture, &original, id);
        assert_eq!(fixture.json(&["discard-workspace", id])["discarded"], false);
    }
    let resumed = fixture
        .command(&["resume", &successor, "--restart"])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let final_id = receipt(&resumed);
    finished(&fixture, &final_id);
    assert_eq!(
        fixture.json(&["status", &final_id])["status"]["terminalResult"]["status"],
        "succeeded"
    );
    assert!(
        fixture.json(&["checkpoints", &final_id])["checkpoints"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn local_discard_rejects_a_redirected_repository_without_changing_checkout_or_catalogs() {
    let fixture = Fixture::new().outside_workspace();
    let id = receipt(&fixture.run("fail", false));
    let catalogs = fixture.root.path(&format!("state/runs/{id}/checkpoints"));
    let config: Value =
        serde_json::from_slice(&fs::read(catalogs.join("storage.json")).unwrap()).unwrap();
    let repository = Path::new(config["repository"].as_str().unwrap());
    let retained = repository.with_extension("retained");
    fs::rename(repository, &retained).unwrap();
    let outside = fixture.root.path("unrelated");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), "unrelated bytes").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, repository).unwrap();
    #[cfg(windows)]
    success(
        &std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(repository)
            .arg(&outside)
            .output()
            .unwrap(),
    );
    let checkout = tree(&fixture.workspace);
    let before = tree(&catalogs);
    let recovery = fixture
        .root
        .path(&format!("state/runs/{id}/workspace-recovery.json"));
    let recovery_before = fs::read(&recovery).unwrap();
    let result = fixture
        .command(&["discard-workspace", &id])
        .output()
        .unwrap();
    assert!(!result.status.success(), "{result:?}");
    assert_eq!(
        fs::read_to_string(outside.join("sentinel")).unwrap(),
        "unrelated bytes"
    );
    assert_eq!(tree(&catalogs), before);
    assert_eq!(tree(&fixture.workspace), checkout);
    assert_eq!(fs::read(recovery).unwrap(), recovery_before);
    #[cfg(unix)]
    fs::remove_file(repository).unwrap();
    #[cfg(windows)]
    fs::remove_dir(repository).unwrap();
    fs::rename(retained, repository).unwrap();
    assert_eq!(fixture.json(&["discard-workspace", &id])["discarded"], true);
}
