use super::*;

/// Run once per configured real lane. Every run, credential home and repository is disposable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit ZEROSHOT_LIVE_* settings and real provider credentials"]
async fn live_suspended_conversation() {
    let required = |name: &str| std::env::var(name).assert_value_with(name);
    let harness = required("ZEROSHOT_LIVE_HARNESS");
    let provider = required("ZEROSHOT_LIVE_PROVIDER");
    let model = required("ZEROSHOT_LIVE_MODEL");
    assert!(matches!(harness.as_str(), "codex" | "claude"));
    let mut fixture = LocalFixture::new();
    let native = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
    let executable = std::env::split_paths(&std::env::var_os("PATH").assert_value())
        .map(|path| path.join(&harness))
        .find(|path| path.is_file());
    // Use the configured installed CLI, never the fixture's stub.
    let program = executable.assert_value();
    fs::remove_file(fixture.root.path("bin/codex")).assert_value();
    openengine_cluster_testkit::fixture::write_executable(
        &fixture.root.path(&format!("bin/{harness}")),
        format!("#!/bin/sh\nexec {} \"$@\"\n", program.display()),
        0o755,
    )
    .assert_value();
    fixture.harness_environment = native
        .into_iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "PATH"
                    | "OPENAI_API_KEY"
                    | "CODEX_API_KEY"
                    | "ANTHROPIC_API_KEY"
                    | "ANTHROPIC_BASE_URL"
                    | "OPENAI_BASE_URL"
                    | "CODEX_BASE_URL"
                    | "HTTP_PROXY"
                    | "HTTPS_PROXY"
                    | "NO_PROXY"
                    | "http_proxy"
                    | "https_proxy"
                    | "no_proxy"
                    | "NODE_USE_ENV_PROXY"
                    | "LANG"
                    | "LC_ALL"
            )
        })
        .collect();
    let user = fixture.root.path("live-user");
    let storage = fixture.root.path("live-provider");
    fs::create_dir_all(&user).assert_value();
    fs::create_dir_all(&storage).assert_value();
    for (name, path) in [
        ("HOME", &user),
        ("CODEX_HOME", &storage),
        ("CLAUDE_CONFIG_DIR", &storage),
    ] {
        fixture
            .harness_environment
            .insert(name.to_owned(), path.display().to_string());
    }
    fixture.harness_environment.insert(
        "PATH".to_owned(),
        format!(
            "{}:{}",
            fixture.root.path("bin").display(),
            required("PATH")
        ),
    );
    let key = if harness == "codex" {
        "OPENAI_API_KEY"
    } else {
        "ANTHROPIC_API_KEY"
    };
    let mut fields = vec![key.to_owned()];
    if let Ok(extra) = std::env::var("ZEROSHOT_LIVE_CONNECTION_FIELDS") {
        fields.extend(extra.split(',').map(str::to_owned));
    }
    for name in &fields {
        required(name);
    }
    let mut runtime = local_runtime();
    runtime["harness"] = json!(harness);
    runtime["provider"] = json!(provider);
    runtime["nodes"] = json!({"worker":{"kind":"agent","model":model,"effort":"medium","sessionScope":"node_instance","connections":{"live":fields}}});
    runtime["nodes"]["check"] = runtime["nodes"]["worker"].clone();
    runtime["nodes"]["check"]["sessionScope"] = json!("execution");
    let instructions = concat!(
        "On EVERY invocation, including a continued conversation, your first action must be a NEW ",
        "native shell call: `if test -e secret-word; then printf 'NEW_WORD\\n'; ",
        "cat secret-word && rm -- secret-word && test ! -e secret-word; ",
        "else printf 'RECALL_WORD\\n'; fi`. ",
        "Choose the branch ONLY from the marker printed by THIS invocation's shell call. ",
        "Never reuse a previous invocation's file check or branch decision. ",
        "For NEW_WORD, remember the printed word in this conversation and return the literal ",
        "string \"stored\" as output. Do not switch branches after deleting the file. ",
        "For RECALL_WORD, return the exact word learned in the earlier invocation as output. ",
        "The earlier reply \"stored\" is an acknowledgement, not the word to recall. ",
        "Do not save it in any other workspace file. ",
        "Never include the word in diagnostic. Do not use Git or network tools. ",
        "Obey the runtime response contract without encoding another JSON object inside the output string."
    );
    let mut graph = local_graph::graph();
    graph["root"]["children"] = json!([
        {"kind":"loop","name":"twice","state":{"kind":"null"},"maxIterations":2,"promotedStatePaths":[],
         "until":{"kind":"in","value":{"source":"signal","name":"check","field":"done"},"labels":["yes"]},
         "body":{"kind":"seq","name":"round","state":{"kind":"null"},"promotedStatePaths":[],"children":[
             {"kind":"step","name":"worker","worker":"agent.worker@1","instructions":instructions,
              "input":{"kind":"null"},"output":{"kind":"string"},"inputBindings":[],"writeBindings":[],"attempts":1},
             {"kind":"verifier","name":"check","worker":"agent.check@1",
              "instructions":"Read only the ZEROSHOT_NODE_ITERATION environment variable with a native tool. Signal done=no for iteration 1 and done=yes for iteration 2. Return null output and null diagnostic. Do not inspect workspace files or use Git or network tools.",
              "input":{"kind":"null"},"output":{"kind":"null"},"signals":{"done":["yes","no"]},"diagnostic":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1}
         ]}},
        {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
    ]);
    write_json(&fixture.graph, &graph);
    write_json(&fixture.runtime, &runtime);
    let word = format!("fact-{}", uuid::Uuid::now_v7().simple());
    fs::write(fixture.repository.join("secret-word"), &word).assert_value();
    async fn json(fixture: &LocalFixture, args: &[&str]) -> Value {
        let output = timeout(
            Duration::from_secs(300),
            fixture.command_with_inline(args, "live", false).output(),
        )
        .await
        .assert_value()
        .assert_value();
        assert_success(&output, "live local command");
        serde_json::from_slice(&output.stdout).assert_value()
    }
    let run = json(
        &fixture,
        &[
            "run",
            "--title",
            "Live checkpoint conversation",
            "--graph",
            fixture.graph.to_str().assert_value(),
            "--input",
            fixture.input.to_str().assert_value(),
            "--runtime-config",
            fixture.runtime.to_str().assert_value(),
            "-d",
        ],
    )
    .await["runId"]
        .as_str()
        .assert_value()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let status = json(&fixture, &["status", &run]).await;
        if status["status"]["phase"] == "running"
            && status["status"]["activeExecutions"]
                .as_array()
                .is_some_and(|active| active.iter().any(|execution| execution["node"] == "worker"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "worker did not start: {status}");
        sleep(Duration::from_millis(30)).await;
    }
    json(&fixture, &["suspend", &run]).await;
    loop {
        let status = json(&fixture, &["status", &run]).await;
        if status["status"]["phase"] == "suspended" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not suspend after one turn: {status}"
        );
        sleep(Duration::from_millis(100)).await;
    }
    wait_for_exit(fixture.ready_pid(&run)).await;
    if fixture.repository.join("secret-word").exists() {
        let logs = fixture
            .command_with_inline(&["logs", &run], "live", false)
            .output()
            .await
            .assert_value();
        panic!(
            "first live turn did not consume secret-word: {}",
            String::from_utf8_lossy(&logs.stdout)
        );
    }
    use zeroshot_engine::v2_run_ledger::{RunLedger, sqlite::SqliteRunLedger};
    let predecessor =
        SqliteRunLedger::open_read_only(fixture.run_storage(&run).join("runs.sqlite3"))
            .assert_value()
            .get(&openengine_cluster_protocol::RunId::new(&run))
            .await
            .assert_value()
            .assert_value();
    assert_eq!(
        predecessor
            .snapshot
            .executions
            .values()
            .filter(|execution| execution.reference.node.as_str() == "worker")
            .count(),
        1,
        "suspension must hold back the worker's second turn"
    );
    let first_turn = predecessor
        .snapshot
        .executions
        .values()
        .find(|execution| execution.reference.node.as_str() == "worker")
        .assert_value();
    assert!(
        matches!(first_turn.outcome(), Some(openengine_cluster_protocol::WorkerOutcome::Verified { output, .. }) if output == "stored"),
        "first turn must acknowledge storing the fact: {:?}",
        first_turn.outcome()
    );
    // Use empty provider storage; the declared connection supplies authentication again.
    let transcripts = storage.join(if harness == "codex" {
        "sessions"
    } else {
        "projects"
    });
    assert!(transcripts.is_dir());
    fs::remove_dir_all(&storage).assert_value();
    fs::create_dir(&storage).assert_value();
    fn absent(path: &Path, word: &str) {
        for entry in fs::read_dir(path).assert_value() {
            let path = entry.assert_value().path();
            if path.is_dir() {
                absent(&path, word);
            } else {
                assert!(
                    !fs::read(path)
                        .assert_value()
                        .windows(word.len())
                        .any(|bytes| bytes == word.as_bytes())
                );
            }
        }
    }
    absent(&storage, &word);
    absent(&fixture.repository, &word);
    let successor = json(&fixture, &["resume", &run]).await["runId"]
        .as_str()
        .assert_value()
        .to_owned();
    loop {
        let status = json(&fixture, &["status", &successor]).await;
        if status["status"]["phase"] == "finished" {
            assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "successor did not complete: {status}"
        );
        sleep(Duration::from_millis(100)).await;
    }
    wait_for_exit(fixture.ready_pid(&successor)).await;
    let ledger =
        SqliteRunLedger::open_read_only(fixture.run_storage(&successor).join("runs.sqlite3"))
            .assert_value();
    let saved = ledger
        .get(&openengine_cluster_protocol::RunId::new(&successor))
        .await
        .assert_value()
        .assert_value();
    let turns = saved
        .snapshot
        .executions
        .values()
        .filter(|execution| execution.reference.node.as_str() == "worker")
        .collect::<Vec<_>>();
    assert_eq!(
        turns.len(),
        1,
        "exactly one worker turn must happen in each run"
    );
    assert!(!turns[0].input.to_string().contains(&word));
    let recalled = matches!(turns[0].outcome(),Some(openengine_cluster_protocol::WorkerOutcome::Verified {output,..}) if output.as_str().is_some_and(|output| output.contains(&word)));
    let evidence = fixture.root.as_path().with_extension("failed");
    if !recalled {
        // Both controllers have exited. Keep their ledgers and native transcripts for diagnosis.
        fs::rename(fixture.root.as_path(), &evidence).assert_value_with("retain recall evidence");
    }
    assert!(
        recalled,
        "successor did not recall the first-turn-only fact: {:?}; evidence retained at {}",
        turns[0].outcome(),
        evidence.display()
    );
}
