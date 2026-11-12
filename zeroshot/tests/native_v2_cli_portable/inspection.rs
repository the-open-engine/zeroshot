//! Native command exit codes and filesystem invariance for retained local inspection.
use super::fixture::{Fixture, success};
use fs2::FileExt;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{File, Permissions};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime};

#[path = "../support/sqlite_crash.rs"]
mod sqlite_crash;

#[derive(Debug, PartialEq)]
struct Entry {
    bytes: Option<Vec<u8>>,
    modified: SystemTime,
    permissions: Permissions,
}

fn tree(path: &Path) -> BTreeMap<PathBuf, Entry> {
    let metadata = std::fs::symlink_metadata(path).unwrap();
    let mut result = BTreeMap::new();
    if metadata.is_dir() {
        for child in std::fs::read_dir(path).unwrap() {
            result.extend(tree(&child.unwrap().path()));
        }
    }
    result.insert(
        path.to_owned(),
        Entry {
            bytes: if metadata.is_file() {
                contents(path)
            } else {
                None
            },
            modified: metadata.modified().unwrap(),
            permissions: metadata.permissions(),
        },
    );
    result
}

/// A live controller locks its lease for the whole length, and Windows refuses a read of a
/// range that another process locked. Only that file's bytes may stay out of a snapshot.
fn contents(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        #[cfg(windows)]
        Err(error) if locked_lease(path, &error) => None,
        Err(error) => panic!("{}: {error}", path.display()),
    }
}

#[cfg(windows)]
fn locked_lease(path: &Path, error: &std::io::Error) -> bool {
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    path.file_name()
        .is_some_and(|name| name == "controller.lock")
        && error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32)
}

fn make_readonly(entries: &BTreeMap<PathBuf, Entry>) {
    for (path, entry) in entries.iter().rev() {
        // Unix sockets need write permission to accept connections from an observer.
        let file_type = std::fs::symlink_metadata(path).unwrap().file_type();
        if !file_type.is_dir() && !file_type.is_file() {
            continue;
        }
        let mut permissions = entry.permissions.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(if file_type.is_dir() { 0o500 } else { 0o400 });
        }
        #[cfg(windows)]
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}

