use super::*;
use openengine_cluster_protocol::{RunStatus, RunStatusParams};
use crate::native_v2_observability::NativeV2Observability;

fn result(label: &str, message: &str) -> WorkerOutcome {
    WorkerOutcome::Verifier {
        output: Value::Null,
        signals: BTreeMap::from([(
            "readiness".parse().assert_value(),
            label.parse().assert_value(),
        )]),
        diagnostic: json!(message),
        artifacts: Vec::new(),
    }
}

fn complete(outcome: WorkerOutcome) -> Behavior {
    Behavior::Complete {
        delay: Duration::from_millis(50),
        outcome,
    }
}

fn review_graph() -> GraphSpec {
    let mut check = verifier("assessor", 10_000);
    check["signals"] = json!({"readiness": ["revise", "ready"]});
    check["diagnostic"] = json!({"kind":"string"});
    check["attempts"] = json!(2);
    graph(
        sequence(
            vec![
                json!({
                    "kind":"loop", "name":"refinement", "state":null_type(),
                    "maxIterations":3, "promotedStatePaths":[], "body":check,
                    "until":{"kind":"in","value":{"name":"assessor","source":"signal","field":"readiness"},"labels":["ready"]}
                }),
                json!({"kind":"fail","name":"halt","reason":"needs_followup"}),
            ],
            null_type(),
        ),
        null_type(),
    )
}

#[tokio::test]
async fn graph_defined_verifier_results_and_exact_loop_rounds_survive_retries_and_replay() {
    let harness = harness(
        review_graph(),
        Value::Null,
        FakeDriver::scripted([(
            "assessor",
            vec![
                complete(WorkerOutcome::declared_failure(WorkerErrorCode::Crash)),
                complete(result("revise", "first finding")),
                complete(WorkerOutcome::declared_failure(WorkerErrorCode::Crash)),
                complete(result("revise", "second finding")),
                complete(WorkerOutcome::declared_failure(WorkerErrorCode::Crash)),
                complete(result("revise", "latest missing boundary test")),
            ],
        )]),
    )
    .await;
    harness.supervisor.drive().await.assert_value();
    let stored = stored_run(&harness.ledger).await;
    let TerminalResult::Failed {
        context: Some(context),
        ..
    } = stored.snapshot.terminal.as_ref().assert_value()
    else {
        panic!("missing evidence")
    };
    assert!(context.primary.is_none());
    assert_eq!(context.verifiers.len(), 1);
    let latest = &context.verifiers[0];
    assert!(latest.message.as_str().contains("readiness"));
    assert!(
        latest
            .message
            .as_str()
            .contains("latest missing boundary test")
    );
    assert!(!latest.message.as_str().contains("first finding"));
    assert_eq!(
        latest.execution.as_ref().assert_value().node.as_str(),
        "assessor"
    );
    assert_eq!(context.exhausted_loops.len(), 1);
    assert_eq!(context.exhausted_loops[0].node.as_str(), "refinement");
    assert_eq!(context.exhausted_loops[0].completed_iterations.get(), 3);
    assert_eq!(stored.snapshot.executions.len(), 6);
    let tail = harness
        .ledger
        .snapshot_and_tail(&stored.snapshot.run_id, None)
        .await
        .assert_value();
    let matching = tail
        .events
        .iter()
        .find(|entry| Some(&entry.cursor) == latest.cursor.as_ref())
        .assert_value();
    assert!(
        matches!(&matching.event, RunEvent::SafeLog { line, .. } if line.as_str().contains("latest missing boundary test"))
    );
    assert!(tail.events.iter().any(|entry| matches!(&entry.event, RunEvent::SafeLog { line, .. } if line.as_str().contains("first finding"))));
    assert!(
        matches!(&tail.events.last().assert_value().event, RunEvent::Terminal { result } if Some(result) == stored.snapshot.terminal.as_ref())
    );
    let service = NativeV2Observability::new(harness.ledger.clone());
    let status = service
        .status(RunStatusParams {
            run_id: stored.snapshot.run_id.clone(),
        })
        .await
        .assert_value();
    assert!(
        matches!(status.status, RunStatus::Finished { terminal_result, .. } if Some(&terminal_result) == stored.snapshot.terminal.as_ref())
    );
}

