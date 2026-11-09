use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use fs2::FileExt;
use openengine_cluster_testkit::assertions::AssertValue;
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep, timeout};
use crate::execution::driver::{DriverCancellation, WorkspaceCapability};
use crate::execution::WorkspaceAccessMode;
use crate::execution::process::{LocalProcessRunner, ProcessSessionCommand, ProcessSession};
use crate::native_v2_portable_controller::{ControllerLease, workspace_lease_path};

const FIXTURE: &str = "ZEROSHOT_TREE_OWNER_FIXTURE";
const OWNER_TEST: &str = "execution::process::tree_supervisor::tests::owner_loss_preserves_workspace_until_its_tree_is_reaped";
const CLOSED_TEST: &str = "execution::process::tree_supervisor::tests::closed_stdio_cannot_replace_supervision_descriptors";

async fn session(
    workspace: &Path,
    script: &str,
) -> (ProcessSession, tokio::sync::watch::Sender<bool>) {
    let (sender, receiver) = tokio::sync::watch::channel(false);
    // Keep the cancellation source alive: dropping it is not an intentional owner-loss signal.
    let cancellation = DriverCancellation::new(receiver);
    let command = ProcessSessionCommand {
        program: "/bin/sh".to_owned(),
        argv: vec!["-c".to_owned(), script.to_owned()],
        environment: BTreeMap::new(),
        workspace: WorkspaceCapability {
            current_dir: workspace.to_owned(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: None,
    };
    let session = LocalProcessRunner::new()
        .open(command, cancellation)
        .await
        .assert_value();
    (session, sender)
}
async fn finished(session: &mut ProcessSession) -> super::super::ProcessSessionOutput {
    timeout(Duration::from_secs(5), async {
        while session.recv_stdout().await.is_some() {}
        session.wait().await.assert_value()
    })
    .await
    .assert_value()
}
fn fixture_child(name: &str, root: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().assert_value());
    command
        .args(["--exact", name, "--nocapture"])
        .env(FIXTURE, root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}
async fn ready(path: &Path) -> String {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(value) = fs::read_to_string(path) {
            if !value.is_empty() {
                return value;
            }
        }
        assert!(
            Instant::now() < end,
            "fixture did not write {}",
            path.display()
        );
        sleep(Duration::from_millis(10)).await;
    }
}
fn live(pid: i32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rfind(") ").and_then(|i| s.as_bytes().get(i + 2).copied()))
        .is_some_and(|state| !matches!(state, b'Z' | b'X'))
}
fn send(pid: i32, signal: i32) {
    // SAFETY: PID comes from this fixture's owned process, with lifetime controlled by the test.
    assert_eq!(
        unsafe { libc::kill(pid, signal) },
        0,
        "could not signal owned fixture"
    );
}

#[tokio::test]
async fn closed_stdio_cannot_replace_supervision_descriptors() {
    if let Some(root) = std::env::var_os(FIXTURE) {
        // SAFETY: this is a dedicated test subprocess, not the parent test runner.
        unsafe {
            for fd in 0..3 {
                libc::close(fd);
            }
        }
        let (mut command, _source) = session(Path::new(&root), "exit 29").await;
        let output = finished(&mut command).await;
        fs::write(
            Path::new(&root).join("status"),
            format!("{:?}", output.exit_code),
        )
        .assert_value();
        std::process::exit(i32::from(output.exit_code != Some(29)));
    }
    let root = tempfile::tempdir().assert_value();
    let status = timeout(
        Duration::from_secs(10),
        fixture_child(CLOSED_TEST, root.path()).status(),
    )
    .await
    .assert_value()
    .assert_value();
    assert!(status.success(), "closed-stdio supervisor child failed");
    assert_eq!(
        fs::read_to_string(root.path().join("status")).assert_value(),
        "Some(29)"
    );
}

#[tokio::test]
async fn supervisor_preserves_root_exit_status_and_signal() {
    let root = tempfile::tempdir().assert_value();
    let (mut normal, _normal_source) = session(root.path(), "exit 37").await;
    let output = finished(&mut normal).await;
    assert_eq!(output.exit_code, Some(37));
    assert_eq!(output.termination_signal, None);
    let (mut signal, _signal_source) = session(root.path(), "kill -TERM $$").await;
    let output = finished(&mut signal).await;
    assert_eq!(output.exit_code, None);
    assert_eq!(output.termination_signal, Some(libc::SIGTERM));
}

