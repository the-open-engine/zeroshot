use super::*;
use openengine_cluster_protocol::WorkerErrorCode;
use openengine_cluster_testkit::assertions::AssertError;

async fn optional_map_collection(prefilled: bool) -> VerifiedGraph {
    let state = json!({
        "kind":"record","fields":{
            "items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true},
            "results":{"type":{"kind":"array","items":{"kind":"integer"}},"required":false}
        }
    });
    let mapped = json!({
        "kind":"map","name":"collect","state":state.clone(),
        "over":{"source":"state","path":["items"]},"maxItems":2,
        "promotedStatePaths":[["results"]],
        "body":promoted_integer_step("inspect","results")
    });
    let route = json!({
        "kind":"choice","name":"outcome","state":state.clone(),
        "branches":[
            {"when":{"kind":"in","value":{"name":"collect","source":"group","field":"overflow"},"labels":["overflow"]},
             "node":{"kind":"fail","name":"too_many","reason":"too_many_items"}},
            {"when":{"kind":"k_of_map","count":1,"value":{"name":"inspect","source":"error","field":null},
                     "labels":["timeout","crash","malformed","refusal"]},
             "node":{"kind":"fail","name":"inspection_failed","reason":"inspection_failed"}}
        ],
        "otherwise":{"kind":"succeed","name":"done",
            "output":{"kind":"record","fields":{"results":{"type":{"kind":"array","items":{"kind":"integer"}},"required":true}}},
            "bindings":[{"target":["results"],"value":{"source":"state","path":["results"]}}]
        },
        "promotedStatePaths":[]
    });
    let mut children = vec![mapped, route];
    if prefilled {
        children.insert(
            0,
            json!({
                "kind":"map","name":"prior_collection","state":state.clone(),
                "over":{"source":"state","path":["items"]},"maxItems":2,
                "promotedStatePaths":[["results"]],
                "body":promoted_integer_step("prior_inspect","results")
            }),
        );
    }
    let root = json!({
        "kind":"seq","name":"run","state":state.clone(),
        "children":children,"promotedStatePaths":[]
    });
    reducer_test_support::verified_graph(root, state, true).await
}

#[tokio::test]
async fn failed_map_item_routes_its_error_without_fabricating_a_collected_value() {
    let graph = optional_map_collection(false).await;
    for code in [
        WorkerErrorCode::Crash,
        WorkerErrorCode::Malformed,
        WorkerErrorCode::Refusal,
        WorkerErrorCode::Timeout,
    ] {
        let history = [
            settled(
                SettledSpec::new(1, 1, "inspect")
                    .map_indices(vec![0])
                    .position(1),
                success(7),
            ),
            settled(
                SettledSpec::new(2, 2, "inspect")
                    .map_indices(vec![1])
                    .position(2),
                WorkerOutcome::declared_failure(code),
            ),
        ];
        let result = FullV1Reducer::native_v2(&graph).reduce(ReductionInput {
            initial_input: &json!({"items":[null,null]}),
            executions: &history,
            next_node_instance: 3,
            next_execution: 3,
        });
        assert!(
            result.is_ok(),
            "map must reach its authored error route: {result:?}"
        );
        let reduction = result.assert_value();
        assert_eq!(
            reduction.terminal,
            Some(TerminalProjection::Failed {
                reason: "inspection_failed".parse().assert_value(),
            })
        );
        assert!(
            !reduction.decisions.iter().any(|decision| matches!(decision,
                Decision::Promote { node, .. } if node.as_str() == "collect"
            )),
            "an incomplete result array must not be promoted"
        );
    }
}

#[tokio::test]
async fn successful_and_empty_maps_still_collect_every_item_in_input_order() {
    let graph = optional_map_collection(false).await;
    assert_eq!(
        reduce(&graph, &json!({"items":[]}), &[]).terminal,
        Some(TerminalProjection::Succeeded {
            output: json!({"results":[]})
        })
    );
    let history = [
        settled(
            SettledSpec::new(2, 2, "inspect")
                .map_indices(vec![1])
                .position(1),
            success(20),
        ),
        settled(
            SettledSpec::new(1, 1, "inspect")
                .map_indices(vec![0])
                .position(2),
            success(10),
        ),
    ];
    assert_eq!(
        reduce(&graph, &json!({"items":[null,null]}), &history).terminal,
        Some(TerminalProjection::Succeeded {
            output: json!({"results":[10,20]})
        })
    );
}

#[tokio::test]
async fn failed_map_collection_never_reuses_a_prior_collection_as_an_item_result() {
    let graph = optional_map_collection(true).await;
    let history = [
        settled(
            SettledSpec::new(1, 1, "prior_inspect")
                .map_indices(vec![0])
                .position(1),
            success(100),
        ),
        settled(
            SettledSpec::new(2, 2, "prior_inspect")
                .map_indices(vec![1])
                .position(2),
            success(200),
        ),
        settled(
            SettledSpec::new(3, 3, "inspect")
                .map_indices(vec![0])
                .position(3),
            success(10),
        ),
        settled(
            SettledSpec::new(4, 4, "inspect")
                .map_indices(vec![1])
                .position(4),
            WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
        ),
    ];
    let reduction = reduce(&graph, &json!({"items":[null,null]}), &history);
    assert_eq!(
        reduction.terminal,
        Some(TerminalProjection::Failed {
            reason: "inspection_failed".parse().assert_value(),
        })
    );
    let collections = reduction
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Promote { node, values, .. }
                if node.as_str() == "prior_collection" || node.as_str() == "collect" =>
            {
                Some((node.as_str(), values))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].0, "prior_collection");
    assert_eq!(collections[0].1[0].value, json!([100, 200]));
}

#[tokio::test]
async fn map_collection_does_not_hide_a_malformed_successful_durable_output() {
    let graph = optional_map_collection(false).await;
    let history = [settled(
        SettledSpec::new(1, 1, "inspect")
            .map_indices(vec![0])
            .position(1),
        WorkerOutcome::Verified {
            output: json!({}),
            artifacts: Vec::new(),
        },
    )];
    let result = FullV1Reducer::native_v2(&graph).reduce(ReductionInput {
        initial_input: &json!({"items":[null]}),
        executions: &history,
        next_node_instance: 2,
        next_execution: 2,
    });
    assert_eq!(result.assert_error(), ReducerError::MissingSelectedValue);
}
