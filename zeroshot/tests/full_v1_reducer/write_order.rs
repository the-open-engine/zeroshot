use super::*;
use async_trait::async_trait;
use openengine_cluster_protocol::{WorkerDescriptor, WorkerRef};
use openengine_cluster_server::worker_registry::{WorkerRegistry, WorkerRegistryError};

fn data_schema() -> Value {
    json!({"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"integer"}},"required":true}}})
}
fn parent_output() -> Value {
    json!({"kind":"record","fields":{"value":{"type":data_schema(),"required":true}}})
}
struct ParentWorkers;
#[async_trait]
impl WorkerRegistry for ParentWorkers {
    async fn resolve(&self, worker: &WorkerRef) -> Result<WorkerDescriptor, WorkerRegistryError> {
        let original = TestWorkers { rich_outputs: true }.resolve(worker).await?;
        let mut descriptor = serde_json::to_value(original).unwrap();
        if worker.as_str() == "worker.parent@1" {
            descriptor["contract"]["output"] = parent_output();
        }
        Ok(serde_json::from_value(descriptor).unwrap())
    }
}

async fn ordered_graph(nested: bool, parent_last: bool) -> VerifiedGraph {
    let schema = json!({"kind":"record","fields":{"data":{"type":data_schema(),"required":true},"next_items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}});
    let mut copy = step("copy", 1);
    copy["writeBindings"] = json!([{"value":{"node":"copy","channel":"out","path":["value"]},"target":["data","items"]}]);
    let grow = json!({"kind":"map","name":"grow","state":schema,"body":copy,"over":{"source":"state","path":["next_items"]},"maxItems":1,"promotedStatePaths":[["data","items"]]});
    let mut replace = reducer_test_support::step_node("replace", 1, parent_output());
    replace["worker"] = json!("worker.parent@1");
    replace["writeBindings"] =
        json!([{"value":{"node":"replace","channel":"out","path":["value"]},"target":["data"]}]);
    let target = json!({"kind":"map","name":"target","state":schema,"body":verifier("a",1),"over":{"source":"state","path":["data","items"]},"maxItems":1,"promotedStatePaths":[]});
    let mut children = vec![step("gate", 1)];
    children.extend(if parent_last {
        vec![grow, replace]
    } else {
        vec![replace, grow]
    });
    children.push(target);
    let no_errors = json!({"kind":"not","guard":{"kind":"k_of_map","count":1,"value":{"name":"copy","source":"error","field":null},"labels":["crash","timeout","malformed","refusal"]}});
    let condition = json!({"kind":"all","guards":[no_errors,opposite_verdicts()]});
    let mut definition = race_definition(seq("left", children, &schema), condition, &schema);
    if nested {
        let outer = json!({"kind":"map","name":"outer","state":schema,"body":definition,"over":{"source":"state","path":["next_items"]},"maxItems":1,"promotedStatePaths":[]});
        definition = seq("outer_run", vec![outer, succeed("outer_done")], &schema);
    }
    let graph: GraphSpec = serde_json::from_value(json!({"profile":"openengine.graph.full/v1","initialInput":schema,"policy":{"policy":"policy.test@1","default":"deny"},"root":definition})).unwrap();
    ProductionGraphVerifier::new(ParentWorkers)
        .verify(&graph)
        .await
        .unwrap()
}

fn completed_replacements(scope: Vec<u64>, parent_last: bool) -> Vec<DurableExecution> {
    let mut item = scope.clone();
    item.push(0);
    let mut other = scope.clone();
    other.push(1);
    let mut history = vec![
        entry(1, "b", "rejected", scope.clone()),
        done(2, "gate", scope.clone()),
    ];
    let mut parent = done(if parent_last { 4 } else { 3 }, "replace", scope);
    if let DurableExecutionState::Settled {
        outcome: WorkerOutcome::Verified { output, .. },
        ..
    } = &mut parent.state
    {
        *output = json!({"value":{"items":[5,6]}});
    }
    let copy = done(if parent_last { 3 } else { 4 }, "copy", item.clone());
    history.extend(if parent_last {
        vec![copy, parent]
    } else {
        vec![parent, copy]
    });
    history.push(entry(5, "a", "accepted", item));
    if parent_last {
        history.push(entry(6, "a", "rejected", other));
    }
    history
}

#[tokio::test]
async fn overlapping_replacements_follow_authored_order_in_scalar_and_mapped_scopes() {
    for nested in [false, true] {
        for parent_last in [false, true] {
            let graph = ordered_graph(nested, parent_last).await;
            let input = json!({"data":{"items":[1]},"next_items":[null]});
            let scope = if nested { vec![0] } else { vec![] };
            let history = completed_replacements(scope, parent_last);
            assert_pending_dispatch(&one_slot(&graph, &input, &history[..1]), "gate");
            let complete = one_slot(&graph, &input, &history);
            if parent_last {
                assert!(
                    matches!(
                        complete.terminal,
                        Some(TerminalProjection::Succeeded { .. })
                    ),
                    "{complete:#?}"
                );
            } else {
                assert_no_satisfier(&complete);
            }
        }
    }
}
