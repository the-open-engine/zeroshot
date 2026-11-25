use super::{Fixture, success};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::{Duration, Instant};

fn receipt(output: &Output) -> String {
    success(output);
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["runId"]
        .as_str()
        .unwrap()
        .into()
}

fn settled(fixture: &Fixture, id: &str, phase: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let status = fixture.json(&["status", id]);
        if status["status"]["phase"] == phase {
            return status;
        }
        assert_ne!(
            status["status"]["phase"], "finished",
            "unexpected outcome: {status}"
        );
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn runtime(fixture: &Fixture, checkpoints: bool) {
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime["workspaceCheckpoints"] = checkpoints.into();
    fixture.write("runtime.json", &runtime);
}

fn assert_in_place(fixture: &Fixture, folder: &Path, id: &str) {
    let status = settled(fixture, id, "finished");
    assert_eq!(
        status["status"]["terminalResult"]["status"], "succeeded",
        "{status}"
    );
    assert!(status.get("source").is_none(), "{status}");
    assert_eq!(status["workspace"]["placement"], "in_place");
    assert_eq!(
        Path::new(status["workspace"]["localDirectory"].as_str().unwrap()),
        folder.canonicalize().unwrap()
    );
    assert_eq!(
        fs::read(folder.join("mutation.txt")).unwrap(),
        b"native worker ran"
    );
    assert!(
        fixture.json(&["checkpoints", id])["checkpoints"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!fixture.root.path("state-workspaces").exists());
}

#[test]
fn local_workspace_defaults_to_exact_in_place_folder_without_git_or_snapshots() {
    for git_subfolder in [false, true] {
        let fixture = Fixture::new();
        runtime(&fixture, true);
        let folder = if git_subfolder {
            fixture.workspace.join("selected")
        } else {
            fixture.root.path("plain")
        };
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("user.txt"), b"uncommitted work").unwrap();
        let mut command = fixture.run_command("finish", true);
        command
            .current_dir(&folder)
            .arg("--workspace")
            .env("ZEROSHOT_RESTIC", fixture.root.path("absent-restic"));
        #[cfg(unix)]
        {
            fixture.write("environment.json", &json!({"startup":"pwd > startup-path"}));
            command
                .arg("--environment")
                .arg(fixture.root.path("environment.json"));
        }
        let submitted = command.output().unwrap();
        let id = receipt(&submitted);
        assert_in_place(&fixture, &folder, &id);
        assert_eq!(
            fs::read(folder.join("user.txt")).unwrap(),
            b"uncommitted work"
        );
        assert!(!fixture.workspace.join("mutation.txt").exists());
        #[cfg(unix)]
        assert_eq!(
            Path::new(
                fs::read_to_string(folder.join("startup-path"))
                    .unwrap()
                    .trim()
            ),
            folder.canonicalize().unwrap()
        );
    }
}

#[test]
fn managed_local_restart_reuses_pinned_root_and_only_disposes_the_copy() {
    let fixture = Fixture::new().outside_workspace();
    runtime(&fixture, false);
    let original_index = fs::read(fixture.workspace.join(".git/index")).unwrap();
    fs::create_dir(fixture.workspace.join("empty")).unwrap();
    let id = receipt(
        &fixture
            .run_command("fail", true)
            .args(["--workspace", "--copy-workspace"])
            .output()
            .unwrap(),
    );
    let status = settled(&fixture, &id, "finished");
    assert_eq!(status["status"]["terminalResult"]["status"], "failed");
    let copy = std::path::PathBuf::from(status["workspace"]["localDirectory"].as_str().unwrap());
    assert_ne!(copy, fixture.workspace);
    assert!(copy.join("empty").is_dir());
    assert_eq!(fs::read(copy.join("seed")).unwrap(), b"seed");
    fs::write(copy.join("seed"), b"worker edit").unwrap();
    success(
        &std::process::Command::new("git")
            .arg("-C")
            .arg(&copy)
            .args(["add", "seed"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read(fixture.workspace.join(".git/index")).unwrap(),
        original_index
    );
    fs::write(copy.join("extra"), b"worker extra").unwrap();
    fs::write(fixture.workspace.join("seed"), b"new upstream bytes").unwrap();
    let successor = receipt(
        &fixture
            .command(&["resume", &id])
            .env("FIXTURE_MODE", "fail")
            .output()
            .unwrap(),
    );
    let status = settled(&fixture, &successor, "finished");
    assert_eq!(
        status["workspace"]["localDirectory"],
        copy.to_str().unwrap()
    );
    assert_eq!(fs::read(copy.join("seed")).unwrap(), b"seed");
    assert!(!copy.join("extra").exists());
    assert_eq!(
        fs::read(fixture.workspace.join("seed")).unwrap(),
        b"new upstream bytes"
    );
    let successor = receipt(
        &fixture
            .command(&["resume", &successor])
            .env("FIXTURE_MODE", "finish")
            .output()
            .unwrap(),
    );
    let status = settled(&fixture, &successor, "finished");
    assert_eq!(
        status["status"]["terminalResult"]["status"], "succeeded",
        "{status}"
    );
    assert!(!copy.exists());
    assert!(fixture.workspace.is_dir());
    assert!(!fixture.workspace.join("mutation.txt").exists());
}

#[test]
fn in_place_request_continuation_preserves_external_edits_and_refuses_restore() {
    let fixture = Fixture::new().outside_workspace();
    runtime(&fixture, true);
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    let worker = graph["root"]["children"][0].clone();
    let state =
        json!({"kind":"record","fields":{"text":{"type":{"kind":"string"},"required":true}}});
    graph["initialInput"] = state.clone();
    graph["root"]["state"] = state.clone();
    graph["root"]["children"] = json!([
        {"kind":"verifier","name":"approval","worker":"builtin.request.ask@1", "input":state,
         "output":{"kind":"record","fields":{}},"signals":{},
         "diagnostic":{"kind":"record","fields":{"message":{"type":{"kind":"string"},"required":true}}},
         "attempts":1,"inputBindings":[{"target":["text"],"value":{"source":"state","path":["text"]}}],"writeBindings":[]},
        worker, {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]);
    fixture.write("graph.json", &graph);
    fixture.write("input.json", &json!({"text":"Continue?"}));
    let mut plan: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    plan["nodes"]["approval"] = json!({"kind":"request"});
    fixture.write("runtime.json", &plan);
    let id = receipt(
        &fixture
            .run_command("finish", true)
            .arg("--workspace")
            .env("ZEROSHOT_RESTIC", fixture.root.path("absent-restic"))
            .output()
            .unwrap(),
    );
    settled(&fixture, &id, "suspended");
    fs::write(fixture.workspace.join("seed"), b"external edit survives").unwrap();
    let refused = fixture
        .command(&[
            "resume",
            &id,
            "--from-checkpoint",
            "not-a-checkpoint",
            "--discard-workspace-changes",
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(
        fs::read(fixture.workspace.join("seed")).unwrap(),
        b"external edit survives"
    );
    fixture.write("answer.json", &json!({"signals":{},"output":{}}));
    let successor = receipt(
        &fixture
            .command(&["answer", &id, "--input"])
            .arg(fixture.root.path("answer.json"))
            .env("FIXTURE_MODE", "finish")
            .env("ZEROSHOT_RESTIC", fixture.root.path("absent-restic"))
            .output()
            .unwrap(),
    );
    assert_in_place(&fixture, &fixture.workspace, &successor);
    assert_eq!(
        fs::read(fixture.workspace.join("seed")).unwrap(),
        b"external edit survives"
    );
}

#[test]
fn workspace_git_flags_and_in_place_seeds_are_refused_before_startup() {
    let fixture = Fixture::new();
    for arguments in [
        vec!["--branch", "main"],
        vec!["--repository", "acme/repo"],
        vec![
            "--workspace-placement",
            "in-place",
            "--workspace-seed",
            "user:one.txt",
        ],
    ] {
        let output = fixture
            .run_command("finish", true)
            .arg("--workspace")
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
    assert!(!fixture.root.path("state/runs").exists());
    assert!(!fixture.workspace.join("mutation.txt").exists());
}

#[test]
fn managed_local_file_selections_and_submission_retry_keep_original_bytes() {
    let fixture = Fixture::new().outside_workspace();
    runtime(&fixture, false);
    let source = fixture.root.path("library-source");
    fs::create_dir_all(source.join("nested/empty")).unwrap();
    fs::write(source.join("one.txt"), b"first").unwrap();
    fs::write(source.join("two.txt"), b"second").unwrap();
    fs::write(source.join("nested/data"), b"child").unwrap();
    success(
        &fixture
            .command(&["file", "upload"])
            .arg(&source)
            .arg("library")
            .output()
            .unwrap(),
    );
    let submit = || {
        fixture
            .run_command("fail", true)
            .args([
                "--workspace",
                "--submission-key",
                "pinned-inputs",
                "--workspace-seed",
                "user:library#one.txt",
                "--workspace-seed",
                "user:library#two.txt",
                "--workspace-seed",
                "user:library#nested",
            ])
            .output()
            .unwrap()
    };
    let original = receipt(&submit());
    let status = settled(&fixture, &original, "finished");
    let workspace = Path::new(status["workspace"]["localDirectory"].as_str().unwrap());
    fs::remove_dir_all(source).unwrap();
    fixture.json(&["file", "remove", "library"]);
    assert_eq!(receipt(&submit()), original);
    assert_eq!(fs::read(workspace.join("one.txt")).unwrap(), b"first");
    assert_eq!(fs::read(workspace.join("two.txt")).unwrap(), b"second");
    assert_eq!(fs::read(workspace.join("nested/data")).unwrap(), b"child");
    assert!(workspace.join("nested/empty").is_dir());
    let successor = receipt(
        &fixture
            .command(&["resume", &original])
            .env("FIXTURE_MODE", "finish")
            .output()
            .unwrap(),
    );
    assert_eq!(
        settled(&fixture, &successor, "finished")["status"]["terminalResult"]["status"],
        "succeeded"
    );
}

#[test]
fn missing_in_place_folder_and_snapshot_restore_have_actionable_refusals() {
    let fixture = Fixture::new().outside_workspace();
    runtime(&fixture, true);
    let id = receipt(
        &fixture
            .run_command("fail", true)
            .arg("--workspace")
            .output()
            .unwrap(),
    );
    settled(&fixture, &id, "finished");
    let refused = fixture
        .command(&[
            "resume",
            &id,
            "--from-checkpoint",
            "any-checkpoint",
            "--discard-workspace-changes",
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("in-place workspaces cannot restore"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let moved = fixture.root.path("temporarily-unavailable");
    fs::rename(&fixture.workspace, &moved).unwrap();
    let refused = fixture
        .command(&["resume", &id])
        .env("FIXTURE_MODE", "finish")
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("make the original folder available"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(fs::read(moved.join("seed")).unwrap(), b"seed");
    assert!(!fixture.workspace.exists());
    fs::rename(moved, &fixture.workspace).unwrap();
    let successor = receipt(
        &fixture
            .command(&["resume", &id])
            .env("FIXTURE_MODE", "finish")
            .output()
            .unwrap(),
    );
    assert_in_place(&fixture, &fixture.workspace, &successor);
}

#[test]
fn managed_copy_replay_survives_missing_source_and_retains_conflict_checks() {
    let fixture = Fixture::new().outside_workspace();
    runtime(&fixture, false);
    let submit = || {
        fixture
            .run_command("fail", true)
            .args([
                "--workspace",
                "--copy-workspace",
                "--submission-key",
                "lost-copy-ack",
            ])
            .output()
            .unwrap()
    };
    let id = receipt(&submit());
    let status = settled(&fixture, &id, "finished");
    let copy = Path::new(status["workspace"]["localDirectory"].as_str().unwrap());
    fs::rename(&fixture.workspace, fixture.root.path("moved-source")).unwrap();
    assert_eq!(receipt(&submit()), id);
    assert_eq!(fs::read(copy.join("seed")).unwrap(), b"seed");
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    graph["root"]["name"] = "different-root".into();
    fixture.write("graph.json", &graph);
    let conflict = submit();
    assert!(!conflict.status.success());
    assert!(
        String::from_utf8_lossy(&conflict.stderr).contains("submission"),
        "{}",
        String::from_utf8_lossy(&conflict.stderr)
    );
    fixture.write("input.json", &Value::Null);
    let successor = receipt(
        &fixture
            .command(&["resume", &id])
            .env("FIXTURE_MODE", "finish")
            .output()
            .unwrap(),
    );
    assert_eq!(
        settled(&fixture, &successor, "finished")["status"]["terminalResult"]["status"],
        "succeeded"
    );
    assert!(!copy.exists());
}

#[test]
fn managed_linked_worktree_copy_is_refused_before_acceptance_without_source_mutation() {
    for checkpoints in [false, true] {
        let mut fixture = Fixture::new().outside_workspace();
        runtime(&fixture, checkpoints);
        let original = fixture.workspace.clone();
        let linked = fixture.root.path("linked");
        success(
            &std::process::Command::new("git")
                .arg("-C")
                .arg(&original)
                .args(["worktree", "add", "-b", "linked-copy"])
                .arg(&linked)
                .output()
                .unwrap(),
        );
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&linked)
                .args(args)
                .output()
                .unwrap();
            success(&output);
            output.stdout
        };
        let index = std::path::PathBuf::from(
            String::from_utf8(git(&["rev-parse", "--git-path", "index"]))
                .unwrap()
                .trim(),
        );
        let before = fs::read(&index).unwrap();
        let refs = git(&["show-ref"]);
        fs::write(linked.join("seed"), b"dirty original").unwrap();
        fixture.workspace = linked.clone();
        let output = fixture
            .run_command("finish", true)
            .args(["--workspace", "--copy-workspace"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("external Git administration"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(index).unwrap(), before);
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&linked)
            .arg("show-ref")
            .output()
            .unwrap();
        success(&output);
        assert_eq!(output.stdout, refs);
        assert_eq!(fs::read(linked.join("seed")).unwrap(), b"dirty original");
        assert!(!linked.join("mutation.txt").exists());
        assert!(!fixture.root.path("state/runs").exists());
    }
}

#[test]
fn managed_inputs_refuse_worktree_config_while_ordinary_git_restore_is_isolated() {
    for checkpoints in [false, true] {
        for seeded in [false, true] {
            for redirect in [false, true] {
                let fixture = Fixture::new().outside_workspace();
                runtime(&fixture, checkpoints);
                let git = |root: &Path, args: &[&str]| {
                    let output = std::process::Command::new("git")
                        .arg("-C")
                        .arg(root)
                        .args(args)
                        .output()
                        .unwrap();
                    success(&output);
                };
                if redirect {
                    git(
                        &fixture.workspace,
                        &[
                            "config",
                            "core.worktree",
                            fixture.workspace.to_str().unwrap(),
                        ],
                    );
                }
                fs::write(fixture.workspace.join("seed"), b"dirty user edit").unwrap();
                let config = fs::read(fixture.workspace.join(".git/config")).unwrap();
                let index = fs::read(fixture.workspace.join(".git/index")).unwrap();
                let mut command = fixture.run_command("fail", true);
                command.arg("--workspace");
                if seeded {
                    success(
                        &fixture
                            .command(&["file", "upload"])
                            .arg(&fixture.workspace)
                            .arg("repository")
                            .output()
                            .unwrap(),
                    );
                    command.args(["--workspace-seed", "user:repository"]);
                } else {
                    command.arg("--copy-workspace");
                }
                let submitted = command.output().unwrap();
                // Drain an unexpectedly accepted run before asserting a refusal.
                let status = submitted.status.success().then(|| {
                    let id = receipt(&submitted);
                    settled(&fixture, &id, "finished")
                });
                if redirect {
                    assert!(
                        !submitted.status.success(),
                        "unsafe config was admitted: {status:?}"
                    );
                    assert!(
                        String::from_utf8_lossy(&submitted.stderr)
                            .contains("external Git administration")
                    );
                    assert!(!fixture.root.path("state/runs").exists());
                } else {
                    let status = status.unwrap();
                    let mut copy = std::path::PathBuf::from(
                        status["workspace"]["localDirectory"].as_str().unwrap(),
                    );
                    if seeded {
                        copy.push("repository");
                    }
                    assert_eq!(fs::read(copy.join("seed")).unwrap(), b"dirty user edit");
                    git(&copy, &["restore", "--", "seed"]);
                    assert_eq!(fs::read(copy.join("seed")).unwrap(), b"seed");
                }
                assert_eq!(
                    fs::read(fixture.workspace.join("seed")).unwrap(),
                    b"dirty user edit"
                );
                assert_eq!(
                    fs::read(fixture.workspace.join(".git/config")).unwrap(),
                    config
                );
                assert_eq!(
                    fs::read(fixture.workspace.join(".git/index")).unwrap(),
                    index
                );
            }
        }
    }
}

#[test]
fn managed_library_seeds_refuse_linked_git_metadata_before_acceptance() {
    for checkpoints in [false, true] {
        let fixture = Fixture::new().outside_workspace();
        runtime(&fixture, checkpoints);
        let linked = fixture.root.path("linked-seed");
        success(
            &std::process::Command::new("git")
                .arg("-C")
                .arg(&fixture.workspace)
                .args(["worktree", "add", "-b", "linked-seed"])
                .arg(&linked)
                .output()
                .unwrap(),
        );
        fs::write(linked.join("seed"), b"dirty linked user edit").unwrap();
        let admin = fixture.workspace.join(".git/worktrees/linked-seed");
        let index = fs::read(admin.join("index")).unwrap();
        let pointer = fs::read(linked.join(".git")).unwrap();
        success(
            &fixture
                .command(&["file", "upload"])
                .arg(&linked)
                .arg("library")
                .output()
                .unwrap(),
        );
        for selection in ["user:library", "user:library#.git"] {
            let submitted = fixture
                .run_command("fail", true)
                .args(["--workspace", "--workspace-seed", selection])
                .output()
                .unwrap();
            if submitted.status.success() {
                settled(&fixture, &receipt(&submitted), "finished");
            }
            assert!(
                !submitted.status.success(),
                "unsafe seed was admitted: {selection}"
            );
            assert!(
                String::from_utf8_lossy(&submitted.stderr).contains("external Git administration"),
                "{}",
                String::from_utf8_lossy(&submitted.stderr)
            );
            assert!(!fixture.root.path("state/runs").exists());
        }
        assert_eq!(fs::read(admin.join("index")).unwrap(), index);
        assert_eq!(fs::read(linked.join(".git")).unwrap(), pointer);
        assert_eq!(
            fs::read(linked.join("seed")).unwrap(),
            b"dirty linked user edit"
        );
        assert_eq!(fs::read(fixture.workspace.join("seed")).unwrap(), b"seed");
    }
}

#[test]
fn managed_copy_recognizes_git_administration_without_a_git_name() {
    assert_administrative_layout_isolation(None);
}

#[test]
fn managed_seed_recognizes_git_administration_without_a_git_name() {
    assert_administrative_layout_isolation(Some(&[]));
}

#[test]
fn managed_seeds_recognize_an_assembled_git_administrative_root() {
    assert_administrative_layout_isolation(Some(&[
        "HEAD",
        "config",
        "objects",
        "refs",
        "index",
        "packed-refs",
    ]));
}

fn assert_administrative_layout_isolation(seed_members: Option<&[&str]>) {
    for checkpoints in [false, true] {
        for redirect in [false, true] {
            let mut fixture = Fixture::new().outside_workspace();
            runtime(&fixture, checkpoints);
            let original = fixture.workspace.clone();
            let admin = fixture.root.path("administration");
            success(
                &std::process::Command::new("git")
                    .args(["clone", "--bare", "--no-hardlinks"])
                    .arg(&original)
                    .arg(&admin)
                    .output()
                    .unwrap(),
            );
            let configure = |args: &[&str]| {
                success(
                    &std::process::Command::new("git")
                        .arg("--git-dir")
                        .arg(&admin)
                        .args(args)
                        .output()
                        .unwrap(),
                );
            };
            configure(&["config", "core.bare", "false"]);
            if redirect {
                configure(&["config", "core.worktree", original.to_str().unwrap()]);
            }
            configure(&["read-tree", "HEAD"]);
            configure(&["pack-refs", "--all"]);
            fs::write(original.join("seed"), b"dirty user edit").unwrap();
            let source_index = fs::read(original.join(".git/index")).unwrap();
            let source_config = fs::read(original.join(".git/config")).unwrap();
            let admin_index = fs::read(admin.join("index")).unwrap();
            let admin_config = fs::read(admin.join("config")).unwrap();
            if seed_members.is_none() {
                fixture.workspace = admin.clone();
            }
            let mut command = fixture.run_command("fail", true);
            command.arg("--workspace");
            if let Some(members) = seed_members {
                success(
                    &fixture
                        .command(&["file", "upload"])
                        .arg(&admin)
                        .arg("library")
                        .output()
                        .unwrap(),
                );
                if members.is_empty() {
                    command.args(["--workspace-seed", "user:library"]);
                } else {
                    for member in members {
                        command
                            .arg("--workspace-seed")
                            .arg(format!("user:library#{member}"));
                    }
                }
            } else {
                command.arg("--copy-workspace");
            }
            #[cfg(unix)]
            {
                let operation = if redirect {
                    "git restore -- seed"
                } else {
                    "git read-tree --empty"
                };
                let startup = if seed_members.is_some_and(|members| members.is_empty()) {
                    format!("cd library && {operation}")
                } else {
                    operation.to_owned()
                };
                fixture.write("environment.json", &json!({"startup":startup}));
                command
                    .arg("--environment")
                    .arg(fixture.root.path("environment.json"));
            }
            let submitted = command.output().unwrap();
            // Drain a wrongly admitted run, then exercise the same relative operation on Windows.
            if submitted.status.success() {
                let status = settled(&fixture, &receipt(&submitted), "finished");
                let mut copy = std::path::PathBuf::from(
                    status["workspace"]["localDirectory"].as_str().unwrap(),
                );
                if seed_members.is_some_and(|members| members.is_empty()) {
                    copy.push("library");
                }
                success(
                    &std::process::Command::new("git")
                        .arg("-C")
                        .arg(&copy)
                        .args(if redirect {
                            &["restore", "--", "seed"][..]
                        } else {
                            &["read-tree", "--empty"][..]
                        })
                        .output()
                        .unwrap(),
                );
                if !redirect {
                    assert_ne!(fs::read(copy.join("index")).unwrap(), admin_index);
                    assert!(!copy.join("seed").exists());
                }
            }
            assert_eq!(
                fs::read(original.join("seed")).unwrap(),
                b"dirty user edit",
                "managed Git escaped to the original folder"
            );
            assert_eq!(fs::read(original.join(".git/index")).unwrap(), source_index);
            assert_eq!(
                fs::read(original.join(".git/config")).unwrap(),
                source_config
            );
            assert_eq!(fs::read(admin.join("index")).unwrap(), admin_index);
            assert_eq!(fs::read(admin.join("config")).unwrap(), admin_config);
            assert!(!admin.join("seed").exists());
            if redirect {
                assert!(
                    !submitted.status.success(),
                    "redirected administration was admitted"
                );
                assert!(
                    String::from_utf8_lossy(&submitted.stderr)
                        .contains("external Git administration")
                );
                assert!(!fixture.root.path("state/runs").exists());
            } else {
                success(&submitted);
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn managed_shutdown_preserves_live_edits_for_plain_resume() {
    let fixture = Fixture::new().outside_workspace();
    runtime(&fixture, true);
    let id = receipt(
        &fixture
            .run_command("block", true)
            .args(["--workspace", "--copy-workspace"])
            .output()
            .unwrap(),
    );
    let status = settled(&fixture, &id, "running");
    let copy = Path::new(status["workspace"]["localDirectory"].as_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    while !copy.join("mutation.txt").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    fs::write(copy.join("seed"), b"keep shutdown edit").unwrap();
    let ready: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .path(&format!("state/runs/{id}/controller.ready.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let pid = i32::try_from(ready["pid"].as_u64().unwrap()).unwrap();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    settled(&fixture, &id, "finished");
    assert_eq!(fs::read(copy.join("seed")).unwrap(), b"keep shutdown edit");
    let successor = receipt(
        &fixture
            .command(&["resume", &id])
            .env("FIXTURE_MODE", "fail")
            .output()
            .unwrap(),
    );
    settled(&fixture, &successor, "finished");
    assert_eq!(fs::read(copy.join("seed")).unwrap(), b"keep shutdown edit");
    assert_eq!(fs::read(fixture.workspace.join("seed")).unwrap(), b"seed");
}

#[test]
fn explicit_stop_of_reconstructed_suspended_run_disposes_only_managed_folder() {
    for managed in [false, true] {
        let fixture = Fixture::new().outside_workspace();
        runtime(&fixture, true);
        let state =
            json!({"kind":"record","fields":{"text":{"type":{"kind":"string"},"required":true}}});
        let mut graph: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
        graph["initialInput"] = state.clone();
        graph["root"]["state"] = state.clone();
        graph["root"]["children"] = json!([
            {"kind":"verifier","name":"approval","worker":"builtin.request.ask@1","input":state,
             "output":{"kind":"record","fields":{}},"signals":{},"diagnostic":{"kind":"record","fields":{"message":{"type":{"kind":"string"},"required":true}}},
             "attempts":1,"inputBindings":[{"target":["text"],"value":{"source":"state","path":["text"]}}],"writeBindings":[]},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]);
        fixture.write("graph.json", &graph);
        fixture.write("input.json", &json!({"text":"Continue?"}));
        let mut plan: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
        plan["nodes"] = json!({"approval":{"kind":"request"}});
        fixture.write("runtime.json", &plan);
        let mut command = fixture.run_command("finish", true);
        command.arg("--workspace");
        if managed {
            command.arg("--copy-workspace");
        }
        let id = receipt(&command.output().unwrap());
        let status = settled(&fixture, &id, "suspended");
        let root = Path::new(status["workspace"]["localDirectory"].as_str().unwrap());
        // Wait for the native process to exit, forcing reconstructed cleanup on every OS.
        let ready: Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .path(&format!("state/runs/{id}/controller.ready.json")),
            )
            .unwrap(),
        )
        .unwrap();
        let pid = u32::try_from(ready["pid"].as_u64().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while super::super::process_exists(pid) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        success(&fixture.command(&["force-stop", &id]).output().unwrap());
        assert_eq!(root.exists(), !managed);
        assert_eq!(fs::read(fixture.workspace.join("seed")).unwrap(), b"seed");
    }
}
