use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use openengine_cluster_testkit::assertions::AssertValue;
use tokio::sync::watch;

use super::*;
use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::{HostedProcessPool, HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_capsule::provider_process::ProviderProcess;

struct Fixture {
    _directory: TestDirectory,
    candidate: PathBuf,
    runtime: PathBuf,
    cancellation: watch::Sender<bool>,
}

impl Fixture {
    fn new() -> Self {
        let directory = TestDirectory::new("provider-filesystem");
        let candidate = directory.child("workspace");
        let runtime = directory.child("runtime");
        fs::create_dir(&candidate).assert_value();
        fs::create_dir(&runtime).assert_value();
        let (cancellation, _) = watch::channel(false);
        Self {
            _directory: directory,
            candidate,
            runtime,
            cancellation,
        }
    }

    fn specification(&self, execution: u64) -> ExecutionFilesystemSpec {
        let home = self.runtime.join(format!("home-{execution}"));
        create_private_directory(&home).assert_value();
        ExecutionFilesystemSpec {
            runner: LocalProcessRunner::new(),
            home: Arc::new(PrivateDirectory::retained(home)),
            root: self.runtime.join(format!("execution-{execution}")),
            candidate: self.candidate.clone(),
            identity: None,
            cancellation: DriverCancellation::new(self.cancellation.subscribe()),
        }
    }

    fn prepare(&self, execution: u64) -> Arc<ProviderExecutionFiles> {
        Arc::new(ProviderExecutionFiles::prepare(self.specification(execution)).assert_value())
    }
}

#[test]
fn boundary_contract_execution_files_expose_utf8_scratch_and_return_to_idle_after_confirmed_reap() {
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new();
    let files = fixture.prepare(1);
    assert!(files.scratch_text().is_ok());
    files.begin_process();
    assert!(files.ensure_idle().is_err());
    files.process_reaped();
    files.ensure_idle().assert_value();

    let invalid = ProviderExecutionFiles {
        runner: LocalProcessRunner::new(),
        workspace: fixture.candidate.clone(),
        scratch: PathBuf::from(std::ffi::OsString::from_vec(vec![0xff])),
        home: Arc::new(PrivateDirectory::retained(
            fixture.runtime.join("invalid-home"),
        )),
        root: PrivateDirectory::retained(fixture.runtime.join("invalid-root")),
    };
    assert!(matches!(
        invalid.scratch_text(),
        Err(ProcessRunnerError::InvalidCommand(detail)) if detail.contains("not valid UTF-8")
    ));
}

#[test]
fn boundary_contract_private_directory_drop_removes_only_idle_directories() {
    let fixture = Fixture::new();
    let file = fixture.runtime.join("retained-file");
    fs::write(&file, b"keep").assert_value();
    drop(PrivateDirectory::retained(file.clone()));
    assert!(file.is_file());

    let active = fixture.runtime.join("active");
    fs::create_dir(&active).assert_value();
    let directory = PrivateDirectory::retained(active.clone());
    directory.processes.store(1, Ordering::Release);
    drop(directory);
    assert!(active.is_dir());
}

#[test]
fn each_execution_shares_workspace_artifacts_and_observes_edits() {
    let fixture = Fixture::new();
    fs::write(fixture.candidate.join("source"), "version one").assert_value();
    let first = fixture.prepare(1);
    fs::write(first.workspace.join("source"), "verifier edit").assert_value();
    fs::write(first.workspace.join("private-cache"), "private").assert_value();
    drop(first);
    fs::write(fixture.candidate.join("source"), "version two").assert_value();
    let second = fixture.prepare(2);
    assert_eq!(
        fs::read(second.workspace.join("source")).assert_value(),
        b"version two"
    );
    assert!(second.workspace.join("private-cache").exists());
    assert!(fixture.candidate.join("private-cache").exists());
}

#[tokio::test]
async fn session_home_survives_executions_until_close_and_outstanding_resources_finish() {
    let fixture = Fixture::new();
    let core = ProviderSessionCore::new();
    let mut specification = fixture.specification(1);
    let home = fixture.runtime.join("session-home");
    create_private_directory(&home).assert_value();
    specification.home = core.retain_home(home.clone()).await.assert_value();
    let files = ProviderExecutionFiles::prepare(specification).assert_value();
    drop(files);
    assert!(home.exists());
    let mut next = fixture.specification(2);
    next.home = core.retain_home(home.clone()).await.assert_value();
    let files = ProviderExecutionFiles::prepare(next).assert_value();
    core.close().await;
    assert!(home.exists());
    drop(files);
    assert!(!home.exists());
}

#[test]
fn unconfirmed_process_cleanup_retains_files_and_rejects_another_provider_launch() {
    let fixture = Fixture::new();
    let files = fixture.prepare(1);
    let execution = files.root.path.clone();
    let home = files.home.path.clone();
    files.begin_process();
    assert!(files.ensure_idle().is_err());
    drop(files);
    assert!(execution.exists());
    assert!(home.exists());
}

fn command(files: &ProviderExecutionFiles, script: &str) -> ProcessSessionCommand {
    ProcessSessionCommand {
        program: "/bin/sh".to_owned(),
        argv: vec!["-c".to_owned(), script.to_owned()],
        environment: BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), files.home().display().to_string()),
            ("TMPDIR".to_owned(), files.scratch_text().assert_value()),
        ]),
        workspace: WorkspaceCapability {
            current_dir: files.workspace.clone(),
            mode: WorkspaceAccessMode::ReadOnly,
        },
        deadline: Some(tokio::time::Instant::now() + Duration::from_secs(20)),
    }
}

