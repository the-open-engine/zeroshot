//! Runs on native Linux and macOS, against the shipped detached controller and owned children.

use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde_json::Value;

use super::{Fixture, process_exists, success};

#[path = "unix_controller_work_tests.rs"]
mod work;

/// The controller bounds its own shutdown at 30 seconds. Waiting past that bound separates a stop
/// that slow storage delayed from a controller that never settled: a test that expects a recorded
/// stop asserts the terminal afterwards, which a shutdown that ran into the bound does not have.
const STOP_LIMIT: Duration = Duration::from_secs(35);

struct Run {
    fixture: Fixture,
    id: String,
    child: u32,
    controller: i32,
    storage: PathBuf,
}

impl Run {
    fn start() -> Self {
        let fixture = Fixture::new();
        let (id, child) = fixture.start_blocked();
        Self::from_started(fixture, id, child)
    }

    fn from_started(fixture: Fixture, id: String, child: u32) -> Self {
        let storage = fixture.root.path(&format!("state/runs/{id}"));
        let ready: Value =
            serde_json::from_slice(&std::fs::read(storage.join("controller.ready.json")).unwrap())
                .unwrap();
        let controller = ready["pid"].as_i64().unwrap().try_into().unwrap();
        Self {
            fixture,
            id,
            child,
            controller,
            storage,
        }
    }

    fn signal(&self, signal: i32) {
        assert_eq!(unsafe { libc::kill(self.controller, signal) }, 0);
    }

    fn wait_closed(&self, limit: Duration) {
        let lock = File::open(self.storage.join("controller.lock")).unwrap();
        let end = Instant::now() + limit;
        while lock.try_lock_exclusive().is_err() {
            assert!(Instant::now() < end, "controller did not release ownership");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !process_exists(self.child),
            "owned child outlived settlement"
        );
    }

