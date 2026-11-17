use super::*;

fn steer_fixture() -> LocalFixture {
    let fixture = conversations::fixture();
    let helper = fixture.root.path("steer-provider.py");
    let source = openengine_cluster_testkit::fixture::python3_script(
        r#"#!/usr/bin/env python3
import json, os, pathlib, subprocess, sys, time, tomllib
root = pathlib.Path.cwd()
node, iteration, attempt = [os.environ[k] for k in ('ZEROSHOT_NODE_NAME','ZEROSHOT_NODE_ITERATION','ZEROSHOT_NODE_ATTEMPT')]
identity = node+'-'+iteration+'-'+attempt
(root/('steer-prompt-'+identity)).write_bytes(sys.stdin.buffer.read())
(root/('steer-launch-'+identity)).write_text(json.dumps(sys.argv[1:]))
if node == 'worker' and iteration == '1':
    (root/'steer-held').touch()
    while not (root/'steer-release').exists(): time.sleep(.02)
if node == 'check' and iteration == '1' and attempt == '1' and os.environ['FAKE_CODEX_MODE'] == 'steer-retry':
    sys.exit(19)
if node == 'worker' and iteration == '2' and os.environ['FAKE_CODEX_MODE'] == 'steer-old':
    (root/'steer-held-2').touch()
    while not (root/'steer-release-2').exists(): time.sleep(.02)
if node == 'check' and iteration == '2' and os.environ['FAKE_CODEX_MODE'] == 'steer-old':
    sys.exit(19)
settings = next(arg for arg in sys.argv[1:] if arg.startswith('hooks.PreToolUse='))
handler = tomllib.loads(settings)['hooks']['PreToolUse'][0]['hooks'][0]
if node == 'check' and iteration == '1' and os.environ['FAKE_CODEX_MODE'] == 'steer-replace':
    (root/'steer-check-held').touch()
    while not (root/'steer-check-release').exists(): time.sleep(.02)
for call in range(2):
    result = subprocess.run(handler['command'], shell=True, input=json.dumps({'tool_name':'Read','tool_input':{'file_path':'README.md'}}), text=True, capture_output=True, check=True)
    path = root/('steer-tool-'+identity+'-'+str(call))
    # Retain the first callback when the adapter retries a failed native turn.
    if os.environ['FAKE_CODEX_MODE'] != 'steer-old' or not path.exists():
        path.write_text(result.stdout)
if node == 'worker' and iteration == '1' and os.environ['FAKE_CODEX_MODE'] == 'steer-replace':
    while not (root/'steer-worker-finish').exists(): time.sleep(.02)
if node == 'worker' and iteration == '2' and os.environ['FAKE_CODEX_MODE'] == 'steer-old':
    sys.exit(19)
"#,
    );
    openengine_cluster_testkit::fixture::write_executable(&helper, source.as_bytes(), 0o755)
        .assert_value();
    let script = conversations::SCRIPT
        .replace(
            "cat >/dev/null",
            &format!("'{}' \"$@\" || exit $?", helper.display()),
        )
        .replace("TRANSCRIPT_MARKER", "only-in-provider-session-7a8b63");
    openengine_cluster_testkit::fixture::write_executable(
        &fixture.root.path("bin/codex"),
        with_configuration_probe(
            script.as_bytes(),
            "codex",
            &ProbeResponse::Configured(json!({})),
        ),
        0o755,
    )
    .assert_value();
    // A failed verifier attempt must not acknowledge the prompt delivery.
    let mut graph: Value =
        serde_json::from_slice(&fs::read(&fixture.graph).assert_value()).assert_value();
    graph["root"]["children"][0]["body"]["children"][1]["attempts"] = json!(2);
    write_json(&fixture.graph, &graph);
    fixture
}
async fn steer_held(fixture: &LocalFixture) {
    steer_observed(fixture, "steer-held").await;
}
async fn steer_observed(fixture: &LocalFixture, marker: &str) {
    let deadline = Instant::now() + CLI_TIMEOUT;
    while !fixture.repository.join(marker).exists() {
        assert!(
            Instant::now() < deadline,
            "steer provider never wrote {marker}"
        );
        sleep(Duration::from_millis(20)).await;
    }
}
fn steer_read(fixture: &LocalFixture, name: &str) -> String {
    fs::read_to_string(fixture.repository.join(name)).assert_value()
}
fn steer_release(fixture: &LocalFixture) {
    fs::write(fixture.repository.join("steer-release"), "go").assert_value();
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_live_controller_keeps_delivering_after_atomic_executable_replacement() {
    let mut fixture = steer_fixture();
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(&fixture.runtime).assert_value()).assert_value();
    runtime["workspaceCheckpoints"] = json!(false);
    write_json(&fixture.runtime, &runtime);
    // Replace only this fixture's installation, leaving the shared candidate untouched.
    fixture.executable = fixture.root.path("installed-zeroshot");
    fs::copy(env!("CARGO_BIN_EXE_zeroshot"), &fixture.executable).assert_value();
    let run = fixture.submit_detached("steer-replace").await;
    steer_held(&fixture).await;
    let controller = fixture.ready_pid(&run);
    fixture
        .json(&["steer", &run, "before replacement"], "steer-replace")
        .await;
    steer_release(&fixture);
    steer_observed(&fixture, "steer-tool-worker-1-1-1").await;
    assert!(steer_read(&fixture, "steer-tool-worker-1-1-0").contains("before replacement"));
    assert!(!steer_read(&fixture, "steer-tool-worker-1-1-1").contains("<steer>"));

    let staged = fixture.root.path("replacement-zeroshot");
    fs::copy(env!("CARGO_BIN_EXE_zeroshot"), &staged).assert_value();
    fs::rename(staged, &fixture.executable).assert_value();
    let mut deleted = fixture.executable.as_os_str().to_owned();
    deleted.push(" (deleted)");
    assert_eq!(
        fs::read_link(format!("/proc/{controller}/exe")).assert_value(),
        PathBuf::from(deleted)
    );
    fs::write(fixture.repository.join("steer-worker-finish"), "go").assert_value();
    steer_observed(&fixture, "steer-check-held").await;
    assert_eq!(fixture.ready_pid(&run), controller);
    let receipt = fixture
        .json(&["steer", &run, "after replacement"], "steer-replace")
        .await;
    assert_eq!(receipt["steer"]["receivedExecutions"], 0);
    fs::write(fixture.repository.join("steer-check-release"), "go").assert_value();
    fixture.wait_succeeded(&run, "steer-replace").await;
    wait_for_exit(controller).await;
    assert!(steer_read(&fixture, "steer-tool-check-1-1-0").contains("after replacement"));
    assert!(!steer_read(&fixture, "steer-tool-check-1-1-1").contains("<steer>"));
    let status = fixture.json(&["status", &run], "steer-replace").await;
    let steer = status["steers"]
        .as_array()
        .assert_value()
        .iter()
        .find(|steer| steer["id"] == receipt["steer"]["id"])
        .assert_value();
    // The verifier's tool call, the continued worker's prompt and the new verifier's prompt.
    assert_eq!(steer["receivedExecutions"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_running_local_delivers_tools_then_verifier_prompt_and_retries() {
    let fixture = steer_fixture();
    let run = fixture.submit_detached("steer-retry").await;
    steer_held(&fixture).await;
    let text = format!("Direction {}", "é".repeat(700));
    let receipt = fixture.json(&["steer", &run, &text], "steer-retry").await;
    assert_eq!(receipt["steer"]["textBytes"], text.len());
    assert_eq!(
        receipt["steer"]["excerpt"].as_str().assert_value().len(),
        1024
    );
    assert_eq!(receipt["steer"]["receivedExecutions"], 0);
    assert_eq!(
        fixture.json(&["status", &run], "steer-retry").await["steers"][0],
        receipt["steer"]
    );
    steer_release(&fixture);
    fixture.wait_succeeded(&run, "steer-retry").await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    let first = steer_read(&fixture, "steer-tool-worker-1-1-0");
    let context: Value = serde_json::from_str(&first).assert_value();
    assert!(
        context["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .assert_value()
            .contains(&text)
    );
    assert!(!steer_read(&fixture, "steer-tool-worker-1-1-1").contains("<steer>"));
    for attempt in [1, 2] {
        assert_eq!(
            steer_read(&fixture, &format!("steer-prompt-check-1-{attempt}"))
                .matches(&text)
                .count(),
            1,
            "verifier attempt {attempt} lost the steer across a native provider failure"
        );
    }
    assert!(!steer_read(&fixture, "steer-tool-check-1-2-0").contains("<steer>"));
    let status = fixture.json(&["status", &run], "finish").await;
    // Worker conversation, both verifier attempts, and the second round's fresh verifier
    // conversation: a prompt that carried the steer counts when its turn starts.
    assert_eq!(status["steers"][0]["receivedExecutions"], 4);
    assert!(!steer_read(&fixture, "steer-prompt-worker-2-1").contains(&text));
    let vectors = fixture.json(&["vectors", &run], "finish").await;
    assert_eq!(vectors["vectors"][0]["kind"], "steer");
    assert_eq!(vectors["vectors"][1]["kind"], "steer_receipt");
    let full = fixture
        .json(
            &[
                "vectors",
                "show",
                &run,
                receipt["steer"]["id"].as_str().assert_value(),
            ],
            "finish",
        )
        .await;
    assert_eq!(full["text"], text);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_suspended_successor_keeps_receipts_for_continued_conversations() {
    let fixture = steer_fixture();
    let run = fixture.submit_detached("steer").await;
    steer_held(&fixture).await;
    let a = fixture.json(&["steer", &run, "steer-A"], "steer").await;
    fixture.json(&["suspend", &run], "steer").await;
    steer_release(&fixture);
    fixture.wait_suspended(&run).await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    let b = fixture.json(&["steer", &run, "steer-B"], "steer").await;
    assert_eq!(b["steer"]["receivedExecutions"], 0);
    let status = fixture.json(&["status", &run], "steer").await;
    assert_eq!(status["status"]["phase"], "suspended");
    assert!(status["workspaceRecovery"]["successorRunId"].is_null());
    let next = fixture.json(&["resume", &run], "steer").await;
    let next = next["runId"].as_str().assert_value();
    fixture.wait_succeeded(next, "steer").await;
    wait_for_exit(fixture.ready_pid(next)).await;
    let check = steer_read(&fixture, "steer-prompt-check-1-1");
    assert_eq!(check.matches("steer-A").count(), 1);
    assert_eq!(check.matches("steer-B").count(), 1);
    assert!(check.find("steer-A") < check.find("steer-B"));
    let worker = steer_read(&fixture, "steer-prompt-worker-2-1");
    assert!(!worker.contains("steer-A"));
    assert_eq!(worker.matches("steer-B").count(), 1);
    let launch: Vec<String> =
        serde_json::from_str(&steer_read(&fixture, "steer-launch-worker-2-1")).assert_value();
    assert!(launch.iter().any(|arg| arg == "resume"));
    let vectors = fixture.json(&["vectors", next], "steer").await;
    let entries = vectors["vectors"].as_array().assert_value();
    assert!(entries.iter().any(|v| v["kind"] == "steer_receipt"
        && v["runId"] == run
        && v["steerId"] == a["steer"]["id"]));
    assert!(entries.iter().any(|v| v["kind"] == "steer_receipt"
        && v["runId"] == next
        && v["steerId"] == b["steer"]["id"]));
    let refusal = fixture.output(&["steer", &run, "too late"], "steer").await;
    assert_eq!(refusal.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refusal.stderr).contains(next));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_restoration_io_fallback_receives_standing_direction_in_fresh_conversation() {
    let mut fixture = steer_fixture();
    // Repair native storage only after the real restoration path has encountered the blocker.
    let helper = fixture.root.path("steer-provider.py");
    let source = fs::read_to_string(&helper).assert_value().replace(
        "identity = node+'-'+iteration+'-'+attempt",
        "identity = node+'-'+iteration+'-'+attempt\nif node == 'worker' and iteration == '2':\n    p = pathlib.Path(os.environ['CODEX_HOME'])/'sessions'\n    if p.is_file(): p.unlink()",
    );
    openengine_cluster_testkit::fixture::write_executable(&helper, source.as_bytes(), 0o755)
        .assert_value();
    let run = fixture.submit_detached("steer").await;
    steer_held(&fixture).await;
    fixture
        .json(&["steer", &run, "standing-after-restore-failure"], "steer")
        .await;
    fixture.json(&["suspend", &run], "steer").await;
    steer_release(&fixture);
    fixture.wait_suspended(&run).await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    conversations::new_provider_home(&mut fixture);
    let home = fixture.harness_environment.get("CODEX_HOME").assert_value();
    fs::write(Path::new(home).join("sessions"), "restore blocker").assert_value();
    let next = fixture.json(&["resume", &run], "steer").await;
    let next = next["runId"].as_str().assert_value();
    fixture.wait_succeeded(next, "steer").await;
    wait_for_exit(fixture.ready_pid(next)).await;
    let launch: Vec<String> =
        serde_json::from_str(&steer_read(&fixture, "steer-launch-worker-2-1")).assert_value();
    assert!(!launch.iter().any(|arg| arg == "resume"));
    let logs = fixture.output(&["logs", next], "steer").await;
    assert!(
        String::from_utf8_lossy(&logs.stdout)
            .contains("starts a new conversation: provider conversation storage is unavailable")
    );
    assert_eq!(
        steer_read(&fixture, "steer-prompt-worker-2-1")
            .matches("standing-after-restore-failure")
            .count(),
        1
    );
    for call in 0..2 {
        assert!(
            !steer_read(&fixture, &format!("steer-tool-worker-2-1-{call}")).contains("<steer>")
        );
    }
    // The fresh worker has its own receipt in addition to the predecessor worker and verifiers.
    let status = fixture.json(&["status", next], "steer").await;
    assert_eq!(status["steers"][0]["receivedExecutions"], 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_maximum_text_vectors_and_authored_instructions_cross_local_cli_whole() {
    let fixture = steer_fixture();
    let instructions = "i".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES);
    let mut graph: Value =
        serde_json::from_slice(&fs::read(&fixture.graph).assert_value()).assert_value();
    graph["root"]["children"][0]["body"]["children"][0]["instructions"] = json!(instructions);
    write_json(&fixture.graph, &graph);
    let run = fixture.submit_detached("steer").await;
    steer_held(&fixture).await;
    assert!(steer_read(&fixture, "steer-prompt-worker-1-1").contains(&instructions));
    let text = "é".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES / 2);
    let path = fixture.root.path("steer.txt");
    fs::write(&path, &text).assert_value();
    let receipt = fixture
        .json(
            &["steer", &run, "--text-file", path.to_str().assert_value()],
            "steer",
        )
        .await;
    let full = fixture
        .json(
            &[
                "vectors",
                "show",
                &run,
                receipt["steer"]["id"].as_str().assert_value(),
            ],
            "steer",
        )
        .await;
    assert_eq!(full["text"], text);
    for invalid in [
        " ".to_owned(),
        "bad\0text".into(),
        "bad\u{1b}text".into(),
        format!("{text}x"),
    ] {
        fs::write(&path, invalid).assert_value();
        let output = fixture
            .output(
                &["steer", &run, "--text-file", path.to_str().assert_value()],
                "steer",
            )
            .await;
        assert_eq!(output.status.code(), Some(1));
    }
    assert_eq!(
        fixture.json(&["vectors", &run], "steer").await["vectors"]
            .as_array()
            .assert_value()
            .len(),
        1
    );
    steer_release(&fixture);
    fixture.wait_succeeded(&run, "steer").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_request_answer_continues_conversation_with_only_new_direction() {
    let fixture = steer_fixture().with_request(None);
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(&fixture.runtime).assert_value()).assert_value();
    runtime["nodes"]["worker"]["sessionScope"] = json!("node_instance");
    write_json(&fixture.runtime, &runtime);
    let run = fixture.submit_detached("steer").await;
    steer_held(&fixture).await;
    fixture.json(&["steer", &run, "steer-A"], "steer").await;
    steer_release(&fixture);
    let status = fixture.wait_suspended(&run).await;
    assert_eq!(status["status"]["reason"], "request");
    wait_for_exit(fixture.ready_pid(&run)).await;
    fixture.json(&["steer", &run, "steer-B"], "steer").await;
    let path = fixture.root.path("answer-steer.json");
    write_json(
        &path,
        &json!({"signals":{"decision":"rejected"},"output":{"feedback":"Revise"}}),
    );
    let response = fixture
        .json(
            &["answer", &run, "--input", path.to_str().assert_value()],
            "steer",
        )
        .await;
    let next = response["runId"].as_str().assert_value();
    fixture.wait_suspended(next).await;
    wait_for_exit(fixture.ready_pid(next)).await;
    let prompt = steer_read(&fixture, "steer-prompt-worker-2-1");
    assert!(!prompt.contains("steer-A"));
    assert_eq!(prompt.matches("steer-B").count(), 1);
    let args: Vec<String> =
        serde_json::from_str(&steer_read(&fixture, "steer-launch-worker-2-1")).assert_value();
    assert!(args.iter().any(|arg| arg == "resume"));
    fixture.json(&["force-stop", next], "steer").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_selected_older_checkpoint_inherits_only_receipts_in_saved_conversation() {
    let mut fixture = steer_fixture();
    let run = fixture.submit_detached("steer-old").await;
    steer_held(&fixture).await;
    fixture
        .json(&["steer", &run, "direction-before-snapshot"], "steer-old")
        .await;
    steer_release(&fixture);
    let deadline = Instant::now() + CLI_TIMEOUT;
    while !fixture.repository.join("steer-held-2").exists() {
        assert!(Instant::now() < deadline, "second worker never held");
        sleep(Duration::from_millis(20)).await;
    }
    fixture
        .json(&["steer", &run, "direction-after-snapshot"], "steer-old")
        .await;
    fs::write(fixture.repository.join("steer-release-2"), "go").assert_value();
    wait_for_exit(fixture.ready_pid(&run)).await;
    let status = fixture.json(&["status", &run], "steer-old").await;
    assert_eq!(
        status["status"]["terminalResult"]["status"], "failed",
        "{status}"
    );
    assert!(steer_read(&fixture, "steer-tool-worker-2-1-0").contains("direction-after-snapshot"));
    // The worker's tool call, and both attempts of the verifier whose prompt carried it.
    assert_eq!(status["steers"][1]["receivedExecutions"], 3);
    let checkpoints = fixture.json(&["checkpoints", &run], "steer").await;
    let checkpoint = checkpoints["checkpoints"]
        .as_array()
        .assert_value()
        .iter()
        .find(|entry| entry["node"] == "worker" && entry["loopIterations"] == json!([2]))
        .assert_value()["checkpointId"]
        .as_str()
        .assert_value();
    conversations::new_provider_home(&mut fixture);
    let next = fixture
        .json(
            &[
                "resume",
                &run,
                "--from-checkpoint",
                checkpoint,
                "--discard-workspace-changes",
            ],
            "steer",
        )
        .await;
    let next = next["runId"].as_str().assert_value();
    fixture.wait_succeeded(next, "steer").await;
    let prompt = steer_read(&fixture, "steer-prompt-worker-2-1");
    assert!(!prompt.contains("direction-before-snapshot"));
    assert_eq!(prompt.matches("direction-after-snapshot").count(), 1);
    let args: Vec<String> =
        serde_json::from_str(&steer_read(&fixture, "steer-launch-worker-2-1")).assert_value();
    assert!(args.iter().any(|arg| arg == "resume"));
    for call in 0..2 {
        assert!(
            !steer_read(&fixture, &format!("steer-tool-worker-2-1-{call}")).contains("<steer>")
        );
    }
    let vectors = fixture.json(&["vectors", next], "steer").await;
    let after = status["steers"][1]["id"].as_str().assert_value();
    assert_eq!(
        vectors["vectors"]
            .as_array()
            .assert_value()
            .iter()
            .filter(|v| v["kind"] == "steer_receipt" && v["steerId"] == after)
            .count(),
        // The three inherited from the failed run, then the restored worker and its verifier.
        5
    );
}
