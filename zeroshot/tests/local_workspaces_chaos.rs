//! Adversarial local folder admission through the shipped CLI and real child processes.
#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};

use openengine_cluster_testkit::TemporaryDirectory;
use serde_json::{Value, json};

#[path = "support/local_graph.rs"]
mod local_graph;

struct WorkspaceFixture {
    root: TemporaryDirectory,
    launch: PathBuf,
    workspace: PathBuf,
    bin: PathBuf,
    state: PathBuf,
}

impl WorkspaceFixture {
    fn new() -> Self {
        let root = TemporaryDirectory::for_test("wc");
        let launch = root.path("launcher");
        let workspace = root.path("workspace space-é");
        let bin = root.path("bin");
        let state = root.path("state");
        for directory in [&launch, &workspace, &bin] {
            fs::create_dir(directory).unwrap();
        }
        let output = Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        success(&output);
        let node = String::from_utf8(output.stdout).unwrap();
        let harness =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native_v2_cli_portable/harness.cjs");
        openengine_cluster_testkit::fixture::write_executable(
            &bin.join("codex"),
            format!(
                "#!/bin/sh\ncase \"$*\" in *app-server*) ;; *) /bin/pwd >> {} ;; esac\nexec {} {} \"$@\"\n",
                quote(&root.path("provider-starts.txt").to_string_lossy()),
                quote(node.trim()),
                quote(&harness.to_string_lossy())
            ),
            0o700,
        )
        .unwrap();
        write_json(&launch.join("graph.json"), &local_graph::graph());
        write_json(&launch.join("input.json"), &Value::Null);
        write_json(
            &launch.join("runtime.json"),
            // Workspace admission and recovery assertions count only graph worker calls.
            &json!({"harness":"codex","provider":"openai","size":"small",
                "activeMemory":{"repository":false,"graph":false},
                "nodes":{"worker":{"kind":"agent","model":"opaque/test-model:owned@v1",
                    "connections":{"fixture":["OPENAI_API_KEY","FIXTURE_MODE"]}}}}),
        );
        Self {
            root,
            launch,
            workspace,
            bin,
            state,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
        command
            .args(args)
            .current_dir(&self.launch)
            .env_clear()
            .env("PATH", &self.bin)
            .env("ZEROSHOT_STATE_DIR", &self.state)
            .env("ZEROSHOT_CONFIG_DIR", self.root.path("config"))
            .env("OPENAI_API_KEY", "fixture-key")
            .env("UNDECLARED_SECRET", "must-not-inherit");
        command
    }

    fn submission(&self, workspace: &Path, mode: &str, extra: &[&str]) -> Command {
        let mut command = self.command(&[
            "run",
            "--title",
            "Workspace admission chaos",
            "--graph",
            "graph.json",
            "--input",
            "input.json",
            "--runtime-config",
            "runtime.json",
        ]);
        command
            .arg("--cwd")
            .arg(workspace)
            .args(extra)
            .env("FIXTURE_MODE", mode);
        command
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.command(args).output().unwrap();
        success(&output);
        let text = String::from_utf8(output.stdout).expect("CLI JSON is UTF-8");
        serde_json::from_str(&text).expect("CLI returned valid JSON")
    }

    fn initialize_git(&self, committed: bool, detached: bool) {
        git(&self.workspace, &["init", "-b", "main"]);
        if committed {
            fs::write(self.workspace.join("seed"), "committed").unwrap();
            git(&self.workspace, &["add", "seed"]);
            git(&self.workspace, &["commit", "-m", "seed"]);
        }
        if detached {
            git(&self.workspace, &["checkout", "--detach"]);
        }
    }

    fn assert_no_execution(&self) {
        assert!(!self.state.join("runs").exists());
        assert!(!self.root.path("provider-starts.txt").exists());
        assert!(!self.workspace.join("mutation.txt").exists());
    }

    fn provider_starts(&self) -> String {
        fs::read_to_string(self.root.path("provider-starts.txt")).unwrap()
    }

    fn failed_folder_run(&self) -> String {
        let mut graph = local_graph::graph();
        graph["root"]["children"][1]["otherwise"] = json!({
            "kind":"fail", "name":"chaos_failure", "reason":"chaos_failure"
        });
        write_json(&self.launch.join("graph.json"), &graph);
        let failed = self
            .submission(&self.workspace, "finish", &[])
            .output()
            .unwrap();
        assert!(
            !failed.status.success(),
            "authored failure unexpectedly succeeded"
        );
        let original_id = receipt(&failed)["runId"].as_str().unwrap().to_owned();
        assert_eq!(
            self.json(&["status", &original_id])["status"]["terminalResult"]["reason"],
            "chaos_failure"
        );
        original_id
    }

    fn finished_status(&self, run_id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let status = self.json(&["status", run_id]);
            if status["status"]["phase"] == "finished" {
                return status;
            }
            assert!(Instant::now() < deadline, "resumed folder did not settle");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_overlap_rejected(&self, workspace: &Path) {
        let output = self
            .submission(
                workspace,
                "block",
                &["--detach", "--submission-key", "overlapping-folder-key"],
            )
            .output()
            .unwrap();
        if output.status.success() {
            let run_id = receipt(&output)["runId"].as_str().unwrap().to_owned();
            StopRun {
                fixture: self,
                run_id,
                stopped: false,
            }
            .stop();
            panic!("overlapping workspace admitted: {}", workspace.display());
        }
        let starts = self.provider_starts();
        assert_eq!(
            starts.lines().count(),
            1,
            "rejected workspace executed a worker"
        );
    }

    fn dormant_delivery(&self, mode: &str) {
        let rendered = self
            .command(&["template", "show", "software-change", "--delivery", mode])
            .output()
            .unwrap();
        success(&rendered);
        let template: Value = serde_json::from_slice(&rendered.stdout).unwrap();
        let mut delivery = delivery_node(&template).unwrap().clone();
        delivery["input"] = json!({"kind":"null"});
        delivery["inputBindings"] = json!([]);
        delivery["writeBindings"] = json!([]);
        let mut graph = local_graph::graph();
        graph["root"]["children"][1]["branches"][0]["node"] = json!({
            "kind":"seq", "name":"nested_delivery", "state":{"kind":"null"},
            "children":[delivery, {
                "kind":"succeed", "name":"delivered", "output":{"kind":"null"}, "bindings":[]
            }], "promotedStatePaths":[]
        });
        write_json(&self.launch.join("graph.json"), &graph);
        let runtime_path = self.launch.join("runtime.json");
        let mut runtime: Value = serde_json::from_slice(&fs::read(&runtime_path).unwrap()).unwrap();
        runtime["nodes"]["deliver"] = json!({"kind":"git_delivery"});
        write_json(&runtime_path, &runtime);
    }

    fn start_blocked(&self, workspace: &Path, key: &str) -> StopRun<'_> {
        let output = self
            .submission(workspace, "block", &["--detach", "--submission-key", key])
            .output()
            .unwrap();
        success(&output);
        let run_id = receipt(&output)["runId"].as_str().unwrap().to_owned();
        let guard = StopRun {
            fixture: self,
            run_id,
            stopped: false,
        };
        wait_file(&workspace.join("child.pid"));
        guard
    }
}

struct StopRun<'a> {
    fixture: &'a WorkspaceFixture,
    run_id: String,
    stopped: bool,
}

