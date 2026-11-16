use super::*;
use openengine_cluster_protocol::{FailReason, WorkerErrorCode};

fn error_guard(kind: &str) -> Value {
    let selector = json!({"source":"error","name":"work"});
    if kind == "k_of_n" {
        json!({"kind":kind,"count":1,"values":[selector],"labels":["crash"]})
    } else if kind == "k_of_map" {
        json!({"kind":kind,"count":1,"value":selector,"labels":["crash"]})
    } else {
        json!({"kind":"in","value":selector,"labels":["crash"]})
    }
}

fn checked(guard: Value) -> Value {
    json!({"kind":"choice","name":"check","state":{"kind":"record","fields":{}},
        "branches":[{"when":guard,"node":succeed("handled")}],
        "otherwise":succeed("done"),"promotedStatePaths":[]})
}

fn failed(code: WorkerErrorCode) -> DurableExecution {
    settled(
        SettledSpec::new(1, 1, "work"),
        WorkerOutcome::declared_failure(code),
    )
}

fn native(graph: &VerifiedGraph, input: &Value, history: &[DurableExecution]) -> Reduction {
    FullV1Reducer::native_v2(graph)
        .reduce(ReductionInput {
            initial_input: input,
            executions: history,
            next_node_instance: 10,
            next_execution: 10,
        })
        .assert_value()
}

fn assert_unhandled(reduction: &Reduction, code: WorkerErrorCode) {
    let Some(TerminalProjection::Failed {
        reason,
        context: Some(context),
    }) = &reduction.terminal
    else {
        panic!("expected unhandled failure: {reduction:?}");
    };
    assert_eq!(reason.as_str(), "unhandled");
    let cause = context.primary.as_ref().assert_value();
    assert!(cause.message.as_str().contains("work"));
    assert!(cause.message.as_str().contains(code.as_str()));
    assert!(
        !reduction
            .decisions
            .iter()
            .any(|d| matches!(d, Decision::Dispatch { .. }))
    );
}

#[tokio::test]
async fn unchecked_step_and_verifier_errors_fail_with_the_node_and_code() {
    for node in [step("work", 1), verifier("work", 1)] {
        let graph = verified(sequence("run", vec![node, succeed("done")]), json!({})).await;
        for code in [
            WorkerErrorCode::Crash,
            WorkerErrorCode::Timeout,
            WorkerErrorCode::Malformed,
            WorkerErrorCode::Refusal,
        ] {
            let history = [failed(code)];
            let result = native(&graph, &json!({}), &history);
            assert_unhandled(&result, code);
            assert_eq!(result, native(&graph, &json!({}), &history), "replay");
        }
    }
}

#[tokio::test]
async fn every_guard_form_reads_errors_independently_of_the_selected_route_and_label() {
    let direct = error_guard("in");
    for guard in [
        direct.clone(),
        error_guard("k_of_n"),
        json!({"kind":"all","guards":[direct.clone()]}),
        json!({"kind":"any","guards":[direct.clone()]}),
        json!({"kind":"not","guard":direct}),
    ] {
        let graph = verified(
            sequence("run", vec![step("work", 1), checked(guard)]),
            json!({}),
        )
        .await;
        for code in [WorkerErrorCode::Crash, WorkerErrorCode::Timeout] {
            assert!(matches!(
                native(&graph, &json!({}), &[failed(code)]).terminal,
                Some(TerminalProjection::Succeeded { .. })
            ));
        }
    }
}

