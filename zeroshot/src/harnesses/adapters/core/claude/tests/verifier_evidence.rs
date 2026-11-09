//! Public summaries use the effective credential set before SafeLog persistence.
use super::*;
use crate::native_v2_observability::NativeV2Observability;
use crate::native_v2_supervisor::{NativeV2Supervisor, RunEnvironment};
use crate::v2_run_ledger::{CreateRun, RunLedger, fake::FakeRunLedger};
use openengine_cluster_protocol::{RunId, RunLogsParams, Sha256Digest, TerminalResult};

#[tokio::test]
async fn verifier_evidence_redacts_the_same_declared_and_ambient_auth_marker_before_persistence() {
    const MARKER: &str = r#"synthetic-oauth-"marker"-1402\private"#;
    const NAME: &str = "CLAUDE_CODE_OAUTH_TOKEN";
    let encoded_marker = serde_json::to_string(MARKER).assert_value();
    let escaped_marker = &encoded_marker[1..encoded_marker.len() - 1];
    for declared in [false, true] {
        let workspace = TestDirectory::new("claude-verifier-public-redaction");
        let response = json!({"type":"result","subtype":"success","is_error":false,"session_id":"verifier-session",
            "structured_output":{"response":{"output":null,"signals":{"readiness":"revise"},"diagnostic":format!("missing test; {} token={MARKER}", "é".repeat(openengine_cluster_protocol::MAX_LOG_MESSAGE_BYTES))}}});
        workspace.write("response.json", &format!("{response}\n"));
        write_claude_script(
            &workspace,
            "set -eu\ncat >/dev/null\nexec /bin/cat response.json\n",
        );
        let binding = agent_binding(
            "claude-fable-5",
            None,
            SessionScope::Execution,
            if declared { &[NAME] } else { &[] },
        );
        let runtime = RuntimePlan::new(
            openengine_cluster_protocol::RuntimeLane::new("claude", "anthropic").assert_value(),
            run_size("host"),
            BTreeMap::from([("agent".parse().assert_value(), binding)]),
        );
        let mut source_graph = serde_json::to_value(graph(true)).assert_value();
        source_graph["root"]["children"][0]["signals"] = json!({"readiness":["revise","ready"]});
        source_graph["root"]["children"][0]["diagnostic"] = json!({"kind":"string"});
        source_graph["root"]["children"][1] =
            json!({"kind":"fail","name":"halt","reason":"needs_followup"});
        let admitted = admit(RunSubmission {
            environment: None,
            title: RunTitle::new("Verifier credential regression").assert_value(),
            graph: serde_json::from_value(source_graph).assert_value(),
            initial_input: Value::Null,
            runtime,
            source: None,
            submission_key: IdempotencyKey::new("verifier-redaction").assert_value(),
        })
        .await;
        let mut adapter = ClaudeAdapter::new_local(ClaudeAdapterConfig {
            provider: ClaudeProvider::Anthropic,
            executable: "/bin/sh".to_owned(),
            prefix_arguments: vec![
                workspace
                    .path()
                    .join("fake-claude.sh")
                    .to_string_lossy()
                    .into_owned(),
            ],
            workspace: workspace.path().to_owned(),
            runtime_home: workspace.path().to_owned(),
            local_user_home: Some(workspace.path().to_owned()),
            native_environment: LocalHarnessEnvironment::new(BTreeMap::from([
                ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
                (NAME.to_owned(), MARKER.to_owned()),
            ])),
            base_environment: ClaudeProcessEnvironment::new(BTreeMap::from([(
                "PATH".to_owned(),
                "/usr/bin:/bin".to_owned(),
            )]))
            .assert_value(),
            process_pool: HostedProcessPool::new(10_002, 10_002, 20_000).assert_value(),
        })
        .assert_value();
        adapter.test_permission_policy =
            Some(crate::native_v2_capsule::provider_process::PermissionPolicy::Unset);
        let adapter = Arc::new(adapter);
        let runner =
            Arc::new(NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value());
        let values = if declared {
            BTreeMap::from([(
                openengine_cluster_protocol::ConnectionKey::new("provider").assert_value(),
                openengine_cluster_protocol::StaticConnectionValues::new(BTreeMap::from([(
                    environment_name(NAME),
                    MARKER.to_owned(),
                )]))
                .assert_value(),
            )])
        } else {
            BTreeMap::new()
        };
        let environment =
            Arc::new(RunEnvironment::exact(&admitted.runtime, None, values).assert_value());
        let ledger = Arc::new(FakeRunLedger::new());
        let run_id = RunId::new(format!("verifier-redaction-{declared}"));
        ledger
            .create_or_get(CreateRun {
                run_id: run_id.clone(),
                submission_key: IdempotencyKey::new("verifier-redaction").assert_value(),
                submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
                admitted,
            })
            .await
            .assert_value();
        let supervisor =
            NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner, environment);
        supervisor.drive().await.assert_value();
        let stored = ledger.get(&run_id).await.assert_value().assert_value();
        let TerminalResult::Failed {
            context: Some(context),
            ..
        } = stored.snapshot.terminal.assert_value()
        else {
            panic!("missing failure evidence (declared={declared})")
        };
        assert_eq!(
            context.verifiers.len(),
            1,
            "declared={declared}: {context:?}"
        );
        let message = context.verifiers[0].message.as_str();
        assert!(message.len() <= openengine_cluster_protocol::MAX_LOG_MESSAGE_BYTES);
        assert!(message.contains("[middle truncated]"));
        assert!(!message.contains(escaped_marker));
        assert!(
            message.contains("missing test") && message.contains("[REDACTED]"),
            "{message}"
        );
        assert!(
            !serde_json::to_string(&context)
                .assert_value()
                .contains(MARKER)
        );
        let tail = ledger.snapshot_and_tail(&run_id, None).await.assert_value();
        assert!(
            tail.events
                .iter()
                .filter(|event| matches!(
                    &event.event,
                    crate::v2_run_ledger::RunEvent::SafeLog { .. }
                ))
                .all(|event| !serde_json::to_string(event).assert_value().contains(MARKER))
        );
        let (_, mut logs) = NativeV2Observability::new(ledger)
            .logs(RunLogsParams {
                run_id,
                from_cursor: None,
                execution: None,
            })
            .await
            .assert_value();
        let mut matched = false;
        while let Some(log) = logs.recv().await.assert_value() {
            assert!(!log.record.message.as_str().contains(MARKER));
            assert!(!log.record.message.as_str().contains(escaped_marker));
            if Some(&log.cursor) == context.verifiers[0].cursor.as_ref() {
                matched = true;
                assert!(log.record.message.as_str().contains("[REDACTED]"));
            }
        }
        assert!(matched);
    }
}