async fn owner_fixture(root: &Path) {
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).assert_value();
    let leases = root.join("leases");
    let _lease =
        ControllerLease::acquire_workspace(&workspace_lease_path(&leases, &workspace), &workspace)
            .assert_value();
    let unrelated = File::create(root.join("unrelated.lock")).assert_value();
    unrelated.lock_exclusive().assert_value();
    let script = |name: &str| {
        format!(
            "echo $PPID > '../{name}.helper'; setsid /bin/sh -c 'echo $$ > ../{name}.pid; while :; do echo tick >> ../{name}.log; sleep 0.02; done' & wait"
        )
    };
    let (mut first, _first_source) = session(&workspace, &script("first")).await;
    let (_second, _second_source) = session(&workspace, &script("second")).await;
    ready(&root.join("first.pid")).await;
    ready(&root.join("second.pid")).await;
    ready(&root.join("stop-first")).await;
    first.release().await.assert_value();
    fs::write(root.join("first-stopped"), "done").assert_value();
    std::future::pending::<()>().await;
}
struct ResumeOnDrop(i32);
impl Drop for ResumeOnDrop {
    fn drop(&mut self) {
        // SAFETY: resuming this fixture helper lets it clean up after any assertion failure.
        unsafe {
            libc::kill(self.0, libc::SIGCONT);
        }
    }
}
async fn owner_process(root: &Path) -> Child {
    fixture_child(OWNER_TEST, root).spawn().assert_value()
}

#[tokio::test]
async fn owner_loss_preserves_workspace_until_its_tree_is_reaped() {
    if let Some(root) = std::env::var_os(FIXTURE) {
        owner_fixture(Path::new(&root)).await;
        return;
    }
    let root = tempfile::tempdir().assert_value();
    let root = root.path();
    let mut owner = owner_process(root).await;
    let first = ready(&root.join("first.pid"))
        .await
        .trim()
        .parse::<i32>()
        .assert_value();
    let second = ready(&root.join("second.pid"))
        .await
        .trim()
        .parse::<i32>()
        .assert_value();
    fs::write(root.join("stop-first"), "stop").assert_value();
    ready(&root.join("first-stopped")).await;
    assert!(!live(first), "one session left its orphan alive");
    let size = fs::metadata(root.join("second.log")).assert_value().len();
    sleep(Duration::from_millis(100)).await;
    assert!(
        live(second) && fs::metadata(root.join("second.log")).assert_value().len() > size,
        "sibling was killed by another session"
    );
    let helper = ready(&root.join("second.helper"))
        .await
        .trim()
        .parse::<i32>()
        .assert_value();
    send(helper, libc::SIGSTOP);
    let _resume = ResumeOnDrop(helper);
    owner.start_kill().assert_value();
    owner.wait().await.assert_value();
    let workspace = root.join("workspace");
    let path = workspace_lease_path(&root.join("leases"), &workspace);
    let lock = File::open(path).assert_value();
    assert!(
        lock.try_lock_exclusive().is_err(),
        "owner death released workspace before cleanup"
    );
    File::open(root.join("unrelated.lock"))
        .assert_value()
        .try_lock_exclusive()
        .assert_value_with("supervisor inherited an unrelated lock");
    send(helper, libc::SIGCONT);
    let end = Instant::now() + Duration::from_secs(5);
    while live(second) && Instant::now() < end {
        sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !live(second),
        "owner death without resume left the writer alive"
    );
    let end = Instant::now() + Duration::from_secs(5);
    while lock.try_lock_exclusive().is_err() {
        assert!(Instant::now() < end, "workspace lease survived cleanup");
        sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn fallback_closes_descriptors_above_a_lowered_limit() {
    use std::os::fd::AsRawFd;
    if let Some(root) = std::env::var_os(FIXTURE) {
        let file = File::create(Path::new(&root).join("high-fd")).assert_value();
        // SAFETY: isolated fixture process; the duplicated descriptor and reduced process limit
        // cannot affect the parent test runner or any other run.
        unsafe {
            let descriptor = libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 512);
            assert!(descriptor >= 512);
            let limit = libc::rlimit {
                rlim_cur: 64,
                rlim_max: 64,
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_NOFILE, &limit), 0);
            super::close_descriptors_from_proc(descriptor as u32, descriptor as u32 + 1);
            assert_eq!(libc::fcntl(descriptor, libc::F_GETFD), -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
        }
        std::process::exit(0);
    }
    let root = tempfile::tempdir().assert_value();
    let status = timeout(
        Duration::from_secs(10),
        fixture_child("execution::process::tree_supervisor::tests::fallback_closes_descriptors_above_a_lowered_limit", root.path()).status(),
    ).await.assert_value().assert_value();
    assert!(status.success(), "high-fd fallback child failed");
}
