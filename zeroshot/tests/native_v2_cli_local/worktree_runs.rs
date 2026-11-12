use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn publishing_committed_work_without_edits_confirms_delivery_in_both_workspace_modes() {
    for worktree in [false, true] {
        let mut fixture = LocalFixture::new();
        let remote = fixture.root.path("remote.git");
        git(
            &fixture.repository,
            &["clone", "--bare", ".", remote.to_str().unwrap()],
        );
        let url = format!("ssh://git@plain.example{}", remote.display());
        git(&fixture.repository, &["remote", "set-url", "origin", &url]);
        let ssh = fixture.root.path("bin/ssh");
        openengine_cluster_testkit::fixture::write_executable(
            &ssh,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n *git-upload-pack*) exec /usr/bin/git upload-pack '{}' ;;\n *git-receive-pack*) exec /usr/bin/git receive-pack '{}' ;;\n *) exit 1 ;;\nesac\n",
                remote.display(), remote.display(),
            ),
            0o700,
        ).assert_value();
        fixture.harness_environment.insert(
            "GIT_SSH_COMMAND".to_owned(),
            ssh.to_str().unwrap().to_owned(),
        );
        git(
            &fixture.repository,
            &["checkout", "-b", "feature/committed"],
        );
        fs::write(
            fixture.repository.join("committed.txt"),
            "previously unpushed work\n",
        )
        .unwrap();
        git(&fixture.repository, &["add", "committed.txt"]);
        git(
            &fixture.repository,
            &["commit", "-m", "previously committed work"],
        );
        let source = git(&fixture.repository, &["rev-parse", "HEAD"]);

        let mut graph = local_graph::graph();
        let mut fields = serde_json::Map::new();
        for field in ["repository", "targetBranch", "headRevision", "headBranch"] {
            fields.insert(
                field.to_owned(),
                json!({"type":{"kind":"string"},"required":true}),
            );
        }
        for (field, values) in [
            ("version", vec!["v1"]),
            ("mode", vec!["push"]),
            ("outcome", vec!["pushed", "repair_required"]),
        ] {
            fields.insert(
                field.to_owned(),
                json!({"type":{"kind":"enum","values":values},"required":true}),
            );
        }
        graph["root"]["children"][0] = json!({
            "kind":"verifier", "name":"deliver", "worker":"builtin.git-delivery.push@1",
            "input":{"kind":"null"}, "output":{"kind":"record","fields":fields},
            "inputBindings":[], "writeBindings":[], "attempts":1, "timeoutMs":30000,
            "signals":{"delivery":["pushed","repair_required"]},
            "diagnostic":{"kind":"record","fields":{"message":{"type":{"kind":"string"},"required":true}}}
        });
        graph["root"]["children"][1]["branches"][0]["when"] = json!({
            "kind":"in", "value":{"name":"deliver","source":"signal","field":"delivery"},
            "labels":["repair_required"]
        });
        write_json(&fixture.graph, &graph);
        write_json(
            &fixture.runtime,
            &json!({
                "harness":"codex", "provider":"openai", "size":"host",
                "activeMemory":{"repository":false,"graph":false},
                "globalAgents":{"postmortem":{"enabled":false}},
                "nodes":{"deliver":{"kind":"git_delivery","connections":{}}}
            }),
        );
        let mut command = fixture.command(
            &["run", "--title", "Publish committed work", "--detach"],
            "finish",
        );
        for (flag, path) in [
            ("--graph", &fixture.graph),
            ("--input", &fixture.input),
            ("--runtime-config", &fixture.runtime),
        ] {
            command.arg(flag).arg(path);
        }
        if worktree {
            command.arg("--worktree");
        }
        let result = command.output().await.unwrap();
        assert_success(&result, "publish committed work");
        let submitted: Value = serde_json::from_slice(&result.stdout).unwrap();
        let run_id = submitted["runId"].as_str().unwrap();
        fixture.wait_succeeded(run_id, "finish").await;
        wait_for_exit(fixture.ready_pid(run_id)).await;
        let recovery: Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .state
                    .join("runs")
                    .join(run_id)
                    .join("workspace-recovery.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let workspace = Path::new(recovery["workspace"].as_str().unwrap());
        let branch = recovery["localBranch"].as_str().unwrap();
        assert_eq!(git(workspace, &["rev-parse", "HEAD"]), source);
        assert_eq!(git(workspace, &["symbolic-ref", "--short", "HEAD"]), branch);
        assert_eq!(git(&remote, &["rev-parse", branch]), source);
        assert_eq!(
            git(&remote, &["show", &format!("{branch}:committed.txt")]),
            "previously unpushed work"
        );
        assert_eq!(
            git(&fixture.repository, &["symbolic-ref", "--short", "HEAD"]),
            "feature/committed"
        );
        assert_eq!(git(&fixture.repository, &["rev-parse", "HEAD"]), source);
        assert!(git(&fixture.repository, &["status", "--porcelain=v1"]).is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_worktree_run_resumes_in_the_retained_attached_worktree() {
    let fixture = LocalFixture::new();
    fs::write(
        fixture.repository.join(".env"),
        "private untracked resource",
    )
    .unwrap();
    let initial_status = git(&fixture.repository, &["status", "--porcelain=v1"]);
    let branch = git(&fixture.repository, &["symbolic-ref", "--short", "HEAD"]);
    let mut command = fixture.command(
        &[
            "run",
            "--title",
            "Worktree recovery",
            "--worktree",
            "--detach",
            "--submission-key",
            "worktree-replay",
        ],
        "fail",
    );
    for (option, path) in [
        ("--graph", &fixture.graph),
        ("--input", &fixture.input),
        ("--runtime-config", &fixture.runtime),
    ] {
        command.arg(option).arg(path);
    }
    let result = command.output().await.unwrap();
    assert_success(&result, "worktree submission");
    let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
    let run_id = receipt["runId"].as_str().unwrap();
    fixture.wait_terminal(run_id, "fail", "worker_failed").await;
    wait_for_exit(fixture.ready_pid(run_id)).await;
    let replay = command.output().await.unwrap();
    assert_success(&replay, "worktree submission replay");
    assert_eq!(
        serde_json::from_slice::<Value>(&replay.stdout).unwrap(),
        receipt
    );
    let recovery_path = fixture
        .state
        .join("runs")
        .join(run_id)
        .join("workspace-recovery.json");
    let recovery: Value = serde_json::from_slice(&fs::read(recovery_path).unwrap()).unwrap();
    let workspace = PathBuf::from(recovery["workspace"].as_str().unwrap());
    let run_branch = recovery["localBranch"].as_str().unwrap();
    assert!(run_branch.starts_with("zeroshot/v2-"));
    assert_eq!(recovery["workspaceRevision"], fixture.head);
    assert!(workspace.is_dir());
    assert!(!workspace.join(".env").exists());
    // Plain resume keeps work done in the retained worktree after failure, including a newer
    // branch tip and index. It must not reinstall the failed run's latest checkpoint.
    fs::write(workspace.join("fix.txt"), "committed recovery\n").unwrap();
    fs::write(workspace.join(".gitignore"), "build.log\n").unwrap();
    git(&workspace, &["add", "fix.txt", ".gitignore"]);
    git(&workspace, &["commit", "-m", "fix after failure"]);
    let resumed_head = git(&workspace, &["rev-parse", "HEAD"]);
    fs::write(workspace.join("staged.txt"), "staged recovery\n").unwrap();
    git(&workspace, &["add", "staged.txt"]);
    let resumed_index = git(&workspace, &["ls-files", "--stage"]);
    fs::write(workspace.join("seed.txt"), "unstaged recovery\n").unwrap();
    fs::write(workspace.join("untracked.txt"), "untracked recovery\n").unwrap();
    fs::write(workspace.join("build.log"), "ignored recovery\n").unwrap();
    let resumed = fixture.json(&["resume", run_id], "finish").await;
    let successor = resumed["runId"].as_str().unwrap();
    fixture.wait_succeeded(successor, "finish").await;
    wait_for_exit(fixture.ready_pid(successor)).await;
    let recovered: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .state
                .join("runs")
                .join(successor)
                .join("workspace-recovery.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(recovered["workspace"], recovery["workspace"]);
    assert!(workspace.join("local-mutation.txt").exists());
    assert_eq!(
        git(&workspace, &["symbolic-ref", "--short", "HEAD"]),
        run_branch
    );
    assert_eq!(git(&workspace, &["rev-parse", "HEAD"]), resumed_head);
    assert_eq!(git(&workspace, &["rev-parse", run_branch]), resumed_head);
    assert_eq!(git(&workspace, &["ls-files", "--stage"]), resumed_index);
    for (name, contents) in [
        ("fix.txt", "committed recovery\n"),
        ("staged.txt", "staged recovery\n"),
        ("seed.txt", "unstaged recovery\n"),
        ("untracked.txt", "untracked recovery\n"),
        ("build.log", "ignored recovery\n"),
    ] {
        assert_eq!(fs::read_to_string(workspace.join(name)).unwrap(), contents);
    }
    assert_eq!(
        git(&fixture.repository, &["symbolic-ref", "--short", "HEAD"]),
        branch
    );
    assert_eq!(
        git(&fixture.repository, &["rev-parse", "HEAD"]),
        fixture.head
    );
    assert_eq!(
        git(&fixture.repository, &["status", "--porcelain=v1"]),
        initial_status
    );
    assert!(workspace.is_dir());
}
