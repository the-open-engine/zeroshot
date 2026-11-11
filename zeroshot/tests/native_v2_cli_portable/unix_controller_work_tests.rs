//! Cancellation must reach preparation and checkpoint tools as well as graph workers.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use openengine_cluster_protocol::{RunId, RunSubmission};
use openengine_cluster_testkit::fixture::write_executable;
use serde_json::json;
use zeroshot_engine::native_v2_admission::DeliveryPolicy;
use zeroshot_engine::native_v2_portable_controller::{
    PortableControllerBootstrap, write_bootstrap_file,
};
use zeroshot_engine::native_v2_supervisor::RunEnvironment;

use super::{Duration, File, FileExt, Fixture, Instant, Run, Value, process_exists, success};

struct NativeWork {
    process: u32,
    child: u32,
}

impl NativeWork {
    fn wait_started(marker: &Path) -> Self {
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(text) = std::fs::read_to_string(marker) {
                let pids = text
                    .split_whitespace()
                    .map(str::parse::<u32>)
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                if let [process, child] = pids.as_slice() {
                    return Self {
                        process: *process,
                        child: *child,
                    };
                }
            }
            assert!(
                Instant::now() < end,
                "native work never started: {}",
                marker.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_stopped(&self) {
        assert!(
            !process_exists(self.process),
            "native tool outlived controller ownership"
        );
        assert!(
            !process_exists(self.child),
            "native tool child outlived controller ownership"
        );
    }
}

impl Drop for NativeWork {
    fn drop(&mut self) {
        // A regression must not leave this test's private process group running.
        if process_exists(self.process) || process_exists(self.child) {
            unsafe {
                libc::kill(-(self.process as i32), libc::SIGKILL);
            }
        }
    }
}

fn blocking_tool(operation: &str, marker: &Path, fallback: &str) -> String {
    format!(
        "#!/bin/sh\nfor arg do\n  if [ \"$arg\" = '{operation}' ]; then\n    /bin/sleep 90 &\n    printf '%s %s' \"$$\" \"$!\" > '{}'\n    wait\n    exit\n  fi\ndone\n{fallback}\n",
        marker.display(),
    )
}

#[test]
fn unix_controller_sigterm_interrupts_checkpoint_backup_before_releasing_ownership() {
    let fixture = Fixture::new();
    let marker = fixture.root.path("backup.pids");
    let wrapper = fixture.root.path("restic-wrapper");
    let restic = Path::new(env!("CARGO_BIN_EXE_zeroshot")).with_file_name("restic");
    assert!(
        restic.is_file(),
        "prepare the pinned Restic sidecar before this test"
    );
    write_executable(
        &wrapper,
        blocking_tool(
            "backup",
            &marker,
            &format!("exec '{}' \"$@\"", restic.display()),
        ),
        0o700,
    )
    .unwrap();
    let output = fixture
        .run_command("block", true)
        .env("ZEROSHOT_RESTIC", &wrapper)
        .output()
        .unwrap();
    success(&output);
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let work = NativeWork::wait_started(&marker);
    let run = Run::from_started(
        fixture,
        receipt["runId"].as_str().unwrap().to_owned(),
        work.child,
    );
    // The first checkpoint blocks before any worker can start.
    assert!(!run.fixture.workspace.join("mutation.txt").exists());
    run.signal(libc::SIGTERM);
    run.wait_closed(Duration::from_secs(15));
    work.assert_stopped();
    assert_eq!(run.terminal()["reason"], "force_stopped");
    assert!(run.storage.join("checkpoints").is_dir());
    assert_eq!(
        std::fs::read_to_string(run.fixture.workspace.join("seed")).unwrap(),
        "seed"
    );
}

#[test]
fn unix_controller_sigterm_interrupts_checkpoint_git_capture_before_releasing_ownership() {
    let search = std::env::var_os("PATH").unwrap();
    let git = std::env::split_paths(&search)
        .map(|directory| directory.join("git"))
        .find(|path| path.is_file())
        // Keep the invoked name: installed Git may be a dispatcher symlink keyed by argv[0].
        .map(|path| std::path::absolute(path).unwrap())
        .unwrap();
    // Both the captured HEAD and configuration normalization start native Git commands. Blocking
    // only --git-dir/--file commands lets CLI source admission finish before the snapshot starts.
    for operation in ["rev-parse", "config"] {
        let fixture = Fixture::new();
        let marker = fixture.root.path("capture.pids");
        let bin = fixture.root.path("capture-bin");
        std::fs::create_dir(&bin).unwrap();
        let fallback = format!("exec '{}' \"$@\"", git.display());
        let script = blocking_tool(operation, &marker, &fallback).replacen(
            "#!/bin/sh\n",
            &format!("#!/bin/sh\ncapture=false\nfor arg do\n  case \"$arg\" in --git-dir|--file) capture=true ;; esac\ndone\nif [ \"$capture\" != true ]; then {fallback}; fi\n"),
            1,
        );
        write_executable(&bin.join("git"), script, 0o700).unwrap();
        let mut command = fixture.run_command("block", true);
        let old_path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .unwrap()
            .1
            .unwrap()
            .to_owned();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&old_path));
        let output = command
            .env("PATH", std::env::join_paths(paths).unwrap())
            .output()
            .unwrap();
        success(&output);
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut run = Run::from_started(fixture, receipt["runId"].as_str().unwrap().to_owned(), 0);
        let work = NativeWork::wait_started(&marker);
        run.child = work.child;
        assert!(!run.fixture.workspace.join("mutation.txt").exists());
        run.signal(libc::SIGTERM);
        run.wait_closed(Duration::from_secs(15));
        work.assert_stopped();
        assert_eq!(
            run.terminal()["reason"],
            "force_stopped",
            "capture stage {operation}"
        );
        assert!(run.storage.join("checkpoints").is_dir());
        assert_eq!(
            std::fs::read_to_string(run.fixture.workspace.join("seed")).unwrap(),
            "seed"
        );
    }
}

