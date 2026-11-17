use std::fs;
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde_json::Value;

use super::{Fixture, process_exists, success};

fn failed_run(fixture: &Fixture) -> String {
    let output = fixture.run("fail", false);
    assert!(!output.status.success());
    let receipt: Value =
        serde_json::from_slice(output.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
    let id = receipt["runId"].as_str().unwrap().to_owned();
    let ready: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .path(&format!("state/runs/{id}/controller.ready.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let pid = u32::try_from(ready["pid"].as_u64().unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(35);
    while process_exists(pid) {
        // A Linux subreaper can retain an exited controller as a zombie.
        if fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(") ")
                .is_some_and(|(_, fields)| fields.starts_with('Z'))
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "failed controller did not exit");
        std::thread::sleep(Duration::from_millis(30));
    }
    id
}

fn refusal(fixture: &Fixture, id: &str, reason: &str, path: Option<&std::path::Path>) -> String {
    let inspect = |args: &[&str]| -> Value {
        let output = fixture
            .command(args)
            .current_dir(fixture.root.as_path())
            .output()
            .unwrap();
        success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let status = inspect(&["status", id]);
    let list = inspect(&["list"]);
    let output = fixture
        .command(&["resume", id])
        .current_dir(fixture.root.as_path())
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && output.stdout.is_empty(),
        "{error}"
    );
    assert!(error.contains(reason), "expected {reason:?}: {error}");
    if let Some(path) = path {
        assert!(error.contains(&*path.to_string_lossy()), "{error}");
    }
    let structured = fixture
        .command(&["resume", id])
        .current_dir(fixture.root.as_path())
        .env("ZEROSHOT_ERROR_FORMAT", "json")
        .output()
        .unwrap();
    assert_eq!(structured.status.code(), Some(1));
    assert!(structured.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&structured.stderr).unwrap();
    assert_eq!(diagnostic["schema"], "zeroshot.error/v1");
    assert_eq!(diagnostic["kind"], "invalid_request", "{diagnostic}");
    assert_eq!(diagnostic["code"], "request.invalid", "{diagnostic}");
    let message = diagnostic["message"].as_str().unwrap();
    assert!(message.contains(reason), "{diagnostic}");
    if let Some(path) = path {
        assert!(message.contains(&*path.to_string_lossy()), "{diagnostic}");
    }
    if status["status"]["phase"] == "finished" {
        assert_eq!(inspect(&["status", id]), status);
        assert_eq!(inspect(&["list"]), list);
    }
    error.into_owned()
}

#[test]
fn local_resume_names_ineligible_run_states() {
    let fixture = Fixture::new();
    let output = fixture.run("finish", false);
    success(&output);
    let receipt: Value =
        serde_json::from_slice(output.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
    refusal(
        &fixture,
        receipt["runId"].as_str().unwrap(),
        "succeeded",
        None,
    );

    let fixture = Fixture::new();
    let (id, _) = fixture.start_blocked();
    refusal(&fixture, &id, "still running", None);
    success(&fixture.command(&["force-stop", &id]).output().unwrap());
}

#[test]
fn local_resume_names_recovery_record_and_claim_failures() {
    let fixture = Fixture::new();
    let id = failed_run(&fixture);
    let storage = fixture.root.path("state").join("runs").join(&id);
    let record = storage.join("workspace-recovery.json");
    let original = fs::read(&record).unwrap();
    fs::remove_file(&record).unwrap();
    refusal(&fixture, &id, "is missing", Some(&record));
    fs::write(&record, b"not json").unwrap();
    refusal(&fixture, &id, "invalid", Some(&record));
    fs::remove_file(&record).unwrap();
    fs::create_dir(&record).unwrap();
    refusal(&fixture, &id, "could not be read", Some(&record));
    fs::remove_dir(&record).unwrap();
    fs::write(&record, &original).unwrap();

    let claim = storage.join("workspace-recovery.claim");
    // A directory exercises unavailable claim evidence on every OS, even when tests run as root.
    fs::create_dir(&claim).unwrap();
    refusal(&fixture, &id, "claim", Some(&claim));
    fs::remove_dir(&claim).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&claim)
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    refusal(&fixture, &id, "already being resumed", Some(&claim));
    FileExt::unlock(&lock).unwrap();
    assert_eq!(fs::read(&record).unwrap(), original);
    assert_eq!(
        fixture.json(&["status", &id])["workspaceRecovery"]["recoverable"],
        true
    );
    assert_eq!(fixture.json(&["list"])["runs"].as_array().unwrap().len(), 1);
}

#[test]
fn local_resume_restores_checkpoint_into_recreated_empty_workspace() {
    let fixture = Fixture::new();
    let id = failed_run(&fixture);
    let points = fixture.json(&["checkpoints", &id]);
    let checkpoint = points["checkpoints"][0]["checkpointId"].as_str().unwrap();
    let record: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .path(&format!("state/runs/{id}/workspace-recovery.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let workspace = std::path::Path::new(record["workspace"].as_str().unwrap());
    fs::rename(&fixture.workspace, fixture.root.path("removed-workspace")).unwrap();
    let message = refusal(&fixture, &id, "no longer exists", Some(workspace));
    for hint in [
        "new empty folder",
        "--from-checkpoint",
        "--discard-workspace-changes",
    ] {
        assert!(message.contains(hint), "{message}");
    }
    fs::create_dir(&fixture.workspace).unwrap();
    let output = fixture
        .command(&[
            "resume",
            &id,
            "--from-checkpoint",
            checkpoint,
            "--discard-workspace-changes",
        ])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&output);
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let successor = receipt["runId"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", successor]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(
                status["status"]["terminalResult"]["status"], "succeeded",
                "{status}"
            );
            break;
        }
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("seed")).unwrap(),
        "seed"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("mutation.txt")).unwrap(),
        "native worker ran"
    );
    refusal(
        &fixture,
        &id,
        &format!("run already has successor {successor}"),
        None,
    );
}
