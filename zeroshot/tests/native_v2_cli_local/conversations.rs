use super::*;

const TRANSCRIPT_MARKER: &str = "only-in-provider-session-7a8b63";

fn fixture() -> LocalFixture {
    let mut fixture = LocalFixture::new();
    let mut graph = local_graph::graph();
    let worker = graph["root"]["children"][0].clone();
    graph["root"]["children"] = json!([
        {"kind":"loop","name":"twice","state":{"kind":"null"},"maxIterations":2,"promotedStatePaths":[],
         "until":{"kind":"in","value":{"source":"signal","name":"check","field":"done"},"labels":["yes"]},
         "body":{"kind":"seq","name":"round","state":{"kind":"null"},"promotedStatePaths":[],"children":[worker,
            {"kind":"verifier","name":"check","worker":"agent.check@1","instructions":"Check the round.",
             "input":{"kind":"null"},"output":{"kind":"null"},"signals":{"done":["yes","no"]},
             "diagnostic":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1}
         ]}},
        {"kind":"choice","name":"result","state":{"kind":"null"},"promotedStatePaths":[],
         "branches":[{"when":{"kind":"in","value":{"source":"signal","name":"check","field":"done"},"labels":["yes"]},
                      "node":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}}],
         "otherwise":{"kind":"fail","name":"failed","reason":"check_failed"}}
    ]);
    let mut runtime = local_runtime();
    runtime["nodes"]["worker"]["sessionScope"] = json!("node_instance");
    runtime["nodes"]["check"] = runtime["nodes"]["worker"].clone();
    runtime["nodes"]["check"]["sessionScope"] = json!("execution");
    write_json(&fixture.graph, &graph);
    write_json(&fixture.runtime, &runtime);
    let home = fixture.root.path("user-one");
    let provider = fixture.root.path("codex-one");
    fs::create_dir_all(&home).assert_value();
    fs::create_dir_all(&provider).assert_value();
    fs::write(
        provider.join("auth.json"),
        "configuration-secret-never-captured",
    )
    .assert_value();
    fixture.harness_environment.extend([
        ("HOME".to_owned(), home.to_string_lossy().into_owned()),
        (
            "CODEX_HOME".to_owned(),
            provider.to_string_lossy().into_owned(),
        ),
    ]);
    let script = SCRIPT.replace("TRANSCRIPT_MARKER", TRANSCRIPT_MARKER);
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
    fixture
}

fn new_provider_home(fixture: &mut LocalFixture) {
    fs::remove_dir_all(fixture.harness_environment.get("CODEX_HOME").assert_value()).assert_value();
    let provider = fixture.root.path("codex-two");
    fs::create_dir_all(&provider).assert_value();
    fixture.harness_environment.insert(
        "CODEX_HOME".to_owned(),
        provider.to_string_lossy().into_owned(),
    );
    assert_eq!(fs::read_dir(provider).assert_value().count(), 0);
}