struct ControllerChild(Child);

impl Drop for ControllerChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn unix_controller_sigterm_interrupts_startup_review_fetch_before_releasing_workspace() {
    let fixture = Fixture::new();
    let marker = fixture.root.path("fetch.pids");
    let bin = fixture.root.path("review-bin");
    std::fs::create_dir(&bin).unwrap();
    write_executable(
        &bin.join("git"),
        blocking_tool("fetch", &marker, "exit 0"),
        0o700,
    )
    .unwrap();
    let bootstrap = review_bootstrap(&fixture, &bin);
    let bootstrap_path = fixture.root.path("review-bootstrap.json");
    write_bootstrap_file(&bootstrap_path, &bootstrap).unwrap();
    let mut controller = ControllerChild(
        Command::new(env!("CARGO_BIN_EXE_zeroshot"))
            .args([
                zeroshot_engine::native_v2_cli::local::LOCAL_CONTROLLER_MODE,
                "--bootstrap",
            ])
            .arg(bootstrap_path)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let work = NativeWork::wait_started(&marker);
    let lease = File::open(&bootstrap.workspace_lease).unwrap();
    assert!(lease.try_lock_exclusive().is_err());
    assert_eq!(
        unsafe { libc::kill(controller.0.id() as i32, libc::SIGTERM) },
        0
    );
    let end = Instant::now() + Duration::from_secs(15);
    while controller.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < end, "startup cancellation did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
    work.assert_stopped();
    lease.try_lock_exclusive().unwrap();
    assert!(!bootstrap.storage.join("controller.ready.json").exists());
    let ledger = rusqlite::Connection::open(bootstrap.storage.join("runs.sqlite3")).unwrap();
    assert_eq!(
        ledger
            .query_row("SELECT count(*) FROM v2_runs", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("seed")).unwrap(),
        "seed"
    );
}

fn review_bootstrap(fixture: &Fixture, bin: &Path) -> PortableControllerBootstrap {
    let graph = fixture.json(&["template", "show", "issue-review", "--json"]);
    let submission: RunSubmission = serde_json::from_value(json!({
        "title":"Interrupt review checkout", "submissionKey":"review-stop",
        "graph":graph,
        "initialInput":{
            "url":"https://github.com/acme/native-fixture/pull/17",
            "context":{"title":"Review", "body":"Changes", "reviewScope":{
                "baseRepository":"acme/native-fixture", "baseBranch":"main", "baseRevision":"b".repeat(40),
                "headRepository":"acme/native-fixture", "headBranch":"feature", "headRevision":"a".repeat(40)
            }}
        },
        "source":{"repository":"acme/native-fixture", "branch":"main", "revision":"a".repeat(40)},
        "runtime":{"harness":"codex", "provider":"openai", "size":"host", "workspaceCheckpoints":false,
            "activeMemory":{"repository":false,"graph":false},
            "nodes":{"reviewer":{"kind":"agent", "model":"fixture"}, "deliver":{"kind":"comment_delivery"}}}
    })).unwrap();
    PortableControllerBootstrap {
        checkpoint: None,
        preparation_timeout: Duration::from_secs(120),
        run_id: RunId::new("review-stop"),
        delivery_run_id: RunId::new("review-stop"),
        adopt_existing_delivery: false,
        environment: RunEnvironment::exact_for_submission(&submission, BTreeMap::new()).unwrap(),
        submission,
        native_environment: BTreeMap::from([
            ("PATH".to_owned(), bin.to_str().unwrap().to_owned()),
            (
                "HOME".to_owned(),
                fixture.root.path("").to_str().unwrap().to_owned(),
            ),
        ]),
        github_token: Some("fixture-review-token".to_owned()),
        github_token_file: None,
        delivery_validator: None,
        git_credentials: None,
        workspace: fixture.workspace.clone(),
        workspace_lease: fixture.root.path("review-workspace.lock"),
        checkpoint_repository: fixture.root.path("review-checkpoints"),
        memory_storage: None,
        storage: fixture.root.path("review-state"),
        delivery_policy: DeliveryPolicy::Optional,
    }
}