#[tokio::test]
async fn loops_maps_and_choices_propagate_unhandled_and_all_guard_sites_count() {
    let schema = json!({"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}});
    for read in [false, true] {
        let loop_node = json!({"kind":"loop","name":"repeat","state":schema,"body":step("work",1),
            "maxIterations":2,"promotedStatePaths":[]});
        let map_node = json!({"kind":"map","name":"items_map","state":schema,"body":step("work",1),
            "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
        for (mut node, mapped) in [(loop_node, false), (map_node, true)] {
            let mut children = vec![];
            if read && !mapped {
                node["until"] = error_guard("in");
            }
            children.push(node);
            children.push(if read && mapped {
                checked(error_guard("k_of_map"))
            } else {
                succeed("done")
            });
            let mut root = sequence("run", children);
            root["state"] = schema.clone();
            let graph = reducer_test_support::verified_graph(root, schema.clone(), true).await;
            let mut error = failed(WorkerErrorCode::Crash);
            if mapped {
                error.occurrence.map_indices = vec![0];
            }
            let result = native(&graph, &json!({"items":[null,null]}), &[error]);
            if read {
                if mapped {
                    assert!(result.terminal.is_none(), "second item is still queued");
                } else {
                    assert!(matches!(
                        result.terminal,
                        Some(TerminalProjection::Succeeded { .. })
                    ));
                }
            } else {
                assert_unhandled(&result, WorkerErrorCode::Crash);
            }
        }
    }
    let mut route = checked(error_guard("in"));
    // The error read is replaced with a prior verifier signal; the selected branch contains work.
    route["branches"][0]["when"] = json!({"kind":"in","value":{"source":"signal","name":"gate","field":"verdict"},"labels":["accepted"]});
    route["branches"][0]["node"] = step("work", 1);
    route["otherwise"] = step("unused", 1);
    let graph = verified(
        sequence("run", vec![verifier("gate", 1), route, succeed("end")]),
        json!({}),
    )
    .await;
    let history = [
        settled(
            SettledSpec::new(1, 1, "gate").position(2),
            WorkerOutcome::Verifier {
                output: json!({}),
                signals: [(
                    "verdict".parse().assert_value(),
                    "accepted".parse().assert_value(),
                )]
                .into(),
                diagnostic: json!({}),
                artifacts: vec![],
            },
        ),
        settled(
            SettledSpec::new(2, 2, "work").position(4),
            WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
        ),
    ];
    assert_unhandled(
        &native(&graph, &json!({}), &history),
        WorkerErrorCode::Refusal,
    );
}

#[tokio::test]
async fn first_join_when_counts_as_an_error_read() {
    let graph = verified(sequence("run", vec![json!({"kind":"par","name":"race","state":{"kind":"record","fields":{}},
        "branches":[step("work",1),step("other",1)],"join":{"kind":"first","when":{"kind":"any","guards":[error_guard("in"),
            {"kind":"in","value":{"source":"error","name":"other"},"labels":["crash"]}]}},"promotedStatePaths":[]}),succeed("done")]),json!({})).await;
    assert!(matches!(
        native(&graph, &json!({}), &[failed(WorkerErrorCode::Crash)]).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
}

#[tokio::test]
async fn an_error_read_in_an_unselected_branch_still_preserves_authored_handling() {
    let route = json!({"kind":"choice","name":"route","state":{"kind":"record","fields":{}},
        "branches":[{"when":{"kind":"in","value":{"source":"signal","name":"gate","field":"verdict"},"labels":["accepted"]},"node":succeed("selected")}],
        "otherwise":checked(error_guard("in")),"promotedStatePaths":[]});
    let graph = verified(
        sequence("run", vec![step("work", 1), verifier("gate", 1), route]),
        json!({}),
    )
    .await;
    let history = [
        failed(WorkerErrorCode::Crash),
        settled(
            SettledSpec::new(2, 2, "gate").position(3),
            verdict("accepted"),
        ),
    ];
    assert!(matches!(
        native(&graph, &json!({}), &history).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
}

#[tokio::test]
async fn crash_retries_precede_unhandled_but_other_errors_do_not_retry() {
    let graph = verified(
        sequence("run", vec![step("work", 2), succeed("done")]),
        json!({}),
    )
    .await;
    let first = failed(WorkerErrorCode::Crash);
    let retry = native(&graph, &json!({}), std::slice::from_ref(&first));
    assert!(retry.terminal.is_none());
    assert!(
        retry
            .decisions
            .iter()
            .any(|d| matches!(d,Decision::Dispatch {attempt,..} if attempt.get()==2))
    );
    let mut last = failed(WorkerErrorCode::Crash);
    last.execution = ExecutionId::new(2).assert_value();
    last.attempt = PositiveInteger::new(2).assert_value();
    last.dispatch_position = HistoryPosition::new(3).assert_value();
    last.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(4).assert_value(),
        outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
    };
    assert_unhandled(
        &native(&graph, &json!({}), &[first.clone(), last.clone()]),
        WorkerErrorCode::Crash,
    );
    last.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(4).assert_value(),
        outcome: success(7),
    };
    assert!(matches!(
        native(&graph, &json!({}), &[first, last]).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
    for code in [
        WorkerErrorCode::Timeout,
        WorkerErrorCode::Malformed,
        WorkerErrorCode::Refusal,
    ] {
        assert_unhandled(&native(&graph, &json!({}), &[failed(code)]), code);
    }
}

#[test]
fn unhandled_remains_reserved_for_the_runtime() {
    assert!(FailReason::new("unhandled".parse().assert_value()).is_err());
}

#[tokio::test]
async fn exhausted_loop_evidence_does_not_replace_the_unhandled_primary_cause() {
    let repeated = json!({"kind":"loop","name":"repeat","state":{"kind":"record","fields":{}},
        "body":step("prior",1),"maxIterations":1,"promotedStatePaths":[]});
    let graph = verified(
        sequence("run", vec![repeated, step("work", 1), succeed("done")]),
        json!({}),
    )
    .await;
    let history = [
        settled(SettledSpec::new(1, 1, "prior").position(2), success(1)),
        settled(
            SettledSpec::new(2, 2, "work").position(4),
            WorkerOutcome::declared_failure(WorkerErrorCode::Malformed),
        ),
    ];
    let result = native(&graph, &json!({}), &history);
    assert_unhandled(&result, WorkerErrorCode::Malformed);
    let Some(TerminalProjection::Failed {
        context: Some(context),
        ..
    }) = result.terminal
    else {
        panic!("missing failure context")
    };
    assert_eq!(context.exhausted_loops.len(), 1);
    assert_eq!(context.exhausted_loops[0].node.as_str(), "repeat");
}
