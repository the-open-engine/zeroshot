use super::*;
use super::checkpoint_tests::prerequisite;
use crate::native_v2_contract::TokenUsageDelta;
use crate::v2_run_ledger::{SnapshotAndTail, apply_event};
use openengine_cluster_protocol::TokenCount;

fn prerequisite_graph() -> GraphSpec {
    let state = json!({"kind":"record","fields":{
        "message":{"required":true,"type":{"kind":"string"}}
    }});
    let mut prepare = step("prepare", 10_000);
    prepare["output"] = state.clone();
    prepare["writeBindings"] = json!([{
        "value":{"node":"prepare","channel":"out","path":["message"]}, "target":["message"]
    }]);
    let mut consume = step("consume", 10_000);
    consume["input"] = state.clone();
    consume["inputBindings"] = json!([{
        "target":["message"], "value":{"source":"state","path":["message"]}
    }]);
    let route = json!({
        "kind":"choice", "name":"route", "state":state,
        "branches":[{"when":signal_guard("review", "accepted"), "node":consume}],
        "otherwise":step("rejected", 10_000), "promotedStatePaths":[]
    });
    graph(
        sequence(
            vec![prepare, verifier("review", 10_000), route, succeed("done")],
            state.clone(),
        ),
        state,
    )
}

async fn import_prerequisites(harness: &Harness) -> Vec<DurableExecution> {
    let history = vec![
        prerequisite(
            1,
            "prepare",
            WorkerOutcome::Verified {
                output: json!({"message":"saved prerequisite"}),
                artifacts: Vec::new(),
            },
        ),
        prerequisite(2, "review", verifier_outcome("accepted")),
    ];
    harness
        .ledger
        .append(
            &harness.supervisor.run_id,
            history
                .iter()
                .cloned()
                .map(|execution| RunEvent::PriorExecution { execution })
                .collect(),
        )
        .await
        .assert_value();
    let imported = stored_run(&harness.ledger).await.snapshot;
    assert_eq!(imported.execution_seed, history);
    assert!(imported.executions.is_empty());
    assert!(imported.token_usage.is_none());
    history
}

async fn record_successor_usage(harness: &Harness) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while harness.driver.starts("consume") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value();
    let stored = stored_run(&harness.ledger).await;
    let node = stored.snapshot.active_executions().next().assert_value();
    assert_eq!(node.reference.node.as_str(), "consume");
    assert_eq!(node.reference.execution.get(), 3);
    assert_eq!(node.input, json!({"message":"saved prerequisite"}));
    assert_eq!(
        stored.admitted.initial_input,
        json!({"message":"original input"})
    );
    harness
        .ledger
        .append(
            &harness.supervisor.run_id,
            vec![RunEvent::TokenUsageObserved {
                execution: node.reference.execution,
                usage: Some(TokenUsageDelta {
                    input_tokens: TokenCount::new(7).assert_value(),
                    output_tokens: TokenCount::new(11).assert_value(),
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                }),
            }],
        )
        .await
        .assert_value();
}

fn assert_replay_and_accounting(tail: &SnapshotAndTail) {
    let mut replay = tail.snapshot.replay_seed();
    for event in &tail.events {
        apply_event(
            &mut replay,
            &event.event,
            cursor_sequence(&event.cursor).assert_value(),
        )
        .assert_value();
    }
    assert_eq!(replay, tail.snapshot);
    assert_eq!(replay.executions.len(), 1);
    let usage = replay.token_usage.assert_value();
    assert_eq!(
        (usage.input_tokens.get(), usage.output_tokens.get()),
        (7, 11)
    );
    assert_eq!(
        tail.events
            .iter()
            .filter(|entry| matches!(entry.event, RunEvent::NodeStarted { .. }))
            .count(),
        1
    );
    assert!(
        tail.events
            .iter()
            .filter_map(|entry| match entry.event {
                RunEvent::TokenUsageObserved { execution, .. } => Some(execution),
                _ => None,
            })
            .all(|execution| execution.get() == 3)
    );
}

#[tokio::test]
async fn selected_seed_preserves_outputs_routes_and_replays_without_prior_dispatch_or_usage() {
    let release = Arc::new(tokio::sync::Barrier::new(2));
    let harness = harness(
        prerequisite_graph(),
        json!({"message":"original input"}),
        FakeDriver::scripted([("consume", vec![Behavior::Together(release.clone())])]),
    )
    .await;
    let history = import_prerequisites(&harness).await;
    let supervisor = harness.supervisor.clone();
    let drive = tokio::spawn(async move { supervisor.drive().await });
    record_successor_usage(&harness).await;
    release.wait().await;
    assert!(matches!(
        drive.await.assert_value().assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(harness.driver.starts("prepare"), 0);
    assert_eq!(harness.driver.starts("review"), 0);
    assert_eq!(harness.driver.starts("rejected"), 0);
    assert_eq!(harness.driver.starts("consume"), 1);
    assert_eq!(harness.sessions.opened.load(Ordering::SeqCst), 1);
    let tail = harness
        .ledger
        .snapshot_and_tail(&harness.supervisor.run_id, None)
        .await
        .assert_value();
    assert_eq!(tail.snapshot.execution_seed, history);
    assert_replay_and_accounting(&tail);
    let normalized = durable_history(&tail.snapshot).assert_value();
    assert_eq!(normalized.len(), 3);
    assert!(normalized.last().assert_value().dispatch_position.get() > 201);
}
