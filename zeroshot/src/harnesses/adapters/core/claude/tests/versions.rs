use super::*;
use crate::native_v2_observability::NativeV2Observability;
use crate::native_v2_supervisor::{NativeV2Supervisor, RunEnvironment};
use crate::v2_run_ledger::{CreateRun, RunLedger, sqlite::SqliteRunLedger};
use openengine_cluster_protocol::{RunId, RunLogsParams, Sha256Digest, TerminalResult};
use openengine_cluster_testkit::assertions::AssertError;

#[tokio::test]
async fn first_turn_version_refusal_omits_credentials_crossing_the_output_limit() {
    let secret = "SYNTHETIC-1517-CREDENTIAL-abcdefghijklmno";
    let workspace = TestDirectory::new("claude-version-secret-boundary");
    workspace.write(
        "fake-claude.sh",
        &format!(
            "if [ \"$*\" != --version ]; then : > model-turn; exit 91; fi\n\
             printf '%s' '{}'\nprintf '%s' '{}'\n/bin/sleep 0.05\nprintf '%s\\n' '{}'\n",
            "x".repeat(1000),
            &secret[..12],
            &secret[12..]
        ),
    );
    let (runner, binding) =
        anthropic_runner(&workspace, "claude-test", None, SessionScope::Execution).await;
    let handle = runner
        .start(request(binding, 1, &[(ANTHROPIC_KEY, secret)]))
        .await
        .assert_value();
    let (captured, completion) = failure::complete_with_durable(handle).await;
    let error = completion.assert_error();
    let public = format!("{}\n{error}", captured.logs.join("\n"));
    assert!(public.contains("omitted"), "{public}");
    assert!(!public.contains(&secret[..12]), "{public}");
    assert!(!public.contains(&secret[12..]), "{public}");
    assert!(!workspace.child("model-turn").exists());
}

#[tokio::test]
async fn finished_owner_runs_release_version_checks_and_keep_versions_once_per_run() {
    let workspace = TestDirectory::new("claude-run-versions");
    workspace.write(
        "fake-claude.sh",
        r#"set -eu
if [ "$*" = --version ]; then
  printf 'checked\n' >> version-checks
  sleep 0.1
  printf '2.1.318 (Claude Code)\n'
  exit 0
fi
cat >/dev/null
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"session-1","structured_output":{"response":"done"}}'
"#,
    );
    let binding = agent_binding("claude-test", None, SessionScope::NodeInstance, &[]);
    let mut step = serde_json::to_value(graph(false)).assert_value()["root"]["children"][0].clone();
    let first = step.clone();
    step["name"] = json!("peer");
    let parallel = json!({
        "kind":"par", "name":"parallel", "state":{"kind":"null"},
        "branches":[first, step], "join":{"kind":"all"}, "promotedStatePaths":[]
    });
    step = serde_json::to_value(graph(false)).assert_value()["root"]["children"][0].clone();
    step["name"] = json!("later");
    let runtime = RuntimePlan::new(
        openengine_cluster_protocol::RuntimeLane::new("claude", "anthropic").assert_value(),
        run_size("host"),
        ["agent", "peer", "later"]
            .into_iter()
            .map(|name| (NodeName::new(name).assert_value(), binding.clone()))
            .collect(),
    );
    let admitted = admit(RunSubmission {
        environment: None,
        title: RunTitle::new("Run version logs").assert_value(),
        graph: full_graph(vec![parallel, step, success_node()]),
        initial_input: Value::Null,
        runtime,
        source: None,
        submission_key: IdempotencyKey::new("run-versions").assert_value(),
    })
    .await;
    let mut configuration =
        adapter_configuration(ClaudeProvider::Anthropic, "/bin/sh", BTreeMap::new());
    configuration.prefix_arguments = vec![
        workspace
            .child("fake-claude.sh")
            .to_string_lossy()
            .into_owned(),
    ];
    configuration.workspace = workspace.path().to_owned();
    configuration.runtime_home = workspace.path().to_owned();
    let adapter = Arc::new(ClaudeAdapter::new_for_test(configuration).assert_value());
    let runner = Arc::new(
        NativeNodeRunner::new_owner_scoped(&admitted, adapter.clone(), adapter.clone())
            .assert_value(),
    );
    let environment =
        Arc::new(RunEnvironment::exact(&admitted.runtime, None, BTreeMap::new()).assert_value());
    let path = workspace.child("run.sqlite");
    let ledger = Arc::new(SqliteRunLedger::open(&path).assert_value());
    let runs = ["run-versions", "run-successor", "run-third"];
    for run in runs {
        let run_id = RunId::new(run);
        ledger
            .create_or_get(CreateRun {
                run_id: run_id.clone(),
                submission_key: IdempotencyKey::new(run).assert_value(),
                submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
                admitted: admitted.clone(),
            })
            .await
            .assert_value();
        let supervisor =
            NativeV2Supervisor::new(run_id, ledger.clone(), runner.clone(), environment.clone());
        for _ in 0..2 {
            assert!(matches!(
                supervisor.drive().await.assert_value(),
                TerminalResult::Succeeded { .. }
            ));
            assert!(adapter.version_checked_runs.lock().await.is_empty());
        }
    }
    assert_eq!(
        workspace.read("version-checks"),
        "checked\n".repeat(runs.len())
    );
    runner.close_session().await;
    drop(runner);
    drop(adapter);
    drop(ledger);

    // Read the public logs from retained storage after the runtime has gone away.
    let ledger = Arc::new(SqliteRunLedger::open_read_only(path).assert_value());
    for run in runs {
        let (_, mut logs) = NativeV2Observability::new(ledger.clone())
            .logs(RunLogsParams {
                run_id: RunId::new(run),
                from_cursor: None,
                execution: None,
            })
            .await
            .assert_value();
        let first = logs.recv().await.assert_value().assert_value();
        assert!(first.execution.is_none());
        let zeroshot = concat!("Zeroshot version: ", env!("CARGO_PKG_VERSION"));
        assert_eq!(first.record.message.as_str(), zeroshot);
        let mut claude_versions = 0;
        while let Some(log) = logs.recv().await.assert_value() {
            assert_ne!(log.record.message.as_str(), zeroshot);
            if log.record.message.as_str() == "Claude Code version: 2.1.318" {
                assert!(log.execution.is_some());
                claude_versions += 1;
            }
        }
        assert_eq!(claude_versions, 1);
    }
}
