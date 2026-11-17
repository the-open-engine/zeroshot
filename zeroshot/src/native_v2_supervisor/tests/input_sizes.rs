use super::*;
use crate::v2_run_ledger::MAX_EVENT_BYTES;

fn task_type() -> Value {
    json!({"kind":"record","fields":{"task":{"type":{"kind":"string"},"required":true}}})
}

fn task_worker(source: &str) -> Value {
    let mut worker = step("worker", 1_000);
    worker["input"] = task_type();
    worker["inputBindings"] =
        json!([{"target":["task"],"value":{"source":source,"path":["task"]}}]);
    worker
}

#[tokio::test]
async fn oversized_node_input_is_a_descriptive_runtime_failure() {
    let input = json!({"task":"x".repeat(1_400_000)});
    let input_bytes = serde_json::to_vec(&input).assert_value().len();
    let harness = harness(
        graph(
            sequence(vec![task_worker("state"), succeed("done")], task_type()),
            task_type(),
        ),
        input,
        FakeDriver::default(),
    )
    .await;
    let terminal = harness.supervisor.drive().await.assert_value();
    let TerminalResult::Failed { reason, context } = terminal else {
        panic!("oversized input succeeded")
    };
    assert_eq!(reason.as_str(), "runtime_failed");
    let evidence = serde_json::to_string(&context).assert_value();
    for detail in ["worker", &input_bytes.to_string(), "1048576", "1 MiB"] {
        assert!(evidence.contains(detail), "missing {detail}: {evidence}");
    }
    assert_eq!(harness.driver.starts("worker"), 0);
    let mut logs = failure_logs::public_subscription(&harness).await;
    let mut text = String::new();
    while let Some(log) = logs.recv().await.assert_value() {
        text.push_str(log.record.message.as_str());
    }
    assert!(
        text.contains("worker")
            && text.contains(&input_bytes.to_string())
            && text.contains("1048576"),
        "{text}"
    );
}

#[tokio::test]
async fn node_input_just_under_start_record_limit_still_runs() {
    let harness = harness(
        graph(
            sequence(vec![task_worker("state"), succeed("done")], task_type()),
            task_type(),
        ),
        json!({"task":"x".repeat(MAX_EVENT_BYTES - 1024)}),
        FakeDriver::default(),
    )
    .await;
    assert!(matches!(
        harness.supervisor.drive().await.assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(harness.driver.starts("worker"), 1);
}

#[tokio::test]
async fn map_splits_initial_input_above_one_mib_into_small_node_inputs() {
    let state = json!({"kind":"record","fields":{"items":{"required":true,"type":{"kind":"array","items":task_type()}}}});
    let body = json!({"kind":"map","name":"each","state":state,"body":task_worker("item"),
        "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
    let input = json!({"items":vec![json!({"task":"x".repeat(350_000)});4]});
    let bytes = serde_json::to_vec(&input).assert_value().len();
    assert!(bytes > MAX_EVENT_BYTES && bytes < 2 * MAX_EVENT_BYTES);
    let harness = harness(
        graph(sequence(vec![body, succeed("done")], state.clone()), state),
        input,
        FakeDriver::default(),
    )
    .await;
    assert!(matches!(
        harness.supervisor.drive().await.assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(harness.driver.starts("worker"), 4);
}

#[tokio::test]
async fn oversized_node_input_drains_an_active_sibling_before_terminalizing() {
    let mut branch = sequence(
        vec![step("small", 1_000), task_worker("state")],
        task_type(),
    );
    branch["name"] = json!("large_branch");
    let parallel = json!({"kind":"par","name":"parallel","state":task_type(),
        "branches":[step("slow",10_000),branch],"join":{"kind":"all"},"promotedStatePaths":[]});
    let harness = harness(
        graph(
            sequence(vec![parallel, succeed("done")], task_type()),
            task_type(),
        ),
        json!({"task":"x".repeat(1_400_000)}),
        FakeDriver::scripted([
            ("slow", vec![Behavior::Hang]),
            (
                "small",
                vec![Behavior::Complete {
                    delay: Duration::from_millis(20),
                    outcome: WorkerOutcome::Verified {
                        output: Value::Null,
                        artifacts: Vec::new(),
                    },
                }],
            ),
        ]),
    )
    .await;
    let terminal = tokio::time::timeout(Duration::from_secs(5), harness.supervisor.drive())
        .await
        .expect("sibling cleanup must finish")
        .expect("durable input failure");
    assert!(
        matches!(terminal,TerminalResult::Failed {reason,..} if reason.as_str() == "runtime_failed")
    );
    assert_eq!(harness.driver.starts("worker"), 0);
    assert_eq!(harness.driver.cancellations("slow"), 1);
    assert_eq!(harness.driver.state().active, 0);
    assert!(
        stored_run(&harness.ledger)
            .await
            .snapshot
            .active_executions()
            .next()
            .is_none()
    );
}