async fn open(
    files: Arc<ProviderExecutionFiles>,
    command: ProcessSessionCommand,
    cancellation: &watch::Sender<bool>,
) -> ProviderProcess {
    let process = files
        .runner
        .open(command, DriverCancellation::new(cancellation.subscribe()))
        .await
        .assert_value();
    files.begin_process();
    ProviderProcess::new(process, files)
}

#[tokio::test]
async fn nonzero_exit_and_cancelled_process_tree_release_execution_files() {
    let fixture = Fixture::new();
    for (execution, script) in [(1, "exit 17"), (2, "sleep 30 & printf ready; wait")] {
        let files = fixture.prepare(execution);
        let mut process = open(
            files.clone(),
            command(&files, script),
            &fixture.cancellation,
        )
        .await;
        if execution == 2 {
            assert!(process.recv_stdout().await.is_some());
            fixture.cancellation.send_replace(true);
        }
        let output = process.wait().await.assert_value();
        assert!(output.cleanup.proves_tree_empty());
        assert_eq!(output.cancelled, execution == 2);
        if execution == 1 {
            assert_eq!(output.exit_code, Some(17));
        }
        drop(process);
        let home = files.home.path.clone();
        let execution = files.root.path.clone();
        drop(files);
        assert!(!execution.exists());
        assert!(!home.exists());
    }
}

#[test]
fn shared_cargo_target_reuses_compiled_artifacts_and_build_script_outputs() {
    let fixture = Fixture::new();
    fs::write(
        fixture.candidate.join("Cargo.toml"),
        concat!(
            "[package]\nname = \"copied-target-fixture\"\n",
            "version = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n",
        ),
    )
    .assert_value();
    fs::create_dir(fixture.candidate.join("src")).assert_value();
    fs::write(
        fixture.candidate.join("src/main.rs"),
        concat!(
            "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n",
            "fn main() { assert_eq!(answer(), 42); }\n",
        ),
    )
    .assert_value();
    fs::write(
        fixture.candidate.join("build.rs"),
        r#"fn main() {
        println!("cargo:rerun-if-changed=build.rs");
        let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        std::fs::write(output.join("generated.rs"), "fn answer() -> u32 { 42 }").unwrap();
    }"#,
    )
    .assert_value();
    let original = build_cargo_fixture(&fixture.candidate, &fixture.runtime);
    assert!(String::from_utf8_lossy(&original.stderr).contains("Compiling copied-target-fixture"));
    let files = fixture.prepare(1);
    let copied = build_cargo_fixture(&files.workspace, &files.scratch);
    let diagnostics = String::from_utf8_lossy(&copied.stderr);
    assert!(
        diagnostics.contains("Fresh copied-target-fixture"),
        "{diagnostics}"
    );
    assert!(
        !diagnostics.contains("Compiling copied-target-fixture"),
        "{diagnostics}"
    );
    assert!(
        std::process::Command::new(files.workspace.join("target/debug/copied-target-fixture"))
            .status()
            .assert_value()
            .success()
    );
}

