use super::*;
use openengine_cluster_protocol::RunLogsParams;
use zeroshot_engine::native_v2_observability::NativeV2Observability;

#[tokio::test(flavor = "current_thread")]
async fn consecutive_claude_acp_runs_retain_their_own_versions() {
    let fixture = AcpFixture::new();
    fixture.install_profile_with_worker_lane(Some(json!({
        "harness":"claude", "provider":"anthropic"
    })));
    let checks = fixture._root.path().join("claude-version-checks");
    let state = fixture._root.path().join("claude-state");
    let script = format!(
        r#"#!/bin/sh
set -eu
if [ "${{1-}}" = --version ]; then
  printf 'checked\n' >> {checks:?}
  printf '2.1.318 (Claude Code)\n'
  exit 0
fi
/usr/bin/cat >/dev/null
if [ -e {state:?} ]; then response=second; else : > {state:?}; response=first; fi
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"acp-claude-thread"}}'
printf '%s%s%s\n' \
  '{{"type":"result","subtype":"success","is_error":false,"session_id":"acp-claude-thread","structured_output":{{"response":{{"response":"' \
  "$response" \
  '"}}}}}}'
"#,
    );
    openengine_cluster_testkit::fixture::write_executable(
        &fixture._root.path().join("bin/claude"),
        script,
        0o755,
    )
    .unwrap();
    let mut command = fixture.server_command(&fixture.workspace, &fixture.path);
    command.env("CLAUDE_CODE_RESTRICTED", "true");
    let (run_ids, messages) = fixture
        .run_session_at(command.spawn().unwrap(), fixture.workspace.clone(), true)
        .await;
    assert_eq!(messages, ["first", "second"]);
    assert_ne!(run_ids[0], run_ids[1]);
    assert_eq!(
        std::fs::read_to_string(checks).unwrap(),
        "checked\nchecked\n"
    );
    fixture.assert_durable_runs(&run_ids).await;

    // The ACP process has exited; both version records must survive in each run's ledger.
    for run_id in run_ids {
        let ledger = Arc::new(
            SqliteRunLedger::open_read_only(
                fixture
                    .state
                    .join("runs")
                    .join(&run_id)
                    .join("runs.sqlite3"),
            )
            .unwrap(),
        );
        let (_, mut logs) = NativeV2Observability::new(ledger)
            .logs(RunLogsParams {
                run_id: RunId::new(run_id),
                from_cursor: None,
                execution: None,
            })
            .await
            .unwrap();
        let first = logs.recv().await.unwrap().unwrap();
        let zeroshot = concat!("Zeroshot version: ", env!("CARGO_PKG_VERSION"));
        assert_eq!(first.record.message.as_str(), zeroshot);
        let mut claude = 0;
        while let Some(log) = logs.recv().await.unwrap() {
            assert_ne!(log.record.message.as_str(), zeroshot);
            if log.record.message.as_str() == "Claude Code version: 2.1.318" {
                claude += 1;
            }
        }
        assert_eq!(claude, 1);
    }
}
