//! Real caller Jobs, shipped CLI/controllers, checkpoints and runtime-owned descendants.
use std::fs::{self, File};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde_json::Value;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, ResumeThread, THREAD_SUSPEND_RESUME, TerminateProcess,
};

use super::{Fixture, process_exists, success};

struct CallerJob(Option<OwnedHandle>);

impl CallerJob {
    fn raw(&self) -> HANDLE {
        self.0.as_ref().unwrap().as_raw_handle()
    }

    fn close(mut self) {
        self.0.take();
    }

    fn new(flags: u32) -> Self {
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        assert!(!raw.is_null());
        let job = Self(Some(unsafe { OwnedHandle::from_raw_handle(raw) }));
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = flags;
        assert_ne!(
            unsafe {
                SetInformationJobObject(
                    raw,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&limits).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            },
            0
        );
        job
    }

    fn contains(&self, pid: u32) -> bool {
        let process = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION);
        let mut present = 0;
        assert_ne!(
            unsafe { IsProcessInJob(process.as_raw_handle(), self.raw(), &mut present,) },
            0
        );
        present != 0
    }

    fn flags(&self) -> u32 {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        assert_ne!(
            unsafe {
                QueryInformationJobObject(
                    self.raw(),
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_mut(&mut limits).cast(),
                    std::mem::size_of_val(&limits) as u32,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        limits.BasicLimitInformation.LimitFlags
    }

    fn terminate(&self) {
        assert_ne!(unsafe { TerminateJobObject(self.raw(), 1) }, 0);
    }
}

impl Drop for CallerJob {
    fn drop(&mut self) {
        if self.0.is_some() {
            // Retire our own fixtures on an assertion failure too; close() tests native lifetime.
            unsafe {
                TerminateJobObject(self.raw(), 1);
            }
        }
    }
}

// An outer harness owns the handles, so closing the observing CLI and closing the last Job
// handle are distinct actions. Assign the CLI while suspended, before it can launch a controller.
fn spawn_in_jobs(command: &mut Command, jobs: &[&CallerJob]) -> Child {
    let mut child = command.creation_flags(CREATE_SUSPENDED).spawn().unwrap();
    for job in jobs {
        if unsafe { AssignProcessToJobObject(job.raw(), child.as_raw_handle()) } == 0 {
            let error = std::io::Error::last_os_error();
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("fixture Job assignment failed: {error}");
        }
    }
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    assert_ne!(snapshot, INVALID_HANDLE_VALUE);
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
    while found {
        if entry.th32OwnerProcessID == child.id() {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            assert!(!thread.is_null());
            let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
            assert_eq!(unsafe { ResumeThread(thread.as_raw_handle()) }, 1);
            return child;
        }
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    child.kill().unwrap();
    child.wait().unwrap();
    panic!("fixture launch thread not found");
}

fn open_process(pid: u32, access: u32) -> OwnedHandle {
    let process = unsafe { OpenProcess(access, 0, pid) };
    assert!(
        !process.is_null(),
        "OpenProcess({pid}): {}",
        std::io::Error::last_os_error()
    );
    unsafe { OwnedHandle::from_raw_handle(process) }
}

fn wait_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "contained controller fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn output(mut child: Child) -> Output {
    wait_until(|| child.try_wait().unwrap().is_some());
    child.wait_with_output().unwrap()
}

fn controller_pid(fixture: &Fixture, id: &str) -> u32 {
    let ready: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .path(&format!("state/runs/{id}/controller.ready.json")),
        )
        .unwrap(),
    )
    .unwrap();
    ready["pid"].as_u64().unwrap().try_into().unwrap()
}