fn build_cargo_fixture(workspace: &Path, scratch: &Path) -> std::process::Output {
    let output = std::process::Command::new("cargo")
        .args(["build", "--offline", "--verbose"])
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", workspace.join("target"))
        .env("CARGO_TERM_COLOR", "never")
        .env("TMPDIR", scratch)
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[tokio::test]
async fn root_workers_and_reviewers_share_files_and_cleanup_independently() {
    // SAFETY: geteuid only inspects the current process identity.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only concurrent writer gate skipped outside the capsule identity");
        return;
    }
    let fixture = Fixture::new();
    let pool = HostedProcessPool::new(91_002, 91_002, 92_000).assert_value();
    crate::native_v2_capsule::prepare_capsule_filesystem(
        crate::native_v2_capsule::CapsuleFilesystemSpec {
            workspace: &fixture.candidate,
            runtime_home: &fixture.runtime,
            process_pool: pool,
        },
    )
    .assert_value();
    initialize_writer_repository(&fixture, pool);
    let left_identity = pool
        .identity(HostedProcessScope::WriterExecution(1))
        .assert_value();
    let right_identity = pool
        .identity(HostedProcessScope::VerifierExecution(2))
        .assert_value();
    assert_eq!(left_identity.uid(), right_identity.uid());
    let left = shared_writer_files(&fixture, pool, 1);
    let right = shared_writer_files(&fixture, pool, 2);
    let left_script = "set -eu\n\
        mkdir shared\n\
        printf left > shared/left\n\
        printf ready\n\
        read finish\n\
        test -f shared/right\n\
        git status --porcelain >/dev/null";
    let mut left_process = open(
        left.clone(),
        command(&left, left_script),
        &fixture.cancellation,
    )
    .await;
    assert!(left_process.recv_stdout().await.is_some());
    let (right_cancel, _) = watch::channel(false);
    let right_script = "set -eu\n\
        printf right > shared/right\n\
        chmod +x shared/left\n\
        printf '+right' >> shared/left\n\
        git status --porcelain >/dev/null\n\
        printf ready\n\
        read finish";
    let mut right_process = open(right.clone(), command(&right, right_script), &right_cancel).await;
    assert!(right_process.recv_stdout().await.is_some());
    right_cancel.send_replace(true);
    let stopped = right_process.wait().await.assert_value();
    assert!(stopped.cancelled);
    assert!(stopped.cleanup.proves_tree_empty());
    drop(right_process);
    let right_root = right.root.path.clone();
    drop(right);
    assert!(!right_root.exists());
    assert!(left.root.path.exists());
    left_process
        .send(crate::execution::process::ProcessFrame::new(b"finish\n".to_vec()).assert_value())
        .await
        .assert_value();
    left_process.close_stdin().await.assert_value();
    let completed = left_process.wait().await.assert_value();
    assert_eq!(completed.exit_code, Some(0));
    assert!(!completed.cancelled);
    assert!(completed.cleanup.proves_tree_empty());
    assert_eq!(
        fs::read_to_string(fixture.candidate.join("shared/left")).assert_value(),
        "left+right"
    );
    assert_ne!(
        fs::metadata(fixture.candidate.join("shared/left"))
            .assert_value()
            .permissions()
            .mode()
            & 0o100,
        0
    );
    assert_eq!(
        fs::read_to_string(fixture.candidate.join("shared/right")).assert_value(),
        "right"
    );
}

fn shared_writer_files(
    fixture: &Fixture,
    pool: HostedProcessPool,
    execution: u64,
) -> Arc<ProviderExecutionFiles> {
    let identity = pool
        .identity(if execution == 2 {
            HostedProcessScope::VerifierExecution(execution)
        } else {
            HostedProcessScope::WriterExecution(execution)
        })
        .assert_value();
    let mut specification = fixture.specification(execution);
    specification.runner = identity.runner();
    specification.identity = Some(identity);
    set_owner(specification.home.path(), Some(identity)).assert_value();
    Arc::new(ProviderExecutionFiles::prepare(specification).assert_value())
}

fn initialize_writer_repository(fixture: &Fixture, pool: HostedProcessPool) {
    use std::os::unix::process::CommandExt;
    let owner = pool.identity(HostedProcessScope::Writer).assert_value();
    let output = std::process::Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .current_dir(&fixture.candidate)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .uid(owner.uid())
        .gid(owner.gid())
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
