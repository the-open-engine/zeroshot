use super::*;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const MODE: &str = "ZEROSHOT_WINDOWS_DEPARTED_TEST_MODE";
const ROOT: &str = "ZEROSHOT_WINDOWS_DEPARTED_TEST_ROOT";
const TEST: &str = "execution::process::authority::tests::windows::departed::departed_launcher_preserves_only_verified_outside_job_human_callers";

fn probe_command(root: &Path, mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().assert_value());
    command
        .args(["--exact", TEST, "--nocapture"])
        .env(MODE, mode)
        .env(ROOT, root)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

fn wait_until(mut condition: impl FnMut() -> bool, deadline: Instant, description: &str) {
    while !condition() {
        assert!(Instant::now() < deadline, "{description} timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn launcher_status(child: &mut Child, deadline: Instant) -> ExitStatus {
    loop {
        if let Some(status) = child.try_wait().assert_value() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("departed-owner launcher timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn launch_probe(root: &Path) {
    let child = probe_command(root, "probe").spawn().assert_value();
    std::fs::write(root.join("probe-pid"), child.id().to_string()).assert_value();
    // Release the probe process handle and exit, so its native parent really departs.
    drop(child);
}

fn run_probe(root: &Path) {
    // Without a registry the human check returns early, making this regression vacuous.
    drop(Registration::prepare().assert_value());
    let caller = native::process_identity(std::process::id()).assert_value();
    std::fs::write(root.join("ready"), []).assert_value();
    wait_until(
        || root.join("release").exists() || !root.exists(),
        Instant::now() + Duration::from_secs(60),
        "departed-owner probe release",
    );
    if !root.exists() {
        return;
    }
    assert_eq!(
        native::process_identity(caller.parent).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    let outside = native::outside_jobs(caller).assert_value();
    let result = require_human(caller.pid);
    let report = serde_json::json!({
        "pid": caller.pid,
        "parent": caller.parent,
        "outsideJobs": outside,
        "allowed": result.is_ok(),
        "error": result.err().map(|error| error.to_string())
    });
    std::fs::write(
        root.join("result.pending"),
        serde_json::to_vec(&report).assert_value(),
    )
    .assert_value();
    std::fs::rename(root.join("result.pending"), root.join("result.json")).assert_value();
}

fn check_departed_launcher() {
    let root = tempfile::tempdir().assert_value();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut launcher = probe_command(root.path(), "launcher")
        .spawn()
        .assert_value();
    let parent = launcher.id();
    let status = launcher_status(&mut launcher, deadline);
    drop(launcher);
    assert!(status.success(), "departed-owner launcher failed: {status}");
    wait_until(
        || match native::process_identity(parent) {
            Ok(_) => false,
            Err(error) => {
                assert_eq!(error.kind(), io::ErrorKind::NotFound);
                true
            }
        },
        deadline,
        "native launcher departure",
    );
    wait_until(
        || root.path().join("ready").exists(),
        deadline,
        "probe readiness",
    );
    std::fs::write(root.path().join("release"), []).assert_value();
    wait_until(
        || root.path().join("result.json").exists(),
        deadline,
        "probe result",
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("result.json")).assert_value())
            .assert_value();
    let probe: u32 = std::fs::read_to_string(root.path().join("probe-pid"))
        .assert_value()
        .parse()
        .assert_value();
    assert_eq!(report["pid"], probe);
    assert_eq!(report["parent"], parent);
    assert_eq!(report["allowed"], report["outsideJobs"], "{report}");
    println!("departed-owner native Job boundary: {report}");
}

#[test]
fn departed_launcher_preserves_only_verified_outside_job_human_callers() {
    match std::env::var(MODE).ok().as_deref() {
        Some("launcher") => launch_probe(&PathBuf::from(std::env::var_os(ROOT).assert_value())),
        Some("probe") => run_probe(&PathBuf::from(std::env::var_os(ROOT).assert_value())),
        None => check_departed_launcher(),
        Some(mode) => panic!("unknown departed-owner fixture mode: {mode}"),
    }
}