fn start(
    fixture: &Fixture,
    jobs: &[&CallerJob],
    mut command: Command,
) -> (Child, String, u32, u32) {
    let receipt = fixture.root.path("receipt.json");
    command
        .args(["--controller", "contained"])
        .stdout(File::create(&receipt).unwrap())
        .stderr(Stdio::piped());
    let mut cli = spawn_in_jobs(&mut command, jobs);
    let mut id = None;
    wait_until(|| {
        id = fs::read(&receipt).ok().and_then(|bytes| {
            serde_json::from_slice::<Value>(bytes.split(|b| *b == b'\n').next()?).ok()?["runId"]
                .as_str()
                .map(str::to_owned)
        });
        if id.is_none() && cli.try_wait().unwrap().is_some() {
            panic!(
                "contained CLI exited without a receipt: {}",
                fs::read_to_string(&receipt).unwrap()
            );
        }
        id.is_some()
    });
    let id = id.unwrap();
    let worker = fixture.await_child();
    let controller = controller_pid(fixture, &id);
    for job in jobs {
        assert!(job.contains(controller), "controller escaped caller Job");
        assert!(job.contains(worker), "worker escaped caller Job");
    }
    (cli, id, controller, worker)
}

fn long_worker(fixture: &Fixture) {
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    graph["root"]["children"][0]["timeoutMs"] = 180_000.into();
    fixture.write("graph.json", &graph);
}