#[tokio::test]
async fn verifier_evidence_does_not_suppress_a_later_cleanup_timeout() {
    let mut check = verifier("assessor", 100);
    check["signals"] = json!({"readiness": ["revise", "ready"]});
    check["diagnostic"] = json!({"kind":"string"});
    let harness = harness(
        graph(
            sequence(
                vec![
                    check,
                    json!({"kind":"fail","name":"halt","reason":"needs_followup"}),
                ],
                null_type(),
            ),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([(
            "assessor",
            vec![Behavior::VerifierThenHang(result(
                "ready",
                "check completed before cleanup",
            ))],
        )]),
    )
    .await;
    let TerminalResult::Failed {
        context: Some(context),
        ..
    } = harness.supervisor.drive().await.assert_value()
    else {
        panic!("missing failure context")
    };
    assert_eq!(context.verifiers.len(), 1);
    assert!(
        context.verifiers[0]
            .message
            .as_str()
            .contains("check completed before cleanup")
    );
    assert_eq!(context.recorded.len(), 1);
    let timeout = &context.recorded[0];
    assert!(timeout.message.as_str().contains("timeout after"));
    assert_eq!(timeout.execution, context.verifiers[0].execution);
    let mut logs = failure_logs::public_subscription(&harness).await;
    let mut matched = false;
    while let Some(log) = logs.recv().await.assert_value() {
        if Some(&log.cursor) == timeout.cursor.as_ref() {
            assert_eq!(log.record.message, timeout.message);
            assert_eq!(
                log.execution.as_ref(),
                timeout.execution.as_ref().map(|value| &value.execution)
            );
            matched = true;
        }
    }
    assert!(matched);
}

#[tokio::test]
async fn unreachable_parallel_reviews_retain_exhausted_loops_in_status_and_history() {
    for join in [json!({"kind":"all"}), json!({"kind":"any"})] {
        let branches = ["left", "right"].map(|side| {
            let name = format!("{side}_assessor");
            let mut check = verifier(&name, 10_000);
            check["signals"] = json!({"readiness":["revise","ready"]});
            check["diagnostic"] = json!({"kind":"string"});
            json!({"kind":"seq","name":format!("{side}_branch"),"state":null_type(),"promotedStatePaths":[],"children":[
                {"kind":"loop","name":format!("{side}_rounds"),"state":null_type(),"promotedStatePaths":[],"maxIterations":2,"body":check,
                 "until":{"kind":"in","value":{"name":name,"source":"signal","field":"readiness"},"labels":["ready"]}},
                {"kind":"choice","name":format!("{side}_result"),"state":null_type(),"promotedStatePaths":[],
                 "branches":[{"when":{"kind":"in","value":{"name":name,"source":"signal","field":"readiness"},"labels":["revise"]},
                              "node":{"kind":"fail","name":format!("{side}_halt"),"reason":"unfinished"}}],
                 "otherwise":step(&format!("{side}_continue"), 10_000)}
            ]})
        });
        let harness = harness(
            graph(sequence(vec![
                json!({"kind":"par","name":"reviews","state":null_type(),"promotedStatePaths":[],"join":join,"branches":branches}),
                json!({"kind":"fail","name":"halt","reason":"needs_followup"}),
            ], null_type()), null_type()),
            Value::Null,
            FakeDriver::scripted([
                ("left_assessor", vec![complete(result("revise", "first left finding")), complete(result("revise", "latest left finding"))]),
                ("right_assessor", vec![complete(result("revise", "first right finding")), complete(result("revise", "latest right finding"))]),
            ]),
        ).await;
        let terminal = harness.supervisor.drive().await.assert_value();
        let TerminalResult::Failed {
            context: Some(context),
            ..
        } = &terminal
        else {
            panic!("missing loop evidence")
        };
        let mut loops = context
            .exhausted_loops
            .iter()
            .map(|entry| (entry.node.as_str(), entry.completed_iterations.get()))
            .collect::<Vec<_>>();
        loops.sort_unstable();
        assert_eq!(loops, [("left_rounds", 2), ("right_rounds", 2)]);
        assert_eq!(context.verifiers.len(), 2);
        assert!(!context.incomplete);
        let tail = harness
            .ledger
            .snapshot_and_tail(&harness.supervisor.run_id, None)
            .await
            .assert_value();
        assert_eq!(
            tail.events
                .iter()
                .filter(|entry| matches!(
                    entry.event,
                    RunEvent::SafeLog {
                        diagnostic: Some(
                            openengine_cluster_protocol::FailureDiagnosticKind::Verifier
                        ),
                        ..
                    }
                ))
                .count(),
            4
        );
        assert!(
            matches!(&tail.events.last().assert_value().event, RunEvent::Terminal { result } if result == &terminal)
        );
        let status = NativeV2Observability::new(harness.ledger.clone())
            .status(RunStatusParams {
                run_id: harness.supervisor.run_id.clone(),
            })
            .await
            .assert_value();
        assert!(
            matches!(status.status, RunStatus::Finished { terminal_result, .. } if terminal_result == terminal)
        );
    }
}

#[tokio::test]
async fn loop_overflow_remains_incomplete_after_retained_visits_converge() {
    let names = [
        "assessor_a",
        "assessor_b",
        "assessor_c",
        "assessor_d",
        "assessor_e",
    ];
    for count in [4, 5] {
        let checks = names[..count].iter().map(|name| {
            let mut check = verifier(name, 10_000);
            check["signals"] = json!({"readiness":["revise","ready"]});
            check["diagnostic"] = json!({"kind":"string"});
            json!({"kind":"loop","name":format!("{name}_rounds"),"state":null_type(),"promotedStatePaths":[],
                "maxIterations":1,"body":check,
                "until":{"kind":"in","value":{"name":name,"source":"signal","field":"readiness"},"labels":["ready"]}})
        }).collect::<Vec<_>>();
        let harness = harness(
            graph(sequence(vec![
                json!({"kind":"loop","name":"initial_exhausted","state":null_type(),"promotedStatePaths":[],
                    "maxIterations":1,"body":step("initial_worker",10_000)}),
                json!({"kind":"loop","name":"outer_refinement","state":null_type(),"promotedStatePaths":[],
                    "maxIterations":2,"body":{"kind":"seq","name":"checks","state":null_type(),"promotedStatePaths":[],"children":checks},
                    "until":{"kind":"in","value":{"name":names[count-1],"source":"signal","field":"readiness"},"labels":["ready"]}}),
                json!({"kind":"fail","name":"halt","reason":"needs_followup"}),
            ], null_type()), null_type()),
            Value::Null,
            FakeDriver::scripted(std::iter::once(("initial_worker", vec![complete(success_for(NodeRole::Worker))])).chain(
                names[..count].iter().map(|name| (*name, vec![complete(result("revise","first finding")), complete(result("ready","all clear"))])),
            )),
        ).await;
        let terminal = harness.supervisor.drive().await.assert_value();
        let TerminalResult::Failed {
            context: Some(context),
            ..
        } = &terminal
        else {
            panic!("missing recorded evidence")
        };
        assert_eq!(context.incomplete, count == 5);
        assert_eq!(context.exhausted_loops.len(), usize::from(count == 4));
        if count == 4 {
            assert_eq!(
                context.exhausted_loops[0].node.as_str(),
                "initial_exhausted"
            );
            assert_eq!(context.exhausted_loops[0].completed_iterations.get(), 1);
        }
        assert_eq!(context.verifiers.len(), count);
        assert!(
            context
                .verifiers
                .iter()
                .all(|entry| entry.message.as_str().contains("all clear"))
        );
        assert!(context.primary.is_none() && context.recorded.is_empty());
        let stored = stored_run(&harness.ledger).await;
        assert_eq!(stored.snapshot.executions.len(), count * 2 + 1);
        assert_eq!(stored.snapshot.terminal.as_ref(), Some(&terminal));
        let tail = harness
            .ledger
            .snapshot_and_tail(&harness.supervisor.run_id, None)
            .await
            .assert_value();
        assert!(
            matches!(&tail.events.last().assert_value().event, RunEvent::Terminal { result } if result == &terminal)
        );
        let status = NativeV2Observability::new(harness.ledger.clone())
            .status(RunStatusParams {
                run_id: harness.supervisor.run_id.clone(),
            })
            .await
            .assert_value();
        assert!(
            matches!(status.status, RunStatus::Finished { terminal_result, .. } if terminal_result == terminal)
        );
    }
}

#[tokio::test]
async fn acceptance_replaces_earlier_result_and_does_not_claim_loop_exhaustion() {
    let harness = harness(
        review_graph(),
        Value::Null,
        FakeDriver::scripted([(
            "assessor",
            vec![
                complete(result("revise", "old finding")),
                complete(result("ready", "all clear")),
            ],
        )]),
    )
    .await;
    harness.supervisor.drive().await.assert_value();
    let stored = stored_run(&harness.ledger).await;
    let TerminalResult::Failed {
        context: Some(context),
        ..
    } = stored.snapshot.terminal.assert_value()
    else {
        panic!("missing context")
    };
    assert!(context.exhausted_loops.is_empty());
    assert_eq!(context.verifiers.len(), 1);
    assert!(context.verifiers[0].message.as_str().contains("all clear"));
    assert!(
        !context.verifiers[0]
            .message
            .as_str()
            .contains("old finding")
    );
}

#[tokio::test]
async fn map_paths_match_active_watch_logs_and_failure_evidence_including_nested_maps() {
    for nested in [false, true] {
        let state = record_type();
        let map = |name: &str, body: Value| {
            json!({"kind":"map", "name":name, "state":state,
            "over":{"source":"state","path":["items"]},"maxItems":3,"promotedStatePaths":[],"body":body})
        };
        let inner = map("fan", step("work", 10_000));
        let root = if nested { map("outer", inner) } else { inner };
        let failure_index = if nested { 4 } else { 1 };
        let expected = if nested { vec![1, 1] } else { vec![1] };
        let outcomes = (0..if nested { 9 } else { 3 })
            .map(|index| {
                complete(if index == failure_index {
                    WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
                } else {
                    success_for(NodeRole::Worker)
                })
            })
            .collect();
        let harness = harness(
            graph(
                sequence(
                    vec![
                        root,
                        json!({"kind":"fail","name":"stop","reason":"item_failed"}),
                    ],
                    state.clone(),
                ),
                state,
            ),
            json!({"items":["alpha","bravo","charlie"]}),
            FakeDriver::scripted([("work", outcomes)]),
        )
        .await;
        let service = NativeV2Observability::new(harness.ledger.clone());
        let drive = harness.spawn_drive();
        let mut observed = BTreeMap::new();
        let (_, mut watch) = service
            .watch(openengine_cluster_protocol::RunWatchParams {
                run_id: harness.supervisor.run_id.clone(),
                from_cursor: None,
            })
            .await
            .assert_value();
        while let Some(event) = watch.recv().await.assert_value() {
            if let RunStatus::Running { active_executions } = event.status {
                for execution in active_executions {
                    observed.insert(execution.execution.clone(), execution.map_indices);
                }
            }
        }
        drive.await.assert_value().assert_value();
        let stored = stored_run(&harness.ledger).await;
        let TerminalResult::Failed {
            context: Some(context),
            ..
        } = stored.snapshot.terminal.assert_value()
        else {
            panic!("no failure context")
        };
        let failure = &context.recorded[0];
        let identity = failure.execution.as_ref().assert_value();
        assert_eq!(identity.map_indices, expected);
        assert_eq!(observed.get(&identity.execution), Some(&expected));
        let mut logs = failure_logs::public_subscription(&harness).await;
        let mut matched = false;
        while let Some(log) = logs.recv().await.assert_value() {
            if Some(&log.cursor) == failure.cursor.as_ref() {
                assert_eq!(log.execution.as_ref(), Some(&identity.execution));
                assert_eq!(log.map_indices, expected);
                matched = true;
            }
        }
        assert!(matched);
    }
}
