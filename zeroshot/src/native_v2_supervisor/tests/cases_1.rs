use super::*;

#[tokio::test]
async fn drives_every_full_v1_construct_through_the_real_reducer() {
    let harness = harness(
        all_constructs_graph(),
        json!({"items": ["one", "two"]}),
        all_constructs_driver(),
    )
    .await;

    assert_eq!(
        harness
            .supervisor
            .drive()
            .await
            .assert_value_with("terminal"),
        TerminalResult::Succeeded {
            output: Value::Null
        }
    );
    assert_eq!(harness.driver.starts("worker"), 1);
    assert_eq!(harness.driver.starts("choice_work"), 1);
    assert_eq!(harness.driver.starts("choice_other"), 0);
    assert_eq!(harness.driver.starts("loop_check"), 2);
    assert_eq!(harness.driver.starts("map_check"), 2);
    assert!(harness.driver.max_active() >= 2);
    let contexts = harness.driver.state().contexts.clone();
    let loop_contexts = contexts
        .iter()
        .filter(|(reference, _)| reference.node.as_str() == "loop_check")
        .map(|(_, dispatch)| (dispatch.attempt, dispatch.iteration))
        .collect::<Vec<_>>();
    assert_eq!(loop_contexts, vec![(1, 1), (1, 2)]);
    assert!(
        contexts
            .iter()
            .filter(|(reference, _)| reference.node.as_str() != "loop_check")
            .all(|(_, dispatch)| dispatch.attempt == 1 && dispatch.iteration == 0)
    );
    let mapped = contexts
        .iter()
        .filter(|(reference, _)| reference.node.as_str() == "map_check")
        .map(|(reference, _)| reference.node_instance)
        .collect::<BTreeSet<_>>();
    assert_eq!(mapped.len(), 2);
    let stored = stored_run(&harness.ledger).await;
    let loop_visits = stored
        .snapshot
        .executions
        .values()
        .filter(|execution| execution.reference.node.as_str() == "loop_check")
        .collect::<Vec<_>>();
    assert_eq!(loop_visits.len(), 2);
    assert_eq!(
        loop_visits.assert_at(0).reference.node_instance,
        loop_visits.assert_at(1).reference.node_instance
    );
    assert_ne!(
        loop_visits.assert_at(0).reference.execution,
        loop_visits.assert_at(1).reference.execution
    );
    assert!(loop_visits.iter().all(|visit| visit.attempt.get() == 1));
}

