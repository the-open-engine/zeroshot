//! Dedicated test process: a completed 2,048-item replay used to retain over 290 MiB.
use openengine_cluster_protocol::{PositiveInteger, WorkerOutcome};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use zeroshot_engine::full_v1_reducer::*;

#[path = "support/full_v1_reducer.rs"]
mod support;

#[tokio::test]
async fn completed_map_replay_retains_linear_scoped_results() {
    let state = json!({"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}});
    let body = support::step_node(
        "work",
        1,
        json!({"kind":"record","fields":{"value":{"type":{"kind":"integer"},"required":true}}}),
    );
    let mapped = json!({"kind":"map","name":"items_map","state":state,"body":body,
        "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
    let root = json!({"kind":"seq","name":"run","state":state,"children":[support::verifier("gate",1),mapped,
        {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}],"promotedStatePaths":[]});
    let graph = support::verified_graph(root, state, true).await;
    let count = 2048_u64;
    let history = (1..=count + 1)
        .map(|id| DurableExecution {
            dispatch_position: HistoryPosition::new(id * 2).assert_value(),
            node_instance: NodeInstanceId::new(id).assert_value(),
            execution: ExecutionId::new(id).assert_value(),
            occurrence: StructuralOccurrence {
                node: if id == 1 { "gate" } else { "work" }.parse().assert_value(),
                map_indices: if id == 1 { vec![] } else { vec![id - 2] },
            },
            attempt: PositiveInteger::new(1).assert_value(),
            input: Value::Null,
            state: DurableExecutionState::Settled {
                position: HistoryPosition::new(id * 2 + 1).assert_value(),
                outcome: if id == 1 {
                    WorkerOutcome::Verifier {
                        output: json!({}),
                        signals: [(
                            "verdict".parse().assert_value(),
                            "accepted".parse().assert_value(),
                        )]
                        .into_iter()
                        .collect(),
                        diagnostic: json!({}),
                        artifacts: vec![],
                    }
                } else {
                    WorkerOutcome::Verified {
                        output: json!({"value":1}),
                        artifacts: vec![],
                    }
                },
            },
        })
        .collect::<Vec<_>>();
    let input = json!({"items":vec![Value::Null;count as usize]});
    let result = FullV1Reducer::native_v2(&graph)
        .with_dispatch_limit(1)
        .reduce(ReductionInput {
            initial_input: &input,
            executions: &history,
            next_node_instance: count + 2,
            next_execution: count + 2,
        })
        .assert_value();
    assert_eq!(
        result.terminal,
        Some(TerminalProjection::Succeeded {
            output: Value::Null
        })
    );
    #[cfg(target_os = "linux")]
    {
        // This executable contains one test, so its high-water mark is isolated from other suites.
        let status = std::fs::read_to_string("/proc/self/status").assert_value();
        let peak = status
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:"))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
            .expect("Linux VmHWM in KiB");
        assert!(
            peak < 128 * 1024,
            "completed replay retained {peak} KiB; expected below 128 MiB"
        );
    }
}