    // Inspect without issuing a CLI command, which could recover a lost controller itself.
    fn terminal(&self) -> Value {
        let connection = rusqlite::Connection::open_with_flags(
            self.storage.join("runs.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let stored: String = connection
            .query_row(
                "SELECT stored_json FROM v2_runs WHERE run_id = ?1",
                [&self.id],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str::<Value>(&stored).unwrap()["snapshot"]["terminal"].clone()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        // Failed assertions must not leave a detached fixture running.
        let lock = File::open(self.storage.join("controller.lock")).unwrap();
        if lock.try_lock_exclusive().is_err() {
            unsafe {
                libc::kill(self.controller, libc::SIGKILL);
            }
        }
    }
}

#[test]
fn unix_controller_sigterm_persists_stop_retains_checkpoints_and_resumes() {
    let run = Run::start();
    let catalog = std::fs::read_dir(run.storage.join("checkpoints"))
        .unwrap()
        .count();
    assert!(catalog > 0);
    run.signal(libc::SIGTERM);
    run.wait_closed(STOP_LIMIT);
    assert_eq!(run.terminal()["reason"], "force_stopped");
    assert!(run.storage.join("checkpoints").is_dir());
    assert_eq!(
        std::fs::read_to_string(run.fixture.workspace.join("mutation.txt")).unwrap(),
        "native worker ran"
    );
    let checkpoints = run.fixture.json(&["checkpoints", &run.id]);
    assert!(!checkpoints["checkpoints"].as_array().unwrap().is_empty());
    success(
        &run.fixture
            .command(&["resume", &run.id])
            .env("FIXTURE_MODE", "finish")
            .output()
            .unwrap(),
    );
    assert_eq!(run.terminal()["reason"], "force_stopped");
}

#[test]
fn unix_controller_repeated_sigterm_waits_for_durable_stop_after_slow_ledger() {
    let run = Run::start();
    let ledger = rusqlite::Connection::open(run.storage.join("runs.sqlite3")).unwrap();
    ledger.execute_batch("BEGIN IMMEDIATE").unwrap();
    for _ in 0..10 {
        run.signal(libc::SIGTERM);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        run.terminal().is_null(),
        "stop was reported before persistence"
    );
    assert!(
        File::open(run.storage.join("controller.lock"))
            .unwrap()
            .try_lock_exclusive()
            .is_err()
    );
    ledger.execute_batch("ROLLBACK").unwrap();
    run.wait_closed(STOP_LIMIT);
    assert_eq!(run.terminal()["reason"], "force_stopped");
}

/// The provider reports a turn every 50 ms and each report is a ledger write. An outside
/// transaction keeps the first such write, and with it the controller's ledger connection, busy,
/// so the serving loop's next status read queues behind that write. The stop request finds the
/// read there and has to record the stop all the same once the ledger is free again.
#[test]
fn unix_controller_sigterm_behind_a_busy_ledger_still_persists_the_stop() {
    let run = Run::start();
    let ledger = rusqlite::Connection::open(run.storage.join("runs.sqlite3")).unwrap();
    ledger.execute_batch("BEGIN IMMEDIATE").unwrap();
    // Several provider reports and status reads fall into each pause.
    std::thread::sleep(Duration::from_millis(300));
    run.signal(libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(300));
    ledger.execute_batch("ROLLBACK").unwrap();
    run.wait_closed(STOP_LIMIT);
    assert_eq!(run.terminal()["reason"], "force_stopped");
}

#[test]
fn unix_controller_blocked_ledger_has_bounded_shutdown_without_a_fabricated_stop() {
    let run = Run::start();
    let ledger = rusqlite::Connection::open(run.storage.join("runs.sqlite3")).unwrap();
    ledger.execute_batch("BEGIN IMMEDIATE").unwrap();
    run.signal(libc::SIGTERM);
    let end = Instant::now() + STOP_LIMIT;
    let lock = File::open(run.storage.join("controller.lock")).unwrap();
    while lock.try_lock_exclusive().is_err() {
        assert!(
            Instant::now() < end,
            "repeated signals extended the shutdown deadline"
        );
        // Exit can race this signal; only the controller's owned lock decides settlement.
        unsafe {
            libc::kill(run.controller, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(lock);
    run.wait_closed(STOP_LIMIT);
    assert!(run.terminal().is_null());
    ledger.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        run.fixture.json(&["status", &run.id])["status"]["terminalResult"]["reason"],
        "runtime_lost"
    );
    assert!(run.storage.join("checkpoints").is_dir());
}

#[test]
fn unix_controller_detachment_preserves_terminal_closure_behavior() {
    use std::os::fd::{FromRawFd, AsRawFd};
    use std::os::unix::process::CommandExt;

    let fixture = Fixture::new();
    let mut master = -1;
    let mut slave = -1;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    let terminal = unsafe { File::from_raw_fd(master) };
    let input = unsafe { File::from_raw_fd(slave) };
    assert_eq!(
        unsafe { libc::fcntl(terminal.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
        0
    );
    let mut command = fixture.run_command("block", true);
    command.stdin(input);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    success(&output);
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let child = fixture.await_child();
    let run = Run::from_started(
        fixture,
        receipt["runId"].as_str().unwrap().to_owned(),
        child,
    );
    // The detached controller is in a different session from the invoking terminal/CLI.
    assert_eq!(unsafe { libc::getsid(run.controller) }, run.controller);
    assert_ne!(unsafe { libc::getsid(0) }, run.controller);
    drop(terminal);
    std::thread::sleep(Duration::from_millis(100));
    // Ownership and the live provider show that the run survived the closed terminal. The stop
    // recorded below shows that nothing else had ended it.
    assert!(
        File::open(run.storage.join("controller.lock"))
            .unwrap()
            .try_lock_exclusive()
            .is_err(),
        "terminal closure ended the detached controller"
    );
    assert!(process_exists(run.child));
    run.signal(libc::SIGTERM);
    run.wait_closed(STOP_LIMIT);
    assert_eq!(run.terminal()["reason"], "force_stopped");
}