#[tokio::test]
async fn profile_success_voids_child_siblings_before_dispatching_parent_work() {
    let child = json!({
        "kind":"par","name":"child","state":null_type(),
        "branches":[
            {
                "kind":"seq","name":"return_path","state":null_type(),
                "children":[step("fast",1_000),succeed("child_done")],"promotedStatePaths":[]
            },
            step("slow",1_000)
        ],
        "join":{"kind":"all"},"promotedStatePaths":[]
    });
    let body = json!({
        "kind":"seq","name":"child_root","state":null_type(),
        "children":[child,succeed("child_fallback")],"promotedStatePaths":[]
    });
    let root = sequence(
        vec![
            json!({"kind":"profile","name":"call","profile":"child","initialInput":null_type(),"body":body}),
            step("after_call", 1_000),
            succeed("done"),
        ],
        null_type(),
    );
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let driver = FakeDriver::scripted([
        ("fast", vec![Behavior::Together(barrier.clone())]),
        (
            "slow",
            vec![Behavior::CancelAfterStart(
                barrier,
                NodeRunnerError::Cancelled,
            )],
        ),
    ]);
    let harness = harness(graph(root, null_type()), Value::Null, driver).await;

    assert_eq!(
        harness.supervisor.drive().await.assert_value(),
        TerminalResult::Succeeded {
            output: Value::Null
        },
    );
    let tail = harness
        .ledger
        .snapshot_and_tail(&RunId::new("run-supervisor-test"), None)
        .await
        .assert_value();
    let relevant = tail
        .events
        .iter()
        .filter_map(|event| match &event.event {
            RunEvent::ExecutionVoided { reference, .. } if reference.node.as_str() == "slow" => {
                Some("void")
            }
            RunEvent::NodeStarted { reference, .. } if reference.node.as_str() == "after_call" => {
                Some("parent")
            }
            RunEvent::Terminal { .. } => Some("terminal"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(relevant, ["void", "parent", "terminal"]);
    assert_eq!(harness.driver.cancellations("slow"), 1);
}

#[tokio::test]
async fn fail_terminal_is_reduced_without_dispatch() {
    let harness = harness(
        graph(
            json!({"kind": "fail", "name": "failed", "reason": "rejected"}),
            null_type(),
        ),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    assert_eq!(
        harness
            .supervisor
            .drive()
            .await
            .assert_value_with("terminal"),
        TerminalResult::Failed {
            context: None,
            reason: EnumLabel::new("rejected").assert_value_with("label")
        }
    );
    assert_eq!(harness.driver.state().starts.len(), 0);
}

async fn assert_cancelling_parallel_join(
    case: &str,
    join: Value,
    branches: Vec<Value>,
    expected: TerminalResult,
) {
    let driver = FakeDriver::scripted([
        (
            "fast",
            vec![Behavior::Complete {
                delay: Duration::from_millis(5),
                outcome: verifier_outcome("accepted"),
            }],
        ),
        (
            "second",
            vec![Behavior::Complete {
                delay: Duration::from_millis(10),
                outcome: verifier_outcome("accepted"),
            }],
        ),
        ("slow", vec![Behavior::Hang]),
    ]);
    let harness = harness(parallel(join, branches), Value::Null, driver).await;
    let actual = harness.supervisor.drive().await.assert_value_with(case);
    match expected {
        TerminalResult::Failed {
            reason: expected, ..
        } => assert!(
            matches!(actual,
            TerminalResult::Failed { reason, .. } if reason == expected),
            "{case}"
        ),
        _ => assert_eq!(actual, expected),
    }
    assert!(harness.driver.max_active() >= 2, "{case}");
    assert_eq!(harness.driver.cancellations("slow"), 1, "{case}");
    let stored = stored_run(&harness.ledger).await;
    let slow = stored
        .snapshot
        .executions
        .values()
        .find(|execution| execution.reference.node.as_str() == "slow")
        .assert_value_with("slow execution");
    assert!(matches!(slow.state, NodeState::Voided { .. }));
    let execution = slow.reference.execution;
    let tail = harness
        .ledger
        .snapshot_and_tail(&RunId::new("run-supervisor-test"), None)
        .await
        .assert_value_with("tail");
    let log = tail
        .events
        .iter()
        .position(|event| {
            matches!(
                event.event,
                RunEvent::SafeLog {
                    execution: Some(observed),
                    ..
                } if observed == execution
            )
        })
        .assert_value_with("loser log");
    let voided = tail
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.event,
                RunEvent::ExecutionVoided { reference, .. }
                    if reference.execution == execution
            )
        })
        .assert_value_with("void settlement");
    let terminal = tail
        .events
        .iter()
        .position(|event| matches!(event.event, RunEvent::Terminal { .. }))
        .assert_value_with("terminal event");
    assert!(log < voided && voided < terminal, "{case}");
}

#[tokio::test]
async fn every_parallel_join_observes_completion_order_and_cancels_losers() {
    for (case, join, branches) in [
        (
            "any",
            json!({"kind": "any"}),
            vec![verifier("fast", 1_000), verifier("slow", 1_000)],
        ),
        (
            "quorum",
            json!({"kind": "quorum", "count": 2}),
            vec![
                verifier("fast", 1_000),
                verifier("second", 1_000),
                verifier("slow", 1_000),
            ],
        ),
        (
            "first",
            json!({"kind": "first", "when": {"kind":"any","guards":[signal_guard("fast", "accepted"),signal_guard("slow", "accepted")]}}),
            vec![verifier("fast", 1_000), verifier("slow", 1_000)],
        ),
    ] {
        assert_cancelling_parallel_join(
            case,
            join,
            branches,
            TerminalResult::Succeeded {
                output: Value::Null,
            },
        )
        .await;
    }

    let all = harness(
        parallel(
            json!({"kind": "all"}),
            vec![verifier("fast", 1_000), verifier("slow", 1_000)],
        ),
        Value::Null,
        FakeDriver::scripted([
            (
                "fast",
                vec![Behavior::Complete {
                    delay: Duration::from_millis(5),
                    outcome: verifier_outcome("accepted"),
                }],
            ),
            (
                "slow",
                vec![Behavior::Complete {
                    delay: Duration::from_millis(15),
                    outcome: verifier_outcome("accepted"),
                }],
            ),
        ]),
    )
    .await;
    assert!(matches!(
        all.supervisor.drive().await.assert_value_with("all"),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(all.driver.cancellations("slow"), 0);
}

#[tokio::test]
async fn parallel_branch_terminals_cancel_and_drain_siblings_before_finishing() {
    for join in [
        json!({"kind":"all"}),
        json!({"kind":"any"}),
        json!({"kind":"quorum","count":2}),
        json!({"kind":"first","when":signal_guard("slow", "accepted")}),
    ] {
        for succeeds in [false, true] {
            let terminal = if succeeds {
                succeed("branch_done")
            } else {
                json!({"kind":"fail","name":"blocked","reason":"review_rejected"})
            };
            let branch = json!({"kind":"seq","name":"gate","state":null_type(),
                "promotedStatePaths":[],"children":[verifier("fast", 1_000),terminal]});
            let expected = if succeeds {
                TerminalResult::Succeeded {
                    output: Value::Null,
                }
            } else {
                TerminalResult::Failed {
                    reason: EnumLabel::new("review_rejected").assert_value(),
                    context: None,
                }
            };
            assert_cancelling_parallel_join(
                "branch terminal",
                join.clone(),
                vec![branch, verifier("slow", 1_000)],
                expected,
            )
            .await;
        }
    }
}

use openengine_cluster_testkit::assertions::{AssertAt, AssertValue};

#[tokio::test]
async fn concurrent_writers_run_in_parallel_and_map_with_distinct_sessions() {
    let mapped = graph(
        sequence(
            vec![
                json!({
                    "kind": "map", "name": "each", "state": record_type(),
                    "body": step("mapped", 1_000),
                    "over": {"source": "state", "path": ["items"]},
                    "maxItems": 2, "promotedStatePaths": []
                }),
                succeed("done"),
            ],
            record_type(),
        ),
        record_type(),
    );
    let cases = [
        (
            parallel(
                json!({"kind": "all"}),
                vec![step("left", 1_000), step("right", 1_000)],
            ),
            Value::Null,
            ["left", "right"],
        ),
        (
            parallel(
                json!({"kind": "all"}),
                vec![step("left", 1_000), verifier("right", 1_000)],
            ),
            Value::Null,
            ["left", "right"],
        ),
        (
            mapped,
            json!({"items": ["one", "two"]}),
            ["mapped", "mapped"],
        ),
    ];
    for scope in [SessionScope::Execution, SessionScope::NodeInstance] {
        for (graph, input, nodes) in &cases {
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            let driver = FakeDriver::default();
            for node in nodes {
                driver
                    .state()
                    .scripts
                    .entry((*node).to_owned())
                    .or_default()
                    .push_back(Behavior::Together(barrier.clone()));
            }
            let harness =
                harness_with_session_scope(graph.clone(), input.clone(), driver, scope).await;
            assert_eq!(
                harness
                    .supervisor
                    .drive()
                    .await
                    .assert_value_with("concurrent writers complete"),
                TerminalResult::Succeeded {
                    output: Value::Null
                }
            );
            assert_eq!(harness.driver.max_active(), 2);
            assert_eq!(harness.sessions.opened.load(Ordering::SeqCst), 2);
            assert_eq!(harness.sessions.closed.load(Ordering::SeqCst), 2);
            let stored = stored_run(&harness.ledger).await;
            assert_eq!(stored.snapshot.executions.len(), 2);
            let identities: BTreeSet<_> = stored
                .snapshot
                .executions
                .values()
                .map(|execution| execution.reference.node_instance)
                .collect();
            assert_eq!(identities.len(), 2);
            assert!(
                stored
                    .snapshot
                    .executions
                    .values()
                    .all(|execution| { matches!(execution.state, NodeState::Completed { .. }) })
            );
        }
    }
}