fn worker_calls(fixture: &LocalFixture) -> Vec<String> {
    fs::read_to_string(fixture.repository.join("conversation-calls"))
        .assert_value()
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn suspend_turn(fixture: &LocalFixture, mode: &str, turn: u64) -> String {
    let run = fixture.submit_detached(mode).await;
    hold_and_suspend(fixture, &run, mode, turn).await;
    run
}

async fn hold_and_suspend(fixture: &LocalFixture, run: &str, mode: &str, turn: u64) {
    let deadline = Instant::now() + CLI_TIMEOUT;
    while !fixture.repository.join(format!("held-{turn}")).exists() {
        assert!(
            Instant::now() < deadline,
            "worker never held turn {turn}: {}",
            String::from_utf8_lossy(&fixture.output(&["logs", run], mode).await.stdout)
        );
        sleep(Duration::from_millis(30)).await;
    }
    fixture.json(&["suspend", run], mode).await;
    fs::write(
        fixture
            .repository
            .join(format!("release-conversation-{turn}")),
        "release",
    )
    .assert_value();
    fixture.wait_suspended(run).await;
    wait_for_exit(fixture.ready_pid(run)).await;
}

async fn assert_private_and_discarded(fixture: &LocalFixture, predecessor: &str, successor: &str) {
    for run in [predecessor, successor] {
        for command in ["logs", "status", "checkpoints"] {
            let output = fixture.output(&[command, run], "finish").await;
            assert!(!String::from_utf8_lossy(&output.stdout).contains(TRANSCRIPT_MARKER));
        }
    }
    fn inspect(path: &Path) {
        for entry in fs::read_dir(path).assert_value() {
            let path = entry.assert_value().path();
            if path.is_dir() {
                inspect(&path);
            } else {
                let bytes = fs::read(&path).assert_value();
                assert!(
                    !bytes
                        .windows(TRANSCRIPT_MARKER.len())
                        .any(|value| value == TRANSCRIPT_MARKER.as_bytes()),
                    "transcript leaked to {}",
                    path.display()
                );
                assert!(
                    !bytes
                        .windows(b"configuration-secret-never-captured".len())
                        .any(|value| value == b"configuration-secret-never-captured")
                );
            }
        }
    }
    inspect(&fixture.state);
    inspect(&fixture.repository);
    let repositories = fixture.state.join("checkpoint-repositories");
    if repositories.exists() {
        let remaining = fs::read_dir(repositories)
            .assert_value()
            .map(|entry| {
                let path = entry.assert_value().path();
                let children = fs::read_dir(&path)
                    .assert_value()
                    .map(|entry| entry.assert_value().file_name())
                    .collect::<Vec<_>>();
                (path, children)
            })
            .collect::<Vec<_>>();
        assert!(
            remaining.is_empty(),
            "checkpoint repository remains: {remaining:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspended_local_successor_restores_conversation_from_checkpoint() {
    let mut fixture = fixture();
    let run = suspend_turn(&fixture, "suspend", 1).await;
    assert_eq!(worker_calls(&fixture), ["fresh local-thread"]);
    new_provider_home(&mut fixture);
    let resumed = fixture.json(&["resume", &run], "finish").await;
    let successor = resumed["runId"].as_str().assert_value();
    fixture.wait_succeeded(successor, "finish").await;
    wait_for_exit(fixture.ready_pid(successor)).await;
    assert_eq!(
        worker_calls(&fixture),
        ["fresh local-thread", "resume local-thread"],
        "predecessor logs: {}; successor logs: {}",
        String::from_utf8_lossy(&fixture.output(&["logs", &run], "finish").await.stdout),
        String::from_utf8_lossy(&fixture.output(&["logs", successor], "finish").await.stdout)
    );
    assert_private_and_discarded(&fixture, &run, successor).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_codex_usage_starts_a_new_conversation_and_logs_the_reason() {
    let mut fixture = fixture();
    let run = suspend_turn(&fixture, "missing-usage", 1).await;
    let logs = fixture.output(&["logs", &run], "finish").await;
    let logs = String::from_utf8_lossy(&logs.stdout);
    assert!(
        logs.contains("worker") && logs.contains("Codex cumulative usage baseline is unavailable"),
        "{logs}"
    );
    new_provider_home(&mut fixture);
    let resumed = fixture.json(&["resume", &run], "finish").await;
    let successor = resumed["runId"].as_str().assert_value();
    fixture.wait_succeeded(successor, "finish").await;
    wait_for_exit(fixture.ready_pid(successor)).await;
    let status = fixture.json(&["status", successor], "finish").await;
    assert_eq!(
        status["status"]["metadata"]["tokenUsage"]["inputTokens"],
        100
    );
    assert_eq!(
        worker_calls(&fixture),
        ["fresh local-thread", "fresh local-thread"]
    );
    let logs = fixture.output(&["logs", successor], "finish").await;
    let logs = String::from_utf8_lossy(&logs.stdout);
    assert!(
        logs.contains("node worker starts a new conversation")
            && logs.contains("checkpoint has no conversation"),
        "{logs}"
    );
    assert_private_and_discarded(&fixture, &run, successor).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_local_checkpoint_restores_conversation_without_provider_files() {
    let mut fixture = fixture();
    let run = fixture.submit_detached("checkpoint-fail").await;
    let deadline = Instant::now() + CLI_TIMEOUT;
    while fixture.json(&["status", &run], "checkpoint-fail").await["status"]["phase"] != "finished"
    {
        assert!(Instant::now() < deadline);
        sleep(Duration::from_millis(30)).await;
    }
    wait_for_exit(fixture.ready_pid(&run)).await;
    let checkpoints = fixture.json(&["checkpoints", &run], "finish").await;

    let checkpoint = checkpoints["checkpoints"]
        .as_array()
        .assert_value()
        .iter()
        .find(|entry| entry["node"] == "check" && entry["loopIterations"] == json!([1]))
        .assert_value()["checkpointId"]
        .as_str()
        .assert_value();
    new_provider_home(&mut fixture);
    let resumed = fixture
        .json(
            &[
                "resume",
                &run,
                "--from-checkpoint",
                checkpoint,
                "--discard-workspace-changes",
            ],
            "finish",
        )
        .await;
    let successor = resumed["runId"].as_str().assert_value();
    fixture.wait_succeeded(successor, "finish").await;
    wait_for_exit(fixture.ready_pid(successor)).await;
    assert_eq!(
        worker_calls(&fixture),
        ["fresh local-thread", "resume local-thread"],
        "predecessor logs: {}; successor logs: {}",
        String::from_utf8_lossy(&fixture.output(&["logs", &run], "finish").await.stdout),
        String::from_utf8_lossy(&fixture.output(&["logs", successor], "finish").await.stdout)
    );
    assert_private_and_discarded(&fixture, &run, successor).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_conversation_falls_back_and_logs_the_cap() {
    let mut fixture = fixture();
    let run = suspend_turn(&fixture, "cap", 1).await;
    let logs = fixture.output(&["logs", &run], "finish").await;
    assert_eq!(
        String::from_utf8_lossy(&logs.stdout)
            .matches("1 GiB cap")
            .count(),
        1
    );
    new_provider_home(&mut fixture);
    let resumed = fixture.json(&["resume", &run], "finish").await;
    let successor = resumed["runId"].as_str().assert_value();
    fixture.wait_succeeded(successor, "finish").await;
    assert_eq!(
        worker_calls(&fixture),
        ["fresh local-thread", "fresh local-thread"]
    );
    let logs = fixture.output(&["logs", successor], "finish").await;
    assert!(
        String::from_utf8_lossy(&logs.stdout)
            .contains("node worker starts a new conversation: checkpoint has no conversation")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_continued_turn_retries_with_a_new_conversation() {
    let mut fixture = fixture();
    let mut graph: Value =
        serde_json::from_slice(&fs::read(&fixture.graph).assert_value()).assert_value();
    let worker = &mut graph["root"]["children"][0]["body"]["children"][0];
    worker["kind"] = json!("verifier");
    worker["signals"] = json!({});
    worker["diagnostic"] = json!({"kind":"null"});
    worker["attempts"] = json!(2);
    write_json(&fixture.graph, &graph);
    let run = suspend_turn(&fixture, "suspend", 1).await;
    new_provider_home(&mut fixture);
    let resumed = fixture.json(&["resume", &run], "reject-resume").await;
    let successor = resumed["runId"].as_str().assert_value();
    fixture.wait_succeeded(successor, "reject-resume").await;
    let calls = worker_calls(&fixture);
    assert_eq!(calls.first().assert_value(), "fresh local-thread");
    assert_eq!(calls.last().assert_value(), "fresh local-thread");
    assert!(calls.iter().any(|call| call == "resume local-thread"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conversation_checkpoints_deduplicate_growth_across_suspended_successors() {
    let mut fixture = fixture();
    let run = suspend_turn(&fixture, "dedup", 1).await;
    fn size(path: &Path) -> u64 {
        fs::read_dir(path)
            .assert_value()
            .map(|entry| {
                let path = entry.assert_value().path();
                if path.is_dir() {
                    size(&path)
                } else {
                    fs::metadata(path).assert_value().len()
                }
            })
            .sum()
    }
    let repositories = fixture.state.join("checkpoint-repositories");
    let before = size(&repositories);
    assert!(
        before > 64 * 1024 * 1024,
        "repository only has {before} bytes"
    );
    new_provider_home(&mut fixture);
    let resumed = fixture.json(&["resume", &run], "dedup").await;
    let successor = resumed["runId"].as_str().assert_value();
    hold_and_suspend(&fixture, successor, "dedup", 2).await;
    assert_eq!(
        worker_calls(&fixture),
        ["fresh local-thread", "resume local-thread"],
        "successor logs: {}",
        String::from_utf8_lossy(&fixture.output(&["logs", successor], "finish").await.stdout)
    );
    let growth = size(&repositories).saturating_sub(before);
    assert!(
        growth < 16 * 1024 * 1024,
        "repository grew by {growth} bytes"
    );
    fixture.json(&["force-stop", successor], "finish").await;
}

const SCRIPT: &str = r#"#!/bin/sh
set -eu
cat >/dev/null
if test "$ZEROSHOT_NODE_NAME" = check; then
  if test "$FAKE_CODEX_MODE" = checkpoint-fail; then exit 1; fi
  done=no
  if test "$ZEROSHOT_NODE_ITERATION" = 2; then done=yes; fi
  printf '%s\n' '{"type":"thread.started","thread_id":"check-thread"}'
  printf '{"type":"item.completed","item":{"type":"agent_message","text":"{\\"response\\":{\\"output\\":null,\\"signals\\":{\\"done\\":\\"%s\\"},\\"diagnostic\\":null}}"}}\n' "$done"
  printf '%s\n' '{"type":"turn.completed"}'
  exit 0
fi
resumed=false
for arg in "$@"; do if test "$arg" = resume; then resumed=true; fi; done
session="$CODEX_HOME/sessions/2026/11/14/rollout-local-thread.jsonl"
if test "$resumed" = true; then
  printf '%s\n' 'resume local-thread' >> "$PWD/conversation-calls"
  test "$(head -n 1 "$session")" = TRANSCRIPT_MARKER || exit 61
  if test "$FAKE_CODEX_MODE" = reject-resume; then exit 62; fi
  if test "$FAKE_CODEX_MODE" = dedup; then dd if=/dev/urandom bs=1048576 count=4 >> "$session" 2>/dev/null; fi
else
  printf '%s\n' 'fresh local-thread' >> "$PWD/conversation-calls"
  mkdir -p "$(dirname "$session")"
  printf '%s\n' TRANSCRIPT_MARKER > "$session"
  if test "$FAKE_CODEX_MODE" = cap; then truncate -s 1073741825 "$session"; fi
  if test "$FAKE_CODEX_MODE" = dedup; then dd if=/dev/urandom bs=1048576 count=64 >> "$session" 2>/dev/null; fi
fi
printf '%s\n' '{"type":"thread.started","thread_id":"local-thread"}'
case "$FAKE_CODEX_MODE" in
  suspend|cap|dedup|missing-usage)
    touch "$PWD/held-$ZEROSHOT_NODE_ITERATION"
    while ! test -f "$PWD/release-conversation-$ZEROSHOT_NODE_ITERATION"; do sleep 0.05; done
    ;;
esac
if test "$ZEROSHOT_NODE_KIND" = verifier; then
  printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":{\"output\":null,\"signals\":{},\"diagnostic\":null}}"}}'
else
  printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":null}"}}'
fi
if test "$FAKE_CODEX_MODE" = missing-usage; then
  printf '%s\n' '{"type":"turn.completed"}'
elif test "$resumed" = true; then
  printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":130,"output_tokens":17}}'
else
  printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":10}}'
fi
"#;
