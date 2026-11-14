use super::*;

fn example_document(name: &str) -> Value {
    let guide = include_str!("../../../docs/guides/ask-a-person.md");
    let marker = format!("```json title=\"{name}\"\n");
    let text = guide
        .split_once(&marker)
        .assert_value()
        .1
        .split_once("```")
        .assert_value()
        .0;
    serde_json::from_str(text).assert_value()
}
impl LocalFixture {
    fn with_request(self, timeout: Option<u64>) -> Self {
        let mut graph = example_document("graph.json");
        if let Some(timeout) = timeout {
            graph["root"]["children"][0]["body"]["children"][1]["timeoutMs"] = json!(timeout);
            graph["root"]["children"][1]["branches"][0]["node"] =
                json!({"kind":"succeed","name":"expired","output":{"kind":"null"},"bindings":[]});
        }
        let mut runtime = local_runtime();
        runtime["nodes"]["approval"] = json!({"kind":"request"});
        write_json(&self.graph, &graph);
        write_json(&self.runtime, &runtime);
        write_json(&self.input, &example_document("input.json"));
        self
    }
    fn answer_file(&self, value: &Value) -> PathBuf {
        let path = self.root.path("answer.json");
        write_json(&path, value);
        path
    }
    fn invocation_count(&self) -> usize {
        fs::read_to_string(self.repository.join("local-session.args"))
            .assert_value()
            .lines()
            .filter(|line| line.starts_with("mode="))
            .count()
    }
    async fn answer(&self, run: &str, value: &Value) -> Output {
        let path = self.answer_file(value);
        self.output(
            &["answer", run, "--input", path.to_str().assert_value()],
            "finish",
        )
        .await
    }
    async fn own_inbox(&self, run: &str) -> Value {
        self.json(&["inbox", "show", run], "finish").await
    }
}
fn approve() -> Value {
    json!({"signals":{"decision":"approved"},"output":{"feedback":"Looks good."}})
}
fn successor(output: &Output) -> String {
    assert_success(output, "answer accepted");
    let value: Value = serde_json::from_slice(&output.stdout).assert_value();
    value["runId"].as_str().assert_value().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn request_answer_preserves_live_workspace_and_does_not_replay_worker() {
    let fixture = LocalFixture::new().with_request(None);
    let output = fixture.run("Request approval", "finish", false).await;
    assert_success(&output, "foreground request suspension");
    let lines = json_lines(&output.stdout);
    let run = lines[0]["runId"].as_str().assert_value();
    wait_for_exit(fixture.ready_pid(run)).await;
    let status = fixture.wait_suspended(run).await;
    assert_eq!(status["status"]["reason"], "request");
    assert_eq!(status["status"]["node"], "approval");
    assert_eq!(
        status["status"]["request"]["expiresAt"]
            .as_u64()
            .assert_value()
            - status["status"]["createdAt"].as_u64().assert_value(),
        604800000
    );
    assert_eq!(status["workspaceRecovery"]["recoverable"], false);
    let request_id = status["status"]["request"]["id"].as_str().assert_value();
    let vectors = fixture.json(&["vectors", run], "finish").await;
    assert_eq!(vectors["vectors"][0]["kind"], "request");
    assert!(vectors["vectors"][0].get("text").is_none());
    let full = fixture
        .json(&["vectors", "show", run, request_id], "finish")
        .await;
    assert!(full.to_string().contains("Approve"), "{full}");
    let checkpoints = fixture.json(&["checkpoints", run], "finish").await;
    assert!(
        checkpoints.to_string().contains("approval"),
        "{checkpoints}"
    );
    for args in [
        vec!["resume", run],
        vec!["resume", run, "--from-checkpoint", "approval"],
    ] {
        let refused = fixture.output(&args, "finish").await;
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains("zeroshot answer"));
    }
    assert_eq!(fixture.invocation_count(), 1);
    assert_eq!(fixture.own_inbox(run).await["state"], "pending");
    fs::write(
        fixture.repository.join("seed.txt"),
        "edited after checkpoint",
    )
    .assert_value();
    fs::write(fixture.repository.join("new-untracked.txt"), "kept").assert_value();
    let next = successor(&fixture.answer(run, &approve()).await);
    fixture.wait_succeeded(&next, "finish").await;
    assert_eq!(
        fs::read_to_string(fixture.repository.join("seed.txt")).assert_value(),
        "edited after checkpoint"
    );
    assert_eq!(
        fs::read_to_string(fixture.repository.join("new-untracked.txt")).assert_value(),
        "kept"
    );
    assert_eq!(fixture.invocation_count(), 1);
    let old = fixture.own_inbox(run).await;
    assert_eq!(old["state"], "answered");
    assert_eq!(old["successorRunId"], next);
    let next_status = fixture.json(&["status", &next], "finish").await;
    assert_eq!(next_status["workspaceRecovery"]["resumedFrom"], run);
    let vectors = fixture.json(&["vectors", &next], "finish").await;
    assert_eq!(vectors["vectors"][0]["kind"], "request");
    assert!(
        vectors["vectors"]
            .as_array()
            .assert_value()
            .iter()
            .any(|entry| entry["kind"] == "answer" && entry["requestId"] == request_id)
    );
    assert!(
        !fixture
            .output(&["inbox", "show", &next], "finish")
            .await
            .status
            .success()
    );
    let refused = fixture.answer(run, &approve()).await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains(&next));
}
#[tokio::test(flavor = "multi_thread")]
async fn interrupted_answer_launch_reconciles_before_accepting_a_retry() {
    for created_storage in [false, true] {
        let fixture = LocalFixture::new().with_request(None);
        let run = fixture.submit_detached("finish").await;
        fixture.wait_suspended(&run).await;
        wait_for_exit(fixture.ready_pid(&run)).await;
        let interrupted = "0199f33f-3b44-7d21-9000-000000000099";
        let recovery_path = fixture.run_storage(&run).join("workspace-recovery.json");
        let mut recovery: Value =
            serde_json::from_slice(&fs::read(&recovery_path).assert_value()).assert_value();
        recovery["successorRunId"] = json!(interrupted);
        write_json(&recovery_path, &recovery);
        if created_storage {
            fs::create_dir_all(fixture.run_storage(interrupted)).assert_value();
        }
        fs::write(fixture.repository.join("seed.txt"), "later work").assert_value();

        let status = fixture.json(&["status", &run], "finish").await;
        assert_eq!(status["workspaceRecovery"]["recoverable"], false);
        assert!(status["workspaceRecovery"]["successorRunId"].is_null());
        assert_eq!(fixture.own_inbox(&run).await["state"], "pending");
        let next = successor(&fixture.answer(&run, &approve()).await);
        assert_ne!(next, interrupted);
        fixture.wait_succeeded(&next, "finish").await;
        assert_eq!(fixture.invocation_count(), 1);
        assert_eq!(
            fs::read_to_string(fixture.repository.join("seed.txt")).assert_value(),
            "later work"
        );
        assert!(!fixture.run_storage(interrupted).exists());
        assert_eq!(fixture.own_inbox(&run).await["successorRunId"], next);
        let refused = fixture.answer(&run, &approve()).await;
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains(&next));
        assert_eq!(
            fixture.json(&["list"], "finish").await["runs"]
                .as_array()
                .assert_value()
                .len(),
            2
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_answers_do_not_consume_and_rejection_creates_a_fresh_request() {
    let fixture = LocalFixture::new().with_request(None);
    let run = fixture.submit_detached("finish").await;
    let status = fixture.wait_suspended(&run).await;
    let path = fixture.root.path("invalid.json");
    fs::write(&path, "{").assert_value();
    assert!(
        !fixture
            .output(
                &["answer", &run, "--input", path.to_str().assert_value()],
                "finish"
            )
            .await
            .status
            .success()
    );
    for value in [
        json!({"signals":{},"output":{"feedback":"ok"}}),
        json!({"signals":{"decision":"other"},"output":{"feedback":"ok"}}),
        json!({"signals":{"decision":"approved","extra":"x"},"output":{"feedback":"ok"}}),
        json!({"signals":{"decision":"approved"},"output":{}}),
        json!({"signals":{"decision":"approved"},"output":{"feedback":7}}),
        json!({"signals":{"decision":"approved"},"output":{"feedback":"ok","extra":true}}),
        json!({"signals":{"decision":"approved"},"output":{"feedback":"ok"},"extra":true}),
    ] {
        let out = fixture.answer(&run, &value).await;
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("invalid_request"));
        assert_eq!(fixture.json(&["status", &run], "finish").await, status);
        assert_eq!(
            fixture.json(&["list"], "finish").await["runs"]
                .as_array()
                .assert_value()
                .len(),
            1
        );
    }
    let next=successor(&fixture.answer(&run,&json!({"signals":{"decision":"rejected"},"output":{"feedback":"Handle the empty case."}})).await);
    let new = fixture.wait_suspended(&next).await;
    assert_ne!(
        new["status"]["request"]["id"],
        status["status"]["request"]["id"]
    );
    assert_eq!(fixture.invocation_count(), 2);
    let page = fixture.json(&["inbox", "list"], "finish").await;
    assert_eq!(page["requests"].as_array().assert_value().len(), 1);
    assert_eq!(page["requests"][0]["runId"], next);
    fixture.json(&["force-stop", &next], "finish").await;
    assert_eq!(fixture.own_inbox(&next).await["state"], "stopped");
}
#[tokio::test(flavor = "multi_thread")]
async fn expiry_is_lazy_and_answers_create_exactly_one_timeout_successor() {
    let fixture = LocalFixture::new().with_request(Some(1));
    let run = fixture.submit_detached("finish").await;
    fixture.wait_suspended(&run).await;
    assert_eq!(fixture.own_inbox(&run).await["state"], "expired");
    assert_eq!(
        fixture.json(&["inbox", "list"], "finish").await["requests"][0]["state"],
        "expired"
    );
    assert!(
        fixture.json(&["status", &run], "finish").await["workspaceRecovery"]["successorRunId"]
            .is_null()
    );
    let out = fixture.answer(&run, &approve()).await;
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("expired"));
    let old = fixture.own_inbox(&run).await;
    let next = old["successorRunId"].as_str().assert_value();
    fixture.wait_succeeded(next, "finish").await;
    assert_eq!(fixture.invocation_count(), 1);
    let vectors = fixture.json(&["vectors", next], "finish").await;
    assert_eq!(vectors["vectors"][0]["kind"], "request");
    assert!(
        vectors["vectors"]
            .as_array()
            .assert_value()
            .iter()
            .any(|entry| entry["kind"] == "expiry")
    );
    let out = fixture.answer(&run, &approve()).await;
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains(next));
    assert_eq!(
        fixture.json(&["list"], "finish").await["runs"]
            .as_array()
            .assert_value()
            .len(),
        2
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn inbox_paginates_only_requests_that_still_hold_their_run() {
    let fixture = LocalFixture::new();
    let finished = fixture.submit_detached("finish").await;
    fixture.wait_succeeded(&finished, "finish").await;
    let fixture = fixture.with_second_worker();
    let held = fixture.submit_detached("hold").await;
    fixture.wait_running(&held).await;
    fixture.json(&["suspend", &held], "hold").await;
    fs::write(fixture.repository.join("release-turn"), "go").assert_value();
    fixture.wait_suspended(&held).await;
    let fixture = fixture.with_request(None);
    let mut runs = Vec::new();
    for _ in 0..3 {
        let run = fixture.submit_detached("finish").await;
        fixture.wait_suspended(&run).await;
        runs.push(run);
    }
    runs.sort();
    let page = fixture
        .json(&["inbox", "list", "--limit", "2"], "finish")
        .await;
    assert_eq!(page["requests"].as_array().assert_value().len(), 2);
    assert_eq!(page["requests"][0]["runId"], runs[0]);
    assert_eq!(page["nextAfter"], runs[1]);
    let page = fixture
        .json(
            &["inbox", "list", "--after", &runs[1], "--limit", "2"],
            "finish",
        )
        .await;
    assert_eq!(page["requests"][0]["runId"], runs[2]);
    assert!(page["nextAfter"].is_null());
    for limit in ["0", "101"] {
        let out = fixture
            .output(&["inbox", "list", "--limit", limit], "finish")
            .await;
        assert!(!out.status.success());
    }
    let next = successor(&fixture.answer(&runs[0], &approve()).await);
    fixture.wait_succeeded(&next, "finish").await;
    for run in &runs[1..] {
        fixture.json(&["force-stop", run], "finish").await;
    }
    assert_eq!(
        fixture.json(&["inbox", "list"], "finish").await["requests"],
        json!([])
    );
    fixture.json(&["force-stop", &held], "finish").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn answer_refuses_finished_stopped_and_command_suspended_runs_without_starting_anything() {
    for mode in ["finish", "fail", "block", "hold"] {
        let fixture = if mode == "hold" {
            LocalFixture::new().with_second_worker()
        } else {
            LocalFixture::new()
        };
        let run = fixture.submit_detached(mode).await;
        match mode {
            "finish" => {
                fixture.wait_succeeded(&run, mode).await;
            }
            "fail" => {
                fixture.wait_terminal(&run, mode, "worker_failed").await;
            }
            "block" => {
                fixture.wait_running(&run).await;
                fixture.json(&["force-stop", &run], mode).await;
                fixture.wait_terminal(&run, mode, "force_stopped").await;
            }
            _ => {
                fixture.wait_running(&run).await;
                fixture.json(&["suspend", &run], mode).await;
                fs::write(fixture.repository.join("release-turn"), "go").assert_value();
                fixture.wait_suspended(&run).await;
            }
        }
        let before = fixture.json(&["status", &run], mode).await;
        let out = fixture.answer(&run, &approve()).await;
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(if mode == "hold" {
                "command"
            } else {
                "suspended"
            })
        );
        assert_eq!(fixture.json(&["status", &run], mode).await, before);
        assert_eq!(
            fixture.json(&["list"], mode).await["runs"]
                .as_array()
                .assert_value()
                .len(),
            1
        );
        assert!(
            !fixture
                .output(&["inbox", "show", &run], mode)
                .await
                .status
                .success()
        );
        if mode == "hold" {
            fixture.json(&["force-stop", &run], mode).await;
        }
    }
}
