use super::fixture::{Fixture, success};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

struct RestorePermissions(PathBuf, std::fs::Permissions);

impl Drop for RestorePermissions {
    fn drop(&mut self) {
        std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
    }
}

fn receipt_id(output: &std::process::Output) -> String {
    let receipt: Value =
        serde_json::from_slice(output.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
    receipt["runId"].as_str().unwrap().to_owned()
}

#[test]
fn unreadable_history_allows_new_runs_and_resume_without_changing_inspection() {
    let fixture = Fixture::new();
    let mut runtime: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["workspaceCheckpoints"] = Value::Bool(false);
    fixture.write("runtime.json", &runtime);
    let failed = fixture.run("fail", false);
    assert_eq!(failed.status.code(), Some(1));
    let failed_id = receipt_id(&failed);
    assert_eq!(
        fixture.json(&["status", &failed_id])["status"]["terminalResult"]["status"],
        "failed"
    );

    let denied_id = "0199f33f-3b44-7d21-9000-000000000000";
    let denied = fixture.root.path(&format!("state/runs/{denied_id}"));
    std::fs::create_dir(&denied).unwrap();
    let denied_bootstrap = denied.join("controller.bootstrap.json");
    std::fs::write(&denied_bootstrap, b"keep").unwrap();
    let restore = RestorePermissions(
        denied.clone(),
        std::fs::metadata(&denied).unwrap().permissions(),
    );
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
    if !std::fs::symlink_metadata(&denied_bootstrap)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        eprintln!("skipping: directory permissions are not enforced for this user");
        return;
    }

    // Search permission permits metadata lookup, but does not make this history readable.
    let searchable = fixture
        .root
        .path("state/runs/0199f33f-3b44-7d21-9000-000000000001");
    std::fs::create_dir(&searchable).unwrap();
    let searchable_restore = RestorePermissions(
        searchable.clone(),
        std::fs::metadata(&searchable).unwrap().permissions(),
    );
    let searchable_bootstrap = searchable.join("controller.bootstrap.json");
    std::fs::write(&searchable_bootstrap, b"keep").unwrap();
    for name in ["runs.sqlite3", "workspace-recovery.json"] {
        let path = searchable.join(name);
        std::fs::write(&path, b"unreadable").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    std::fs::set_permissions(&searchable, std::fs::Permissions::from_mode(0o111)).unwrap();
    if !std::fs::read_dir(&searchable)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        eprintln!("skipping: directory permissions are not enforced for this user");
        return;
    }
    assert!(std::fs::symlink_metadata(searchable.join("runs.sqlite3")).is_ok());
    assert!(
        std::fs::read(searchable.join("workspace-recovery.json"))
            .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    );

    let stale = fixture
        .root
        .path(&format!("state/runs/{failed_id}/controller.bootstrap.json"));
    std::fs::write(&stale, b"stale").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(120)),
        )
        .unwrap();
    let listed = fixture.json(&["list"]);
    assert_eq!(listed["runs"].as_array().unwrap().len(), 1);
    assert_eq!(listed["runs"][0]["runId"], failed_id);
    assert!(stale.exists(), "inspection must not scavenge startup files");
    assert_eq!(
        fixture.json(&["status", &failed_id])["status"]["terminalResult"]["status"],
        "failed"
    );

    let admitted = fixture
        .run_command("finish", false)
        .args(["--submission-key", "readable-history"])
        .output()
        .unwrap();
    success(&admitted);
    assert!(!stale.exists());
    let admitted_id = receipt_id(&admitted);
    let repeated = fixture
        .run_command("finish", false)
        .args(["--submission-key", "readable-history"])
        .output()
        .unwrap();
    success(&repeated);
    assert_eq!(receipt_id(&repeated), admitted_id);

    let resumed = fixture
        .command(&["resume", &failed_id])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let successor = receipt_id(&resumed);
    assert_ne!(successor, failed_id);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", &successor]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "successor did not finish: {status}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    drop(restore);
    drop(searchable_restore);
    assert_eq!(std::fs::read(denied_bootstrap).unwrap(), b"keep");
    assert_eq!(std::fs::read(searchable_bootstrap).unwrap(), b"keep");
}
