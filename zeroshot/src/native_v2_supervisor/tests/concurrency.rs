use super::*;

fn map(body: Value, limit: u64) -> Value {
    json!({"kind":"map","name":"items","state":record_type(),"body":body,
        "over":{"source":"state","path":["items"]},"maxItems":limit,"promotedStatePaths":[]})
}

#[tokio::test]
async fn native_occupancy_obeys_default_64_global_overrides_and_item_slots() {
    for (global, item_limit, expected, count) in [
        (None, 128, 64, 128),
        (Some(1), 128, 1, 33),
        (Some(3), 128, 3, 33),
        (Some(8), 2, 2, 34),
    ] {
        let barrier = Arc::new(tokio::sync::Barrier::new(expected));
        let driver = FakeDriver::scripted([(
            "worker",
            (0..count)
                .map(|_| Behavior::Together(barrier.clone()))
                .collect(),
        )]);
        let mut leaf = step("worker", 10_000);
        leaf["attempts"] = json!(1);
        let graph = graph(
            sequence(vec![map(leaf, item_limit), succeed("done")], record_type()),
            record_type(),
        );
        let harness = harness_with_options(
            graph,
            json!({"items":vec!["item";count]}),
            driver,
            HarnessOptions {
                session_scope: SessionScope::Execution,
                resolver: None,
                max_concurrency: global,
            },
        )
        .await;
        let terminal = tokio::time::timeout(Duration::from_secs(30), harness.supervisor.drive())
            .await
            .assert_value()
            .assert_value();
        assert!(matches!(terminal, TerminalResult::Succeeded { .. }));
        assert_eq!(harness.driver.max_active(), expected);
        assert_eq!(harness.driver.starts("worker"), count);
        assert!(
            stored_run(&harness.ledger)
                .await
                .snapshot
                .executions
                .values()
                .all(|execution| execution.attempt.get() == 1)
        );
    }
}

#[tokio::test]
async fn parents_wait_without_global_slots_in_nested_maps_and_parallel_bodies() {
    let state = record_type();
    let branch = json!({"kind":"par","name":"body_parallel","state":state,"join":{"kind":"all"},
        "branches":[step("left",10_000),step("right",10_000)],"promotedStatePaths":[]});
    let inner = json!({"kind":"map","name":"inner","state":state,"body":branch,
        "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
    let graph = graph(
        sequence(vec![map(inner, 2), succeed("done")], state.clone()),
        state,
    );
    let harness = harness_with_options(
        graph,
        json!({"items":["one","two"]}),
        FakeDriver::default(),
        HarnessOptions {
            session_scope: SessionScope::Execution,
            resolver: None,
            max_concurrency: Some(1),
        },
    )
    .await;
    assert!(matches!(
        harness.supervisor.drive().await.assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(harness.driver.starts("left"), 4);
    assert_eq!(harness.driver.starts("right"), 4);
    assert_eq!(harness.driver.max_active(), 1);
}
