//! Exercises the shipped CLI and real OS child processes on Windows and Unix.

#[path = "native_v2_cli_portable/fixture.rs"]
mod fixture;

#[path = "native_v2_cli_portable/inspection.rs"]
mod inspection;
#[cfg(windows)]
#[path = "native_v2_cli_portable/restic_resume_tests.rs"]
mod restic_resume_tests;
use fixture::{Fixture, success};

#[test]
fn connection_store_errors_name_file_and_location_without_secret_values() {
    use std::io::Write as _;
    use std::process::Stdio;

    let fixture = Fixture::new();
    // Let the CLI create the file with native private ownership before damaging its contents.
    let mut child = fixture
        .command(&["connection", "set", "openai", "--json-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"OPENAI_API_KEY":"fixture-key"}"#)
        .unwrap();
    success(&child.wait_with_output().unwrap());
    let path = fixture.root.path("state").join("connections.json");
    let encoded = "{\n  \"openai\": {\"OPENAI_API_KEY\": 987654321012345}\n}";
    std::fs::write(&path, encoded).unwrap();
    for output in [
        fixture.command(&["connection", "list"]).output().unwrap(),
        fixture
            .command(&["connection", "delete", "openai"])
            .output()
            .unwrap(),
        fixture.run("finish", true),
    ] {
        assert!(!output.status.success());
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostic.contains(&path.display().to_string()),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("is malformed at line 2 column 47"),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("987654321012345"), "{diagnostic}");
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), encoded);
    assert!(!fixture.workspace.join("mutation.txt").exists());
}

#[cfg(unix)]
#[path = "native_v2_cli_portable/unix_controller_tests.rs"]
mod unix_controller;

#[test]
fn unchecked_worker_error_fails_foreground_status_and_logs() {
    let fixture = Fixture::new();
    let mut graph: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    graph["root"]["children"] = serde_json::json!([
        graph["root"]["children"][0].clone(),
        {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
    ]);
    fixture.write("graph.json", &graph);
    let result = fixture.run("fail", false);
    assert!(!result.status.success());
    let receipt: serde_json::Value =
        serde_json::from_slice(result.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
    let id = receipt["runId"].as_str().unwrap();
    let status = fixture.json(&["status", id]);
    let terminal = &status["status"]["terminalResult"];
    assert_eq!(terminal["status"], "failed");
    assert_eq!(terminal["reason"], "unhandled");
    let cause = terminal["context"]["primary"]["message"].as_str().unwrap();
    assert!(
        cause.contains("worker") && cause.contains("crash"),
        "{terminal}"
    );
    let logs = fixture.command(&["logs", id]).output().unwrap();
    success(&logs);
    let text = String::from_utf8(logs.stdout).unwrap();
    assert!(
        text.contains("unhandled") && text.contains("worker") && text.contains("crash"),
        "{text}"
    );
}

#[test]
fn native_local_run_handles_unicode_paths_correction_and_large_output() {
    for mode in ["finish", "correction", "volume"] {
        let fixture = Fixture::new();
        let result = fixture.run(mode, false);
        success(&result);
        assert_eq!(
            std::fs::read_to_string(fixture.workspace.join("mutation.txt")).unwrap(),
            "native worker ran"
        );
        if mode == "correction" {
            assert!(fixture.workspace.join("resumed.txt").exists());
        }
        let receipt: serde_json::Value =
            serde_json::from_slice(result.stdout.split(|byte| *byte == b'\n').next().unwrap())
                .unwrap();
        let id = receipt["runId"].as_str().unwrap();
        let status = fixture.json(&["status", id]);
        assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
        success(&fixture.command(&["logs", id]).output().unwrap());
        assert!(
            !fixture
                .root
                .path(&format!("state/runs/{id}/controller.bootstrap.json"))
                .exists()
        );
    }
}

#[test]
fn detached_run_has_concurrent_observers_and_cancels_its_descendants() {
    let fixture = Fixture::new();
    let (id, child) = fixture.start_blocked();
    let id = id.as_str();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..4 {
                    assert_eq!(fixture.json(&["status", id])["status"]["phase"], "running");
                }
            });
        }
    });
    success(&fixture.command(&["force-stop", id]).output().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while fixture.json(&["status", id])["status"]["phase"] != "finished" {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(
        !process_exists(child),
        "worker descendant survived cancellation"
    );
    success(&fixture.command(&["logs", id]).output().unwrap());
}

#[cfg(windows)]
#[test]
fn detached_controller_does_not_retain_unrelated_inheritable_handles() {
    // Handle inheritance is process-wide; concurrent fixtures must not inherit this sentinel.
    if std::env::var_os("ZEROSHOT_INHERITED_HANDLE_FIXTURE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "detached_controller_does_not_retain_unrelated_inheritable_handles",
            ])
            .env("ZEROSHOT_INHERITED_HANDLE_FIXTURE", "1")
            .output()
            .unwrap();
        success(&output);
        return;
    }
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    let fixture = Fixture::new();
    let path = fixture.root.path("caller-owned-handle");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert_ne!(
        unsafe {
            SetHandleInformation(
                file.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        },
        0
    );
    let (id, _) = fixture.start_blocked();
    drop(file);
    let removed = std::fs::remove_file(path);
    success(&fixture.command(&["force-stop", &id]).output().unwrap());
    assert!(
        removed.is_ok(),
        "controller inherited a caller-owned handle: {removed:?}"
    );
}

#[cfg(windows)]
#[test]
fn controller_death_closes_its_job_and_replays_runtime_loss() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    let fixture = Fixture::new();
    let (id, child) = fixture.start_blocked();
    let id = id.as_str();
    let ready: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .root
                .path(&format!("state/runs/{id}/controller.ready.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let controller = u32::try_from(ready["pid"].as_u64().unwrap()).unwrap();
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, controller) };
    assert!(!handle.is_null());
    assert_ne!(unsafe { TerminateProcess(handle, 1) }, 0);
    unsafe {
        CloseHandle(handle);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while process_exists(child) {
        assert!(
            std::time::Instant::now() < deadline,
            "descendant outlived the controller Job"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let status = fixture.json(&["status", id]);
    assert_eq!(status["status"]["phase"], "running");
    assert_eq!(status["controller"]["state"], "lost");
    assert_eq!(
        fixture
            .command(&["logs", id])
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
    success(&fixture.command(&["force-stop", id]).output().unwrap());
    let status = fixture.json(&["status", id]);
    assert_eq!(status["status"]["phase"], "finished");
    assert!(status.get("controller").is_none());
    assert_eq!(status["status"]["terminalResult"]["reason"], "runtime_lost");
    success(&fixture.command(&["logs", id]).output().unwrap());
}

fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let pending = unsafe { WaitForSingleObject(handle, 0) } == 258;
        unsafe {
            CloseHandle(handle);
        }
        pending
    }
}