fn wait_released(storage: &Path) {
    let file = File::open(storage.join("controller.lock")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(40);
    while FileExt::try_lock_shared(&file).is_err() {
        assert!(
            Instant::now() < deadline,
            "controller did not release its lease"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    FileExt::unlock(&file).unwrap();
}

fn kill_controller(pid: u32) {
    #[cfg(unix)]
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
    #[cfg(target_os = "macos")]
    {
        // Lease release precedes reaping; an unreaped PID still means unknown on macOS.
        let deadline = Instant::now() + Duration::from_secs(40);
        while super::process_exists(pid) {
            assert!(Instant::now() < deadline, "controller was not reaped");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_TERMINATE,
            PROCESS_SYNCHRONIZE,
        };
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null());
        assert_ne!(unsafe { TerminateProcess(handle, 1) }, 0);
        assert_eq!(unsafe { WaitForSingleObject(handle, 30_000) }, 0);
        unsafe {
            CloseHandle(handle);
        }
    }
}

fn inspect_responding_controller(fixture: &Fixture, id: &str, execution: &str) {
    // Await the fixture's final durable output before freezing the actively owned run.
    read_live_event(fixture, &["logs", id], "Codex turn started");
    let storage = fixture.root.path(&format!("state/runs/{id}"));
    let original = tree(&storage);
    make_readonly(&original);
    let before = tree(&storage);
    for command in ["status", "list", "checkpoints"] {
        let arguments = if command == "list" {
            vec![command]
        } else {
            vec![command, id]
        };
        let value = fixture.json(&arguments);
        let status = if command == "list" {
            &value["runs"][0]
        } else {
            &value
        };
        assert!(status.get("controller").is_none(), "{command}: {value}");
    }
    for command in ["watch", "logs", "attach"] {
        let arguments = if command == "attach" {
            vec![command, id, execution]
        } else {
            vec![command, id]
        };
        read_live_event(fixture, &arguments, "");
    }
    let after = tree(&storage);
    for (path, entry) in original {
        std::fs::set_permissions(path, entry.permissions).unwrap();
    }
    assert_eq!(before, after, "inspection changed a running run");
}

fn read_live_event(fixture: &Fixture, arguments: &[&str], expected: &str) {
    let mut child = fixture
        .command(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let expected = expected.to_owned();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let line = std::io::BufReader::new(stdout)
            .lines()
            .find(|line| match line {
                Ok(line) => line.contains(&expected),
                Err(_) => true,
            });
        let _ = sender.send(line);
    });
    let line = receiver.recv_timeout(Duration::from_secs(40));
    let _ = child.kill();
    child.wait().unwrap();
    reader.join().unwrap();
    assert!(
        line.unwrap().unwrap().is_ok(),
        "{arguments:?} produced no matching event"
    );
}

#[test]
fn inspection_commands_are_read_only_and_keep_finished_exit_codes() {
    for finished in [true, false] {
        let fixture = Fixture::new();
        let (id, execution) = if finished {
            let output = fixture.run("finish", false);
            success(&output);
            let receipt: Value =
                serde_json::from_slice(output.stdout.split(|byte| *byte == b'\n').next().unwrap())
                    .unwrap();
            (
                receipt["runId"].as_str().unwrap().to_owned(),
                "finished-execution".to_owned(),
            )
        } else {
            let output = fixture.run("block-quiet", true);
            success(&output);
            let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
            let id = receipt["runId"].as_str().unwrap().to_owned();
            fixture.await_child();
            let status = fixture.json(&["status", &id]);
            let execution = status["status"]["activeExecutions"][0]["execution"]
                .as_str()
                .unwrap()
                .to_owned();
            let ready: Value = serde_json::from_slice(
                &std::fs::read(
                    fixture
                        .root
                        .path(&format!("state/runs/{id}/controller.ready.json")),
                )
                .unwrap(),
            )
            .unwrap();
            inspect_responding_controller(&fixture, &id, &execution);
            kill_controller(ready["pid"].as_u64().unwrap() as u32);
            (id, execution)
        };
        let storage = fixture.root.path(&format!("state/runs/{id}"));
        wait_released(&storage);
        sqlite_crash::interrupt(&storage.join("runs.sqlite3"));
        // A stale bootstrap must survive inspection too; scavenging belongs to submissions.
        let stale = storage.join("controller.bootstrap.json");
        std::fs::write(&stale, b"retained-bootstrap").unwrap();
        File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::now() - Duration::from_secs(120)),
            )
            .unwrap();
        let original = tree(&storage);
        make_readonly(&original);
        let before = tree(&storage);
        for command in ["status", "list", "checkpoints", "watch", "logs", "attach"] {
            let arguments = match command {
                "list" => vec![command],
                "attach" => vec![command, &id, &execution],
                _ => vec![command, &id],
            };
            let output = fixture.command(&arguments).output().unwrap();
            let streaming = matches!(command, "watch" | "logs" | "attach");
            let expected = if command == "attach" || (streaming && !finished) {
                1
            } else {
                0
            };
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{command}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if !finished && streaming {
                let message = String::from_utf8_lossy(&output.stderr);
                assert!(
                    message.contains(&format!("zeroshot resume {id}")),
                    "{command}: {message}"
                );
                assert!(message.contains(&format!("zeroshot force-stop {id}")));
                assert!(
                    !output.stdout.is_empty(),
                    "{command} must print the recorded part"
                );
            }
            if command == "status" {
                let status: Value = serde_json::from_slice(&output.stdout).unwrap();
                if finished {
                    assert!(status.get("controller").is_none());
                } else {
                    assert_eq!(status["status"]["phase"], "running");
                    assert_eq!(status["controller"]["state"], "lost");
                    assert!(status["controller"]["observedAt"].as_u64().unwrap() > 0);
                }
            }
        }
        let after = tree(&storage);
        for (path, entry) in original {
            std::fs::set_permissions(path, entry.permissions).unwrap();
        }
        assert_eq!(before, after, "inspection changed retained run files");
        if !finished {
            success(&fixture.command(&["force-stop", &id]).output().unwrap());
            assert_eq!(
                fixture.json(&["status", &id])["status"]["terminalResult"]["reason"],
                "runtime_lost"
            );
        }
    }
}

#[test]
fn explicit_controls_recover_interrupted_ledgers_without_a_temporary_image() {
    for (command, missing_temporary_directory) in
        [("resume", false), ("resume", true), ("force-stop", true)]
    {
        let fixture = Fixture::new();
        let (id, _) = fixture.start_blocked();
        read_live_event(&fixture, &["logs", &id], "Codex turn started");
        let storage = fixture.root.path(&format!("state/runs/{id}"));
        let ready: Value =
            serde_json::from_slice(&std::fs::read(storage.join("controller.ready.json")).unwrap())
                .unwrap();
        kill_controller(ready["pid"].as_u64().unwrap() as u32);
        wait_released(&storage);
        sqlite_crash::interrupt(&storage.join("runs.sqlite3"));
        let missing = fixture.root.path("missing-temporary-directory");
        let mut control = fixture.command(&[command, &id]);
        if missing_temporary_directory {
            control
                .env("TMPDIR", &missing)
                .env("TMP", &missing)
                .env("TEMP", &missing);
        }
        control.env("FIXTURE_MODE", "finish");
        if command == "resume" && missing_temporary_directory {
            // Stop after loss confirmation and before successor launch: no credentials are
            // needed to finalize the predecessor, even when read-only image creation failed.
            control.env_remove("OPENAI_API_KEY");
        }
        let output = control.output().unwrap();
        if command == "resume" && missing_temporary_directory {
            assert_eq!(output.status.code(), Some(1));
        } else {
            success(&output);
        }
        let status = fixture.json(&["status", &id]);
        assert_eq!(
            status["status"]["terminalResult"]["reason"],
            "runtime_lost",
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!storage.join("runs.sqlite3-journal").exists());
    }
}