#[test]
fn contained_caller_job_lifetimes_and_force_stop() {
    for flags in [0, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE] {
        let fixture = Fixture::new();
        long_worker(&fixture);
        let job = CallerJob::new(flags);
        let (mut cli, id, controller, worker) =
            start(&fixture, &[&job], fixture.run_command("block", false));
        assert_eq!(job.flags(), flags, "caller Job policy changed");
        // Killing only the observer is never controller loss, even in a kill-on-close Job.
        cli.kill().unwrap();
        cli.wait().unwrap();
        assert!(process_exists(controller) && process_exists(worker));
        assert_eq!(fixture.json(&["status", &id])["status"]["phase"], "running");
        let refused = fixture
            .command(&["resume", &id, "--controller", "contained"])
            .output()
            .unwrap();
        assert!(
            !refused.status.success(),
            "live predecessor admitted a second writer"
        );
        assert_eq!(
            fs::read_dir(fixture.root.path("state/runs"))
                .unwrap()
                .count(),
            1
        );
        job.close(); // Last caller Job handle: native policy, not CLI liveness, decides lifetime.
        if flags == 0 {
            assert!(process_exists(controller) && process_exists(worker));
            success(&fixture.command(&["force-stop", &id]).output().unwrap());
            wait_until(|| !process_exists(worker));
            assert_eq!(
                fixture.json(&["status", &id])["status"]["terminalResult"]["reason"],
                "force_stopped"
            );
        } else {
            wait_until(|| !process_exists(controller) && !process_exists(worker));
            let lost = fixture.json(&["status", &id]);
            assert_eq!(lost["controller"]["state"], "lost");
            success(&fixture.command(&["force-stop", &id]).output().unwrap());
        }
        success(&fixture.command(&["force-stop", &id]).output().unwrap());
        let terminal = fixture.json(&["status", &id]);
        assert_eq!(terminal["status"]["phase"], "finished");
        assert_eq!(terminal["status"]["terminalResult"]["status"], "failed");
        assert!(
            !fixture
                .command(&["resume", &id, "--controller", "contained"])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(
            fixture.json(&["status", &id])["status"]["terminalResult"],
            terminal["status"]["terminalResult"]
        );
    }
}

#[test]
fn contained_nested_job_checkpoint_recovery_has_one_successor() {
    let fixture = Fixture::new();
    long_worker(&fixture);
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    let mut first = graph["root"]["children"][0].clone();
    first["name"] = "completed".into();
    first["worker"] = "agent.completed@1".into();
    graph["root"]["children"]
        .as_array_mut()
        .unwrap()
        .insert(0, first);
    fixture.write("graph.json", &graph);
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["nodes"]["worker"]["connections"]["openai"]
        .as_array_mut()
        .unwrap()
        .push("FIXTURE_RECEIPTS".into());
    let mut completed = runtime["nodes"]["worker"].clone();
    completed["model"] = "completed-step".into();
    runtime["nodes"]["completed"] = completed;
    fixture.write("runtime.json", &runtime);
    // Receipt path stays outside the checkpoint, making re-execution observable after restore.
    let receipts = fixture.root.path("executions.txt");
    // Use the runtime's declared shell field without changing the reusable fixture environment.
    let ancestor = CallerJob::new(0);
    let inner = CallerJob::new(JOB_OBJECT_LIMIT_BREAKAWAY_OK);
    let mut command = fixture.run_command("block", false);
    command.env("FIXTURE_RECEIPTS", &receipts);
    let (mut cli, id, controller, worker) = start(&fixture, &[&ancestor, &inner], command);
    let id = id.as_str();
    assert!(ancestor.contains(controller) && inner.contains(controller));
    assert!(ancestor.contains(worker) && inner.contains(worker));
    assert_eq!(ancestor.flags(), 0);
    assert_eq!(inner.flags(), JOB_OBJECT_LIMIT_BREAKAWAY_OK);
    cli.kill().unwrap();
    cli.wait().unwrap();
    let process = open_process(controller, PROCESS_TERMINATE);
    assert_ne!(unsafe { TerminateProcess(process.as_raw_handle(), 1) }, 0);
    wait_until(|| !process_exists(controller) && !process_exists(worker));
    // Simulate a predecessor still releasing its original lease. No writer may start while held.
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(
            fixture
                .root
                .path(&format!("state/runs/{id}/controller.lock")),
        )
        .unwrap();
    lease.try_lock_exclusive().unwrap();
    let refused = fixture
        .command(&["resume", id, "--controller", "contained"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(
        fs::read_to_string(&receipts).unwrap(),
        "completed\nworker\n"
    );
    drop(lease);
    fs::write(fixture.workspace.join("completed.txt"), "live edits").unwrap();
    let checkpoints = fixture.json(&["checkpoints", id]);
    let checkpoint = checkpoints["checkpoints"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["checkpointId"]
        .as_str()
        .unwrap();
    let resume = || {
        let mut command = fixture.command(&[
            "resume",
            id,
            "--controller",
            "contained",
            "--from-checkpoint",
            checkpoint,
            "--discard-workspace-changes",
        ]);
        command
            .env("FIXTURE_MODE", "finish")
            .env("FIXTURE_RECEIPTS", &receipts)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        spawn_in_jobs(&mut command, &[&ancestor, &inner])
    };
    let one = resume();
    let two = resume();
    let outcomes = [output(one), output(two)];
    assert_eq!(
        outcomes.iter().filter(|o| o.status.success()).count(),
        1,
        "{outcomes:?}"
    );
    let resumed: Value =
        serde_json::from_slice(&outcomes.iter().find(|o| o.status.success()).unwrap().stdout)
            .unwrap();
    let successor = resumed["runId"].as_str().unwrap();
    wait_until(|| fixture.json(&["status", successor])["status"]["phase"] == "finished");
    assert_eq!(
        fixture.json(&["status", successor])["status"]["terminalResult"]["status"],
        "succeeded"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("completed.txt")).unwrap(),
        "retained work"
    );
    assert_eq!(
        fs::read_to_string(receipts).unwrap(),
        "completed\nworker\nworker\n"
    );
    assert_eq!(
        fs::read_dir(fixture.root.path("state/runs"))
            .unwrap()
            .count(),
        2
    );
    ancestor.terminate();
}

#[test]
fn contained_incompatible_job_refuses_before_worker_execution() {
    let fixture = Fixture::new();
    let job = CallerJob::new(0);
    let restrictions = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES,
    };
    assert_ne!(
        unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectBasicUIRestrictions,
                std::ptr::from_ref(&restrictions).cast(),
                std::mem::size_of_val(&restrictions) as u32,
            )
        },
        0
    );
    let mut command = fixture.run_command("finish", true);
    command
        .args(["--controller", "contained"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let refused = output(spawn_in_jobs(&mut command, &[&job]));
    assert!(
        !refused.status.success(),
        "incompatible nesting succeeded: {refused:?}"
    );
    let reason = String::from_utf8_lossy(&refused.stderr);
    assert!(
        reason.contains("os error 5"),
        "expected native Job assignment rejection: {reason}"
    );
    assert!(!fixture.workspace.join("mutation.txt").exists());
    assert!(!fixture.workspace.join("child.pid").exists());
    assert_eq!(job.flags(), 0);
    job.terminate();
}
