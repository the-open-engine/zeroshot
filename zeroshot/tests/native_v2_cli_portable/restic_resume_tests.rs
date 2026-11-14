use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Fixture, process_exists, success};

#[test]
fn resume_rediscovers_restic_after_the_install_moves() {
    let restic = Path::new(env!("CARGO_BIN_EXE_zeroshot"))
        .with_file_name(format!("restic{}", std::env::consts::EXE_SUFFIX));
    for (selected, fallback) in [(false, false), (true, false), (false, true), (true, true)] {
        let fixture = Fixture::new();
        let original = fixture
            .root
            .path(&format!("old-restic{}", std::env::consts::EXE_SUFFIX));
        let replacement = fixture
            .root
            .path(&format!("new-restic{}", std::env::consts::EXE_SUFFIX));
        fs::copy(&restic, &original).unwrap();
        fs::copy(&restic, &replacement).unwrap();
        let failed = fixture
            .run_command("fail", false)
            .env("ZEROSHOT_RESTIC", &original)
            .output()
            .unwrap();
        assert!(!failed.status.success());
        let receipt: Value =
            serde_json::from_slice(failed.stdout.split(|byte| *byte == b'\n').next().unwrap())
                .unwrap();
        let id = receipt["runId"].as_str().unwrap();
        let storage = fixture.root.path(&format!("state/runs/{id}"));
        let ready: Value =
            serde_json::from_slice(&fs::read(storage.join("controller.ready.json")).unwrap())
                .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let pid = u32::try_from(ready["pid"].as_u64().unwrap()).unwrap();
        while process_exists(pid) {
            // An ancestor subreaper can retain an exited controller's zombie on Linux.
            if fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                stat.rsplit_once(") ")
                    .is_some_and(|(_, fields)| fields.starts_with('Z'))
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "failed controller did not exit");
            std::thread::sleep(Duration::from_millis(30));
        }
        let metadata = storage.join("checkpoints/storage.json");
        let recorded = fs::read(&metadata).unwrap();
        let configuration: Value = serde_json::from_slice(&recorded).unwrap();
        assert_eq!(
            configuration["program"]["executable"],
            fs::canonicalize(&original).unwrap().to_str().unwrap()
        );
        if !fallback {
            fs::remove_file(&original).unwrap();
        }
        fs::write(fixture.workspace.join("seed"), "live edits").unwrap();
        let points = fixture.json(&["checkpoints", id]);
        let checkpoint = points["checkpoints"][0]["checkpointId"].as_str().unwrap();
        let mut command = fixture.command(&["resume", id]);
        if selected {
            command.args([
                "--from-checkpoint",
                checkpoint,
                "--discard-workspace-changes",
            ]);
        }
        // An unavailable explicit selector makes discovery fail even if the developer has
        // a bundled or system Restic. Resume must still use its valid stored program.
        let selected_program = if fallback {
            fixture.root.path("missing-restic")
        } else {
            replacement
        };
        let resumed = command
            .env("ZEROSHOT_RESTIC", selected_program)
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
                assert_eq!(
                    status["status"]["terminalResult"]["status"], "succeeded",
                    "{status}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "resumed controller did not finish: {status}"
            );
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(
            fs::read_to_string(fixture.workspace.join("seed")).unwrap(),
            if selected { "seed" } else { "live edits" }
        );
        assert_eq!(
            fs::read_to_string(fixture.workspace.join("mutation.txt")).unwrap(),
            "native worker ran"
        );
    }
}
