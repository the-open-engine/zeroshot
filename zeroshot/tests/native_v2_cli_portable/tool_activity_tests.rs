use super::fixture::{Fixture, success};
use serde_json::Value;
use std::sync::Arc;
use zeroshot_engine::native_v2_observability::NativeV2Observability;
use zeroshot_engine::v2_run_ledger::sqlite::SqliteRunLedger;

#[test]
fn core_tool_activity_reaches_durable_cli_logs_and_node_transcript_data() {
    const NUMERIC_CREDENTIAL: &str = "8675309123";
    for harness in ["codex", "claude", "copilot", "pi"] {
        let fixture = Fixture::new();
        fixture.tool_harness(harness);
        let result = fixture
            .run_command("finish", false)
            .env(
                "OPENAI_API_KEY",
                if harness == "pi" {
                    NUMERIC_CREDENTIAL
                } else {
                    "fixture-key"
                },
            )
            .env("ANTHROPIC_API_KEY", "fixture-key")
            .output()
            .unwrap();
        success(&result);
        let receipt: Value =
            serde_json::from_slice(result.stdout.split(|byte| *byte == b'\n').next().unwrap())
                .unwrap();
        let id = receipt["runId"].as_str().unwrap();
        assert_eq!(
            fixture.json(&["status", id])["status"]["terminalResult"]["status"],
            "succeeded",
            "{harness}"
        );
        let logs = fixture.command(&["logs", id]).output().unwrap();
        success(&logs);
        let logs = String::from_utf8(logs.stdout).unwrap();
        let records = logs
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let activity = records
            .iter()
            .filter(|record| record.to_string().contains(" tool "))
            .collect::<Vec<_>>();
        let expected_entries = if harness == "codex" { 10 } else { 4 };
        assert_eq!(activity.len(), expected_entries, "{harness}: {logs}");
        for record in &activity {
            let message = record["record"]["message"].as_str().unwrap();
            assert!(message.len() <= 8192, "{harness}: {} bytes", message.len());
            assert!(
                !message.contains(NUMERIC_CREDENTIAL),
                "{harness}: numeric credential exposed"
            );
            assert!(record["execution"].as_str().is_some());
        }
        for status in if harness == "codex" {
            ["status=completed exit=0", "status=failed exit=1"]
        } else {
            ["status=success", "status=error"]
        } {
            assert!(logs.contains(status), "{harness}: missing {status}");
        }
        for expected in [
            "native-ok",
            "native-fail",
            "command-native-ok",
            "command-native-fail",
            "result-native-ok",
            "result-native-fail",
            "input-tail",
            "result-tail",
            "[truncated]",
            "[REDACTED]",
        ] {
            assert!(logs.contains(expected), "{harness}: missing {expected}");
        }
        assert!(!logs.contains("fixture-key"));
        assert!(!logs.contains("ignored-cumulative"));
        // The UI consumes these native history pages to build each node's transcript.
        let ledger = SqliteRunLedger::open_read_only(
            fixture.root.path(&format!("state/runs/{id}/runs.sqlite3")),
        )
        .unwrap();
        let history = NativeV2Observability::new(Arc::new(ledger)).history();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let history_logs = runtime.block_on(async {
            let mut after = None;
            let mut logs = Vec::new();
            loop {
                let page = history
                    .page(&openengine_cluster_protocol::RunId::new(id), after)
                    .await
                    .unwrap();
                logs.extend(
                    page.events
                        .into_iter()
                        .filter(|event| event["event"]["kind"] == "safe_log"),
                );
                if page.complete {
                    break;
                }
                after = Some(page.next_cursor);
            }
            logs
        });
        let tool_logs = history_logs
            .iter()
            .filter(|event| {
                event["event"]["line"]
                    .as_str()
                    .is_some_and(|line| line.contains(" tool "))
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_logs.len(), expected_entries, "{harness}");
        assert!(
            tool_logs
                .iter()
                .all(|event| event["event"]["execution"].as_str().is_some())
        );
        let transcript = serde_json::to_string(&history_logs).unwrap();
        for expected in [
            "command-native-ok",
            "command-native-fail",
            "result-native-ok",
            "result-native-fail",
        ] {
            assert!(
                transcript.contains(expected),
                "{harness}: transcript missing {expected}"
            );
        }
        assert!(!transcript.contains("fixture-key"));
        assert!(!transcript.contains(NUMERIC_CREDENTIAL));
        if harness == "codex" {
            for expected in [
                "spawn_agent id=collab-native-ok status=completed",
                "wait id=collab-native-fail status=failed",
                "fixture-parent",
                "fixture-child",
                "inspect-critical.rs",
                "collab-result-native-ok",
                "collab-result-native-fail",
                "second-query",
            ] {
                assert!(logs.contains(expected), "logs missing {expected}");
                assert!(
                    transcript.contains(expected),
                    "transcript missing {expected}"
                );
            }
            let web = activity
                .iter()
                .map(|record| record["record"]["message"].as_str().unwrap())
                .filter(|message| message.contains("id=native-web "))
                .collect::<Vec<_>>();
            assert_eq!(web.len(), 2);
            assert!(web.iter().all(|message| message.contains("second-query")));
        }
    }
}