impl StopRun<'_> {
    fn stop(mut self) {
        let output = self
            .fixture
            .command(&["force-stop", &self.run_id])
            .output()
            .unwrap();
        success(&output);
        self.stopped = true;
    }
}

impl Drop for StopRun<'_> {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.fixture.command(&["force-stop", &self.run_id]).output();
        }
    }
}

fn delivery_node(value: &Value) -> Option<&Value> {
    if value
        .get("worker")
        .and_then(Value::as_str)
        .is_some_and(|worker| worker.starts_with("builtin.git-delivery"))
    {
        return Some(value);
    }
    match value {
        Value::Object(fields) => fields.values().find_map(delivery_node),
        Value::Array(items) => items.iter().find_map(delivery_node),
        _ => None,
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn receipt(output: &Output) -> Value {
    serde_json::from_slice(output.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap()
}

fn wait_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "fixture never produced {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn git(workspace: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(workspace)
        .args([
            "-c",
            "user.name=Workspace chaos",
            "-c",
            "user.email=chaos@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    success(&output);
}

#[test]
fn plain_folder_without_git_uses_launcher_files_and_reports_absent_source() {
    let fixture = WorkspaceFixture::new();
    for name in ["graph.json", "input.json", "runtime.json"] {
        fs::write(
            fixture.workspace.join(name),
            "poisoned selected-folder argument",
        )
        .unwrap();
    }
    let relative_workspace = Path::new("../workspace space-é");
    let output = fixture
        .submission(relative_workspace, "finish", &[])
        .output()
        .unwrap();
    success(&output);
    let run_id = receipt(&output)["runId"].as_str().unwrap().to_owned();
    let status = fixture.json(&["status", &run_id]);
    assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
    assert!(
        status["source"].is_null(),
        "plain folder acquired a source: {status}"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("mutation.txt")).unwrap(),
        "native worker ran"
    );
    assert!(!fixture.launch.join("mutation.txt").exists());
    assert!(!fixture.workspace.join(".git").exists());
    assert!(
        fixture.json(&["list"])["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["runId"] == run_id && run["source"].is_null())
    );
}

#[test]
fn workspace_state_overlap_and_missing_symlink_leaf_fail_before_run_storage() {
    use std::os::unix::fs::symlink;

    for relation in ["equal", "state-inside", "workspace-inside", "alias-missing"] {
        let fixture = WorkspaceFixture::new();
        let (workspace, state) = match relation {
            "equal" => (fixture.workspace.clone(), fixture.workspace.clone()),
            "state-inside" => (
                fixture.workspace.clone(),
                fixture.workspace.join("missing/state"),
            ),
            "workspace-inside" => (fixture.workspace.clone(), fixture.root.as_path().to_owned()),
            _ => {
                let alias = fixture.root.path("workspace-alias");
                symlink(&fixture.workspace, &alias).unwrap();
                (fixture.workspace.clone(), alias.join("missing/../state"))
            }
        };
        let output = fixture
            .submission(&workspace, "finish", &["--validate-only"])
            .env("ZEROSHOT_STATE_DIR", &state)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{relation} overlap admitted");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("ZEROSHOT_STATE_DIR"), "{relation}: {error}");
        assert!(!state.join("runs").exists());
        assert!(!workspace.join("mutation.txt").exists());
        assert!(!fixture.state.exists());
    }
}

#[test]
fn submission_keys_and_symlink_aliases_cannot_reconcile_to_another_folder() {
    use std::os::unix::fs::symlink;

    let fixture = WorkspaceFixture::new();
    let first = fixture.start_blocked(&fixture.workspace, "same-folder-key");
    let alias = fixture.root.path("workspace-alias");
    symlink(&fixture.workspace, &alias).unwrap();
    let retry = fixture
        .submission(
            &alias,
            "block",
            &["--detach", "--submission-key", "same-folder-key"],
        )
        .output()
        .unwrap();
    success(&retry);
    assert_eq!(receipt(&retry)["runId"], first.run_id);

    let other = fixture.root.path("other-folder");
    fs::create_dir(&other).unwrap();
    let second = fixture
        .submission(&other, "finish", &["--submission-key", "same-folder-key"])
        .output()
        .unwrap();
    if second.status.success() {
        assert_ne!(
            receipt(&second)["runId"],
            first.run_id,
            "other folder reused first run"
        );
        assert!(
            other.join("mutation.txt").is_file(),
            "other folder was not executed"
        );
    } else {
        let error = String::from_utf8_lossy(&second.stderr);
        assert!(
            error.to_lowercase().contains("submission"),
            "unexpected rejection: {error}"
        );
        assert!(!other.join("mutation.txt").exists());
    }
    let other_run = fixture.start_blocked(&other, "independent-folder-key");
    assert_ne!(first.run_id, other_run.run_id);
    assert_eq!(
        fixture.json(&["status", &first.run_id])["status"]["phase"],
        "running"
    );
    assert_eq!(
        fixture.json(&["status", &other_run.run_id])["status"]["phase"],
        "running"
    );
    other_run.stop();
    first.stop();
}

#[test]
fn remoteless_unborn_and_detached_repositories_normalize_cwd_without_delivery() {
    for mode in ["unborn", "detached", "remoteless"] {
        let fixture = WorkspaceFixture::new();
        fixture.initialize_git(mode != "unborn", mode == "detached");
        let nested = fixture.workspace.join("selected/nested");
        fs::create_dir_all(&nested).unwrap();
        let output = fixture
            .submission(&nested, "finish", &[])
            .env("PATH", format!("{}:/usr/bin:/bin", fixture.bin.display()))
            .output()
            .unwrap();
        success(&output);
        assert!(
            fixture.workspace.join("mutation.txt").is_file(),
            "{mode} did not normalize Git root"
        );
        assert!(!nested.join("mutation.txt").exists());
    }
}

#[test]
fn plain_folder_force_stop_closes_worker_descendants_and_retains_recovery() {
    let fixture = WorkspaceFixture::new();
    let held = fixture.start_blocked(&fixture.workspace, "stop-folder-key");
    let run_id = held.run_id.clone();
    let child: i32 = fs::read_to_string(fixture.workspace.join("child.pid"))
        .unwrap()
        .parse()
        .unwrap();
    held.stop();
    let status = fixture.json(&["status", &run_id]);
    assert_eq!(status["status"]["phase"], "finished");
    assert!(status["source"].is_null());
    assert!(
        status["workspaceRecovery"]["recoverable"]
            .as_bool()
            .unwrap()
    );
    // SAFETY: kill(pid, 0) only checks the fixture child's process existence.
    assert_ne!(
        unsafe { libc::kill(child, 0) },
        0,
        "folder worker descendant survived stop"
    );
    let output = fixture.command(&["logs", &run_id]).output().unwrap();
    success(&output);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-key"));
}

#[test]
fn folder_resume_uses_stored_workspace_from_a_different_launcher() {
    let fixture = WorkspaceFixture::new();
    let original_id = fixture.failed_folder_run();
    let other_launcher = fixture.root.path("another-launcher");
    fs::create_dir(&other_launcher).unwrap();
    let resumed = fixture
        .command(&["resume", &original_id])
        .current_dir(&other_launcher)
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let resumed_id = receipt(&resumed)["runId"].as_str().unwrap().to_owned();
    let guard = StopRun {
        fixture: &fixture,
        run_id: resumed_id.clone(),
        stopped: false,
    };
    assert_ne!(resumed_id, original_id);
    let status = fixture.finished_status(&resumed_id);
    assert_eq!(
        status["status"]["terminalResult"]["reason"],
        "chaos_failure"
    );
    assert!(status["source"].is_null());
    assert_eq!(status["workspaceRecovery"]["resumedFrom"], original_id);
    let starts = fixture.provider_starts();
    let workspace = fs::canonicalize(&fixture.workspace).unwrap();
    assert_eq!(
        starts.lines().count(),
        2,
        "resume did not execute a fresh worker: {starts}"
    );
    assert!(starts.lines().all(|line| Path::new(line) == workspace));
    assert!(!other_launcher.join("mutation.txt").exists());
    assert!(
        !fixture.workspace.join("resumed.txt").exists(),
        "resume imported an old provider session"
    );
    assert!(
        !fixture.json(&["status", &original_id])["workspaceRecovery"]["recoverable"]
            .as_bool()
            .unwrap()
    );
    guard.stop();
}

#[test]
fn nested_conditional_delivery_rejects_unusable_sources_before_all_effects() {
    for mode in [
        "plain",
        "unborn",
        "detached",
        "remoteless",
        "ambiguous",
        "bare",
    ] {
        let fixture = WorkspaceFixture::new();
        fixture.dormant_delivery("push");
        if mode != "plain" {
            fixture.initialize_git(mode != "unborn", mode == "detached");
        }
        if mode == "bare" {
            git(&fixture.workspace, &["config", "core.bare", "true"]);
            git(
                &fixture.workspace,
                &[
                    "remote",
                    "add",
                    "release",
                    "git@forge.invalid:team/bare.git",
                ],
            );
        }
        if mode == "ambiguous" {
            for name in ["first", "second"] {
                git(
                    &fixture.workspace,
                    &[
                        "remote",
                        "add",
                        name,
                        &format!("git@forge.invalid:team/{name}.git"),
                    ],
                );
            }
        }
        let mut submission = fixture.submission(&fixture.workspace, "finish", &["--validate-only"]);
        if mode != "plain" {
            submission.env("PATH", format!("{}:/usr/bin:/bin", fixture.bin.display()));
        }
        let output = submission.output().unwrap();
        assert!(
            !output.status.success(),
            "{mode} conditional delivery admitted"
        );
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("Git"),
            "{mode} failed outside source preflight: {error}"
        );
        fixture.assert_no_execution();
    }
}

#[test]
fn generic_nonorigin_remote_supports_push_and_rejects_forge_modes_before_launch() {
    let fixture = WorkspaceFixture::new();
    fixture.initialize_git(true, false);
    git(
        &fixture.workspace,
        &[
            "remote",
            "add",
            "release",
            "ssh://deployer@forge.invalid:2222/team/nested/project.git",
        ],
    );
    for mode in ["push", "pull_request", "merge"] {
        fixture.dormant_delivery(mode);
        let output = fixture
            .submission(&fixture.workspace, "finish", &["--validate-only"])
            .env("PATH", format!("{}:/usr/bin:/bin", fixture.bin.display()))
            .output()
            .unwrap();
        if mode == "push" {
            success(&output);
        } else {
            assert!(!output.status.success(), "generic {mode} admitted");
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("registered forge identity"),
                "{mode} failed outside forge preflight: {error}"
            );
        }
        fixture.assert_no_execution();
    }
}

#[test]
fn ambient_git_repository_redirects_cannot_move_a_plain_folder_run() {
    let fixture = WorkspaceFixture::new();
    let unrelated = fixture.root.path("unrelated-checkout");
    fs::create_dir(&unrelated).unwrap();
    git(&unrelated, &["init", "-b", "main"]);
    fs::write(unrelated.join("seed"), "must remain unchanged").unwrap();
    git(&unrelated, &["add", "seed"]);
    git(&unrelated, &["commit", "-m", "unrelated"]);
    git(
        &unrelated,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:unrelated/project.git",
        ],
    );
    let output = fixture
        .submission(&fixture.workspace, "finish", &[])
        .env("PATH", format!("{}:/usr/bin:/bin", fixture.bin.display()))
        .env("GIT_DIR", unrelated.join(".git"))
        .env("GIT_COMMON_DIR", unrelated.join(".git"))
        .env("GIT_WORK_TREE", &unrelated)
        .output()
        .unwrap();
    success(&output);
    let run_id = receipt(&output)["runId"].as_str().unwrap().to_owned();
    assert!(fixture.json(&["status", &run_id])["source"].is_null());
    assert!(fixture.workspace.join("mutation.txt").is_file());
    assert!(!unrelated.join("mutation.txt").exists());
    assert!(!fixture.workspace.join(".git").exists());
    let starts = fixture.provider_starts();
    assert_eq!(starts.trim(), fixture.workspace.to_string_lossy());
}

#[test]
fn ancestor_workspace_leases_reject_both_orders_and_allow_sibling_runs() {
    for child_first in [false, true] {
        let fixture = WorkspaceFixture::new();
        let child = fixture.workspace.join("nested-child");
        fs::create_dir(&child).unwrap();
        let (first, rejected) = if child_first {
            (&child, &fixture.workspace)
        } else {
            (&fixture.workspace, &child)
        };
        let active = fixture.start_blocked(first, "active-folder-key");
        fixture.assert_overlap_rejected(rejected);
        assert_eq!(
            fixture.json(&["status", &active.run_id])["status"]["phase"],
            "running"
        );
        active.stop();
    }
    let fixture = WorkspaceFixture::new();
    let first = fixture.workspace.join("sibling-one");
    let second = fixture.workspace.join("sibling-two");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let one = fixture.start_blocked(&first, "first-sibling-key");
    let two = fixture.start_blocked(&second, "second-sibling-key");
    for run in [&one, &two] {
        assert_eq!(
            fixture.json(&["status", &run.run_id])["status"]["phase"],
            "running"
        );
    }
    two.stop();
    one.stop();
}

#[test]
fn selected_plain_folder_checkpoint_restores_bytes_and_a_fresh_worker_session() {
    let fixture = WorkspaceFixture::new();
    let original_id = fixture.failed_folder_run();
    let catalog = fixture.json(&["checkpoints", &original_id]);
    let checkpoint = catalog["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|checkpoint| checkpoint["node"] == "worker")
        .unwrap_or_else(|| panic!("plain folder did not capture a worker boundary: {catalog}"));
    let checkpoint_id = checkpoint["checkpointId"].as_str().unwrap();
    fs::write(
        fixture.workspace.join("later.txt"),
        "must be removed by restore",
    )
    .unwrap();
    fs::write(
        fixture.workspace.join("mutation.txt"),
        "tampered after failure",
    )
    .unwrap();
    let resumed = fixture
        .command(&["resume", &original_id, "--from-checkpoint", checkpoint_id])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    success(&resumed);
    let run_id = receipt(&resumed)["runId"].as_str().unwrap().to_owned();
    let active = StopRun {
        fixture: &fixture,
        run_id: run_id.clone(),
        stopped: false,
    };
    let terminal = fixture.finished_status(&run_id);
    assert!(terminal["source"].is_null());
    assert_eq!(terminal["workspaceRecovery"]["resumedFrom"], original_id);
    assert_eq!(
        terminal["status"]["terminalResult"]["reason"],
        "chaos_failure"
    );
    assert!(!fixture.workspace.join("later.txt").exists());
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("mutation.txt")).unwrap(),
        "native worker ran"
    );
    assert!(!fixture.workspace.join("resumed.txt").exists());
    assert!(!fixture.workspace.join(".git").exists());
    let starts = fixture.provider_starts();
    assert_eq!(
        starts.lines().count(),
        2,
        "checkpoint did not start a fresh worker"
    );
    active.stop();
}
