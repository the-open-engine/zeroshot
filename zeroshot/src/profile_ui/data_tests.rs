use super::*;
use async_trait::async_trait;
use openengine_cluster_protocol::{
    GraphSpec, WorkerDescriptor, WorkerRef, WorkerOutcome, WorkerErrorCode,
};
use openengine_cluster_server::{
    admission::{GraphVerifier, VerifiedGraph},
    graph_verifier::ProductionGraphVerifier,
    worker_registry::{WorkerRegistry, WorkerRegistryError},
};
use openengine_cluster_testkit::assertions::AssertValue;
use crate::full_v1_reducer::{
    FullV1Reducer, ReductionInput, Reduction, Decision, DurableExecution, DurableExecutionState,
    StructuralOccurrence, HistoryPosition, NodeInstanceId, ExecutionId, TerminalProjection,
};

fn record(fields: Value) -> Value {
    json!({"kind":"record","fields":fields})
}
fn worker(name: &str) -> Value {
    json!({"kind":"step","name":name,"worker":format!("{name}@1"),"instructions":"Produce a useful result.","input":{"kind":"record","fields":{}},"output":record(json!({"text":{"type":{"kind":"string"},"required":true}})),"inputBindings":[],"writeBindings":[],"attempts":1})
}
fn seq(name: &str, children: Vec<Value>) -> Value {
    json!({"kind":"seq","name":name,"state":record(json!({})),"children":children,"promotedStatePaths":[]})
}
fn done() -> Value {
    json!({"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]})
}
fn graph(children: Vec<Value>) -> Value {
    json!({"profile":"openengine.graph.full/v1","initialInput":record(json!({})),"policy":{"policy":"policy.native-v2@1","default":"deny"},"root":seq("run",children)})
}
fn incomplete_route(source: Value) -> Value {
    graph(vec![
        source,
        worker("consumer"),
        seq("unfinished", Vec::new()),
    ])
}
fn action(graph: Value, action: Value) -> Result<Value, String> {
    let request = serde_json::from_value(
        json!({"graph":graph,"runtime":{"harness":"","nodes":{}},"action":action}),
    )
    .map_err(|error| error.to_string())?;
    let result = apply(request).map_err(|error| error.message)?;
    Ok(result.graph)
}
fn connect(graph: Value, source: &str, target: &str) -> Value {
    action(graph,json!({"kind":"connect","target":{"node":target,"input":"text"},"source":{"kind":"node_output","node":source,"channel":"out","path":["text"]}})).assert_value()
}
fn protect(graph: Value, node: &str) -> Value {
    let request = serde_json::from_value(
        json!({"graph":graph,"runtime":{},"action":{"kind":"protect","node":node}}),
    )
    .assert_value();
    let result = super::super::outcomes::apply(request)
        .map_err(|error| error.message)
        .assert_value();
    serde_json::to_value(result).assert_value()["graph"].clone()
}

struct Workers(Value);
#[async_trait]
impl WorkerRegistry for Workers {
    async fn resolve(&self, worker: &WorkerRef) -> Result<WorkerDescriptor, WorkerRegistryError> {
        let nodes = index(&self.0).map_err(|_| WorkerRegistryError::NotFound {
            worker: worker.clone(),
        })?;
        let node = nodes
            .values()
            .filter_map(|node| self.0.pointer(&node.pointer))
            .find(|node| node["worker"] == worker.as_str())
            .ok_or_else(|| WorkerRegistryError::NotFound {
                worker: worker.clone(),
            })?;
        serde_json::from_value(json!({
            "worker":worker,"graphProfiles":["openengine.graph.full/v1"],"binding":{"protocol":"fixture","version":"1","profile":"fixture.worker/v1"},
            "contract":{"input":node["input"],"output":node["output"],"verifier":if node["kind"]=="verifier"{json!({"signals":node["signals"],"diagnostic":node["diagnostic"]})}else{Value::Null},"errors":["timeout","crash","malformed","refusal"]},
            "capabilityPolicy":{"autonomy":"strict","permissionPolicy":"policy.strict@1"},"artifactProfile":{"allowedTypeIds":["openengine.result@1"],"allowedMediaTypes":["application/json"],"minimumRedaction":"internal"},"credentialRequirements":[]
        })).map_err(|_|WorkerRegistryError::NotFound{worker:worker.clone()})
    }
}
async fn verify(graph: Value) -> Result<VerifiedGraph, String> {
    let typed: GraphSpec =
        serde_json::from_value(graph.clone()).map_err(|error| error.to_string())?;
    ProductionGraphVerifier::new(Workers(graph))
        .verify(&typed)
        .await
        .map_err(|error| error.to_string())
}
fn settled(name: &str, id: u64, outcome: WorkerOutcome) -> DurableExecution {
    DurableExecution {
        dispatch_position: HistoryPosition::new(id * 2 - 1).assert_value(),
        node_instance: NodeInstanceId::new(id).assert_value(),
        execution: ExecutionId::new(id).assert_value(),
        occurrence: StructuralOccurrence {
            node: NodeName::new(name).assert_value(),
            map_indices: Vec::new(),
        },
        attempt: openengine_cluster_protocol::PositiveInteger::new(1).assert_value(),
        input: Value::Null,
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(id * 2).assert_value(),
            outcome,
        },
    }
}
fn success(text: &str) -> WorkerOutcome {
    WorkerOutcome::Verified {
        output: json!({"text":text}),
        artifacts: Vec::new(),
    }
}
fn reduce(graph: &VerifiedGraph, input: &Value, history: &[DurableExecution]) -> Reduction {
    let result = FullV1Reducer::native_v2(graph).reduce(ReductionInput {
        initial_input: input,
        executions: history,
        next_node_instance: history.len() as u64 + 1,
        next_execution: history.len() as u64 + 1,
    });
    assert!(result.is_ok(), "{result:?}");
    result.assert_value()
}

#[tokio::test]
async fn serial_optional_route_authors_success_guard_and_dispatches_actual_output() {
    let unguarded = connect(
        graph(vec![worker("draft"), worker("read"), done()]),
        "draft",
        "read",
    );
    assert!(verify(unguarded.clone()).await.is_ok());
    let source = protect(unguarded, "draft");
    let verified = verify(source).await;
    assert!(verified.is_ok(), "{verified:?}");
    let verified = verified.assert_value();
    let result = reduce(
        &verified,
        &json!({}),
        &[settled("draft", 1, success("actual plan"))],
    );
    assert!(result.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,input,..} if occurrence.node.as_str()=="read" && input==&json!({"text":"actual plan"}))));
    let failed = reduce(
        &verified,
        &json!({}),
        &[settled(
            "draft",
            1,
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
        )],
    );
    assert!(matches!(
        failed.terminal,
        Some(TerminalProjection::Failed { .. })
    ));
}

#[tokio::test]
async fn parallel_writers_promote_only_actual_values_and_fail_before_missing_read() {
    let par = json!({"kind":"par","name":"plans","state":record(json!({})),"branches":[worker("venue"),worker("agenda")],"promotedStatePaths":[],"join":{"kind":"all"}});
    let source = connect(
        protect(graph(vec![par, worker("packet"), done()]), "plans"),
        "venue",
        "packet",
    );
    let verified = verify(source).await;
    assert!(verified.is_ok(), "{verified:?}");
    let verified = verified.assert_value();
    let result = reduce(
        &verified,
        &json!({}),
        &[
            settled("venue", 1, success("venue plan")),
            settled("agenda", 2, success("agenda plan")),
        ],
    );
    assert!(result.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,input,..} if occurrence.node.as_str()=="packet" && input==&json!({"text":"venue plan"}))));
    let failed = reduce(
        &verified,
        &json!({}),
        &[
            settled(
                "venue",
                1,
                WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
            ),
            settled("agenda", 2, success("agenda plan")),
        ],
    );
    assert!(matches!(
        failed.terminal,
        Some(TerminalProjection::Failed { .. })
    ));
    assert!(!failed.decisions.iter().any(|decision|matches!(decision,Decision::Promote{values,..} if values.iter().any(|value|value.value==json!("venue plan")))));
}

#[test]
fn run_inputs_sync_bindings_and_reject_overwritten_caller_data() {
    let source = action(
        graph(vec![worker("draft"), done()]),
        json!({"kind":"run_input_field","name":"task","type":{"kind":"string"},"required":true}),
    )
    .assert_value();
    let source=action(source,json!({"kind":"connect","target":{"node":"draft","input":"brief"},"source":{"kind":"run_input","path":["task"]}})).assert_value();
    let renamed=action(source.clone(),json!({"kind":"run_input_field","before":"task","name":"request","type":{"kind":"string"},"required":true})).assert_value();
    assert!(renamed["initialInput"]["fields"].get("task").is_none());
    assert_eq!(
        renamed["root"]["children"][0]["inputBindings"][0]["value"]["path"],
        json!(["request"])
    );
    serde_json::from_value::<GraphSpec>(renamed.clone()).assert_value();
    assert!(renamed["root"].get("bindings").is_none());
    assert!(renamed["root"].get("inputBindings").is_none());
    assert!(renamed["root"]["children"][0].get("bindings").is_none());
    assert!(
        renamed["root"]["children"][0]
            .get("promotedStatePaths")
            .is_none()
    );
    let removed =
        action(renamed, json!({"kind":"remove_run_input","name":"request"})).assert_value();
    assert!(
        removed["root"]["children"][0]["input"]["fields"]
            .get("brief")
            .is_none()
    );
    serde_json::from_value::<GraphSpec>(removed).assert_value();
    let mut overwritten = source;
    overwritten["root"]["children"][0]["writeBindings"] =
        json!([{"value":{"node":"draft","channel":"out","path":["text"]},"target":["task"]}]);
    assert!(action(overwritten,json!({"kind":"connect","target":{"node":"draft","input":"original"},"source":{"kind":"run_input","path":["task"]}})).is_err());
}

#[test]
fn adding_and_renaming_blank_run_input_preserves_node_fields() {
    let source = action(
        graph(Vec::new()),
        json!({"kind":"run_input_field","name":"field","type":{"kind":"string"},"required":true}),
    )
    .assert_value();
    let mut source = action(source, json!({"kind":"run_input_field","before":"field","name":"request","type":{"kind":"string"},"required":true})).assert_value();
    assert_eq!(source["root"].as_object().assert_value().len(), 5);
    // A blank sequence is intentionally an incomplete draft; adding its first activity must
    // produce a native graph without cleanup of keys from unrelated node variants.
    source["root"]["children"] = json!([worker("draft"), done()]);
    serde_json::from_value::<GraphSpec>(source).assert_value();
}

#[test]
fn collection_setup_supports_empty_map_drafts_and_item_fields() {
    let items = record(json!({"name":{"type":{"kind":"string"},"required":true}}));
    let map = json!({"kind":"map","name":"items_map","state":record(json!({})),"body":seq("body",Vec::new()),"over":{"source":"state","path":["missing"]},"maxItems":8,"promotedStatePaths":[]});
    let source=action(graph(vec![map,done()]),json!({"kind":"run_input_field","name":"items","type":{"kind":"array","items":items},"required":true})).assert_value();
    let mut source=action(source,json!({"kind":"map_collection","node":"items_map","source":{"kind":"run_input","path":["items"]}})).assert_value();
    source["root"]["children"][0]["body"]["children"] = json!([worker("draft")]);
    let source=action(source,json!({"kind":"connect","target":{"node":"draft","input":"name"},"source":{"kind":"map_item","path":["name"]}})).assert_value();
    assert_eq!(
        source["root"]["children"][0]["body"]["children"][0]["inputBindings"][0]["value"],
        json!({"source":"item","path":["name"]})
    );
}

#[test]
fn editing_run_collection_refreshes_item_types_and_clears_removed_sources() {
    let map = json!({"kind":"map","name":"items_map","state":record(json!({})),"body":seq("body",vec![worker("draft")]),"over":null,"maxItems":8,"promotedStatePaths":[]});
    let item = record(json!({"name":{"type":{"kind":"string"},"required":true}}));
    let source = action(graph(vec![map, done()]),json!({"kind":"run_input_field","name":"items","type":{"kind":"array","items":item},"required":true})).assert_value();
    let source = action(source,json!({"kind":"map_collection","node":"items_map","source":{"kind":"run_input","path":["items"]}})).assert_value();
    let source = action(source,json!({"kind":"connect","target":{"node":"draft","input":"name"},"source":{"kind":"map_item","path":["name"]}})).assert_value();
    let new_item = record(json!({"name":{"type":{"kind":"boolean"},"required":true}}));
    let changed = action(source.clone(),json!({"kind":"run_input_field","before":"items","name":"attendees","type":{"kind":"array","items":new_item},"required":true})).assert_value();
    assert_eq!(
        changed["root"]["children"][0]["over"]["path"],
        json!(["attendees"])
    );
    assert_eq!(
        changed["root"]["children"][0]["body"]["children"][0]["input"]["fields"]["name"]["type"],
        json!({"kind":"boolean"})
    );
    serde_json::from_value::<GraphSpec>(changed).assert_value();
    let removed_item = action(source.clone(),json!({"kind":"run_input_field","before":"items","name":"items","type":{"kind":"array","items":record(json!({}))},"required":true})).assert_value();
    assert_eq!(
        removed_item["root"]["children"][0]["body"]["children"][0]["inputBindings"],
        json!([])
    );
    assert!(
        removed_item["root"]["children"][0]["body"]["children"][0]["input"]["fields"]
            .get("name")
            .is_some()
    );
    let scalar = action(source,json!({"kind":"run_input_field","before":"items","name":"items","type":{"kind":"string"},"required":true})).assert_value();
    assert!(scalar["root"]["children"][0]["over"].is_null());
    assert_eq!(
        scalar["root"]["children"][0]["body"]["children"][0]["inputBindings"],
        json!([])
    );
}

#[test]
fn replacing_input_is_atomic_and_reserved_slots_do_not_capture_old_references() {
    let mut source = graph(vec![worker("draft"), worker("read"), done()]);
    source["root"]["children"][1]["inputBindings"] =
        json!([{"target":["old"],"value":{"source":"state","path":["__ui_data_1"]}}]);
    let source = connect(source, "draft", "read");
    assert!(
        source["root"]["state"]["fields"]
            .get("__ui_data_2")
            .is_some()
    );
    let replaced = connect(source, "draft", "read");
    let nodes = index(&replaced).assert_value();
    let bindings = get(&replaced, &nodes, "read").assert_value()["inputBindings"]
        .as_array()
        .assert_value();
    assert_eq!(
        bindings
            .iter()
            .filter(|binding| binding["target"] == json!(["text"]))
            .count(),
        1
    );
}

#[tokio::test]
async fn map_outputs_are_ordered_complete_arrays_including_empty_input() {
    let state =
        record(json!({"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}));
    let map = json!({"kind":"map","name":"items_map","state":state,"body":seq("body",vec![worker("draft")]),"over":{"source":"state","path":["items"]},"maxItems":8,"promotedStatePaths":[]});
    let mut source = graph(vec![map, worker("packet"), done()]);
    source["initialInput"] = state.clone();
    source["root"]["state"] = state;
    let source = connect(protect(source, "items_map"), "draft", "packet");
    let verified = verify(source).await;
    assert!(verified.is_ok(), "{verified:?}");
    let verified = verified.assert_value();
    let mut first = settled("draft", 1, success("first"));
    first.occurrence.map_indices = vec![0];
    let mut second = settled("draft", 2, success("second"));
    second.occurrence.map_indices = vec![1];
    let result = reduce(
        &verified,
        &json!({"items":[null,null]}),
        &[second.clone(), first.clone()],
    );
    assert!(result.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,input,..} if occurrence.node.as_str()=="packet" && input==&json!({"text":["first","second"]}))));
    let empty = reduce(&verified, &json!({"items":[]}), &[]);
    assert!(empty.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,input,..} if occurrence.node.as_str()=="packet" && input==&json!({"text":[]}))));
    second.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(4).assert_value(),
        outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
    };
    let failed = reduce(&verified, &json!({"items":[null,null]}), &[first, second]);
    assert!(matches!(
        failed.terminal,
        Some(TerminalProjection::Failed { .. })
    ));
}

#[tokio::test]
async fn loop_outputs_use_the_final_round_and_do_not_promote_previous_round_after_error() {
    let mut draft = worker("draft");
    draft["kind"] = json!("verifier");
    draft["signals"] = json!({"verdict":["accepted","rejected"]});
    draft["diagnostic"] = json!({"kind":"null"});
    let until = json!({"kind":"any","guards":[{"kind":"in","value":{"name":"draft","source":"signal","field":"verdict"},"labels":["accepted"]},{"kind":"in","value":{"name":"draft","source":"error","field":null},"labels":["crash","timeout","malformed","refusal"]}]});
    let loop_node = json!({"kind":"loop","name":"rounds","state":record(json!({})),"body":seq("body",vec![draft]),"until":until,"maxIterations":3,"promotedStatePaths":[]});
    let checkpoint = json!({"kind":"choice","name":"result","state":record(json!({})),"branches":[{"when":{"kind":"in","value":{"name":"draft","source":"error","field":null},"labels":["crash","timeout","malformed","refusal"]},"node":{"kind":"fail","name":"failed","reason":"draft_failed"}}],"otherwise":seq("continuation",vec![worker("packet"),done()]),"promotedStatePaths":[]});
    let source = connect(graph(vec![loop_node, checkpoint]), "draft", "packet");
    let verified = verify(source).await;
    assert!(verified.is_ok(), "{verified:?}");
    let verified = verified.assert_value();
    let outcome = |text: &str, verdict: &str| WorkerOutcome::Verifier {
        output: json!({"text":text}),
        signals: BTreeMap::from([(
            FieldName::new("verdict").assert_value(),
            openengine_cluster_protocol::EnumLabel::new(verdict).assert_value(),
        )]),
        diagnostic: Value::Null,
        artifacts: Vec::new(),
    };
    let first = settled("draft", 1, outcome("old round", "rejected"));
    let mut second = settled("draft", 2, outcome("final round", "accepted"));
    second.node_instance = first.node_instance;
    let result = reduce(&verified, &json!({}), &[first.clone(), second.clone()]);
    assert!(result.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,input,..} if occurrence.node.as_str()=="packet" && input==&json!({"text":"final round"}))));
    second.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(4).assert_value(),
        outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
    };
    let failed = reduce(&verified, &json!({}), &[first, second]);
    assert!(matches!(
        failed.terminal,
        Some(TerminalProjection::Failed { .. })
    ));
    assert!(!failed.decisions.iter().any(|decision|matches!(decision,Decision::Promote{node,values,..} if node.as_str()=="rounds" && !values.is_empty())));
}

#[tokio::test]
async fn common_choice_outputs_are_safe_only_after_selected_branch_succeeds() {
    let mut router = worker("router");
    router["kind"] = json!("verifier");
    router["signals"] = json!({"verdict":["accepted","rejected"]});
    router["diagnostic"] = json!({"kind":"null"});
    let choice = json!({"kind":"choice","name":"choose","state":record(json!({})),"branches":[{"when":{"kind":"in","value":{"name":"router","source":"signal","field":"verdict"},"labels":["accepted"]},"node":worker("left")}],"otherwise":worker("right"),"promotedStatePaths":[]});
    let unguarded = connect(
        graph(vec![
            router.clone(),
            choice.clone(),
            worker("packet"),
            done(),
        ]),
        "choose",
        "packet",
    );
    assert!(verify(unguarded).await.is_ok());
    let errors = |name: &str| json!({"kind":"in","value":{"name":name,"source":"error","field":null},"labels":["timeout","crash","malformed","refusal"]});
    let checkpoint = json!({"kind":"choice","name":"result","state":record(json!({})),"branches":[{"when":{"kind":"any","guards":[errors("left"),errors("right")]},"node":{"kind":"fail","name":"failed","reason":"draft_failed"}}],"otherwise":seq("continuation",vec![worker("packet"),done()]),"promotedStatePaths":[]});
    let unsafe_source = action(
        graph(vec![router.clone(), choice.clone(), checkpoint.clone()]),
        json!({"kind":"connect","target":{"node":"packet","input":"text"},"source":{"kind":"node_output","node":"choose","channel":"out","path":["text"]}}),
    );
    assert!(unsafe_source.is_err());
    let source = connect(
        protect(
            graph(vec![router, choice, worker("packet"), done()]),
            "choose",
        ),
        "choose",
        "packet",
    );
    super::super::validate_profile(
        &serde_json::from_value(source.clone()).assert_value(),
        &serde_json::from_value(json!({
            "harness":"codex", "provider":"openai", "size":"small", "nodes": {
                "router":{"kind":"agent","model":"opaque-model"},
                "left":{"kind":"agent","model":"opaque-model"},
                "right":{"kind":"agent","model":"opaque-model"},
                "packet":{"kind":"agent","model":"opaque-model"}
            }
        }))
        .assert_value(),
    )
    .await
    .map_err(|error| error.message)
    .assert_value();
    let verified = verify(source).await;
    assert!(verified.is_ok(), "{verified:?}");
    let verified = verified.assert_value();
    for (verdict, name) in [("accepted", "left"), ("rejected", "right")] {
        let routed = settled(
            "router",
            1,
            WorkerOutcome::Verifier {
                output: json!({"text":"route"}),
                signals: BTreeMap::from([(
                    FieldName::new("verdict").assert_value(),
                    openengine_cluster_protocol::EnumLabel::new(verdict).assert_value(),
                )]),
                diagnostic: Value::Null,
                artifacts: Vec::new(),
            },
        );
        let result = reduce(
            &verified,
            &json!({}),
            &[routed.clone(), settled(name, 2, success(name))],
        );
        assert!(result.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,input,..} if occurrence.node.as_str()=="packet" && input==&json!({"text":name}))));
        for error in [
            WorkerErrorCode::Crash,
            WorkerErrorCode::Malformed,
            WorkerErrorCode::Refusal,
            WorkerErrorCode::Timeout,
        ] {
            let failed = reduce(
                &verified,
                &json!({}),
                &[
                    routed.clone(),
                    settled(name, 2, WorkerOutcome::declared_failure(error)),
                ],
            );
            assert!(matches!(
                failed.terminal,
                Some(TerminalProjection::Failed { .. })
            ));
            assert!(!failed.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,..} if occurrence.node.as_str()=="packet")));
        }
    }
}

#[tokio::test]
async fn repeated_choice_uses_current_route_and_ignores_an_unselected_previous_error() {
    let mut router = worker("router");
    router["kind"] = json!("verifier");
    router["signals"] = json!({"verdict":["accepted","rejected"]});
    router["diagnostic"] = json!({"kind":"null"});
    let route = json!({"kind":"in","value":{"name":"router","source":"signal","field":"verdict"},"labels":["accepted"]});
    let choice = json!({"kind":"choice","name":"choose","state":record(json!({})),"branches":[{"when":route,"node":worker("left")}],"otherwise":worker("right"),"promotedStatePaths":[]});
    let protected = protect(
        graph(vec![
            router.clone(),
            choice.clone(),
            worker("packet"),
            done(),
        ]),
        "choose",
    );
    let checkpoint = protected["root"]["children"][2].clone();
    let repeated = json!({"kind":"loop","name":"rounds","state":record(json!({})),"body":seq("round",vec![router,choice]),"maxIterations":2,"promotedStatePaths":[]});
    let source = connect(graph(vec![repeated, checkpoint]), "choose", "packet");
    let verified = verify(source).await;
    assert!(verified.is_ok(), "{verified:?}");
    let verified = verified.assert_value();
    let route_outcome = |verdict: &str| WorkerOutcome::Verifier {
        output: json!({"text":"route"}),
        signals: BTreeMap::from([(
            FieldName::new("verdict").assert_value(),
            openengine_cluster_protocol::EnumLabel::new(verdict).assert_value(),
        )]),
        diagnostic: Value::Null,
        artifacts: Vec::new(),
    };
    let mut later_route = settled("router", 3, route_outcome("rejected"));
    later_route.node_instance = NodeInstanceId::new(1).assert_value();
    let history = vec![
        settled("router", 1, route_outcome("accepted")),
        settled(
            "left",
            2,
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
        ),
        later_route,
        settled("right", 4, success("current right")),
    ];
    let result = reduce(&verified, &json!({}), &history);
    assert!(result.terminal.is_none());
    assert!(result.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,input,..} if occurrence.node.as_str()=="packet"&&input==&json!({"text":"current right"}))));
}

fn feedback_fixture() -> Value {
    let state = record(json!({"feedback":{"required":true,"type":{"kind":"string"}}}));
    let mut donor = worker("draft");
    donor["input"] = record(json!({"feedback":{"required":true,"type":{"kind":"string"}}}));
    donor["inputBindings"] =
        json!([{"target":["feedback"],"value":{"source":"state","path":["feedback"]}}]);
    let mut repair = worker("repair");
    repair["writeBindings"] =
        json!([{"target":["feedback"],"value":{"node":"repair","channel":"out","path":["text"]}}]);
    let mut body = seq("round", vec![donor, worker("assemble"), repair]);
    body["state"] = state.clone();
    body["promotedStatePaths"] = json!([["feedback"]]);
    let repeated = json!({"kind":"loop","name":"rounds","state":state,"body":body,"maxIterations":2,"promotedStatePaths":[]});
    let mut source = graph(vec![repeated, done()]);
    source["root"]["state"] = state;
    source
}

fn reuse_feedback(source: Value) -> Result<Value, String> {
    action(
        source,
        json!({"kind":"connect","target":{"node":"assemble","input":"repairPlan"},"source":{"kind":"loop_input","node":"draft","path":["feedback"]}}),
    )
}

#[tokio::test]
async fn existing_loop_feedback_is_reused_without_new_slots_writes_or_defaults() {
    let source = feedback_fixture();
    let changed = reuse_feedback(source.clone()).assert_value();
    let expected = index(&source).assert_value();
    let actual = index(&changed).assert_value();
    for name in expected.keys().filter(|name| *name != "assemble") {
        assert_eq!(
            get(&source, &expected, name).assert_value()["state"],
            get(&changed, &actual, name).assert_value()["state"]
        );
        assert_eq!(
            get(&source, &expected, name).assert_value()["writeBindings"],
            get(&changed, &actual, name).assert_value()["writeBindings"]
        );
        assert_eq!(
            get(&source, &expected, name).assert_value()["promotedStatePaths"],
            get(&changed, &actual, name).assert_value()["promotedStatePaths"]
        );
    }
    assert_eq!(changed["initialInput"], source["initialInput"]);
    let assembler = get(&changed, &actual, "assemble").assert_value();
    assert_eq!(
        assembler["inputBindings"],
        json!([{"target":["repairPlan"],"value":{"source":"state","path":["feedback"]}}])
    );
    let verified = verify(changed).await.assert_value();
    let mut first_draft = settled("draft", 1, success("draft"));
    first_draft.input = json!({"feedback":""});
    let first = reduce(&verified, &json!({}), &[first_draft.clone()]);
    assert!(first.decisions.iter().any(|decision| matches!(decision, Decision::Dispatch { occurrence, input, .. } if occurrence.node.as_str()=="assemble" && input==&json!({"repairPlan":""}))));
    let mut second_draft = settled("draft", 4, success("revision"));
    second_draft.node_instance = NodeInstanceId::new(1).assert_value();
    second_draft.input = json!({"feedback":"current run feedback"});
    let mut first_assembly = settled("assemble", 2, success("kit"));
    first_assembly.input = json!({"repairPlan":""});
    let second = reduce(
        &verified,
        &json!({}),
        &[
            first_draft,
            first_assembly,
            settled("repair", 3, success("current run feedback")),
            second_draft,
        ],
    );
    assert!(second.decisions.iter().any(|decision| matches!(decision, Decision::Dispatch { occurrence, input, .. } if occurrence.node.as_str()=="assemble" && input==&json!({"repairPlan":"current run feedback"}))));
}

#[test]
fn feedback_reuse_rejects_missing_returns_optional_state_and_competing_write_paths() {
    for damage in 0..7 {
        let mut source = feedback_fixture();
        match damage {
            0 => source["root"]["children"][0]["body"]["promotedStatePaths"] = json!([]),
            1 => {
                source["root"]["children"][0]["body"]["state"]["fields"]["feedback"]["required"] =
                    json!(false)
            }
            2 => {
                source["root"]["children"][0]["body"]["children"][0]["input"]["fields"]["feedback"]
                    ["required"] = json!(false)
            }
            3 => {
                source["root"]["children"][0]["body"]["children"][1]["writeBindings"] = json!([{"target":["feedback","nested"],"value":{"node":"assemble","channel":"out","path":["text"]}}])
            }
            4 => {
                source["root"]["children"][0]["body"]["children"][2]["writeBindings"][0]["value"]
                    ["node"] = json!("draft")
            }
            5 => source["root"]["state"]["fields"]["feedback"]["type"] = json!({"kind":"number"}),
            6 => {
                source["root"]["children"][0]["body"]["children"][2]["output"]["fields"]["text"]["required"] =
                    json!(false)
            }
            _ => unreachable!(),
        }
        assert!(
            reuse_feedback(source).is_err(),
            "damage {damage} must reject"
        );
    }
}

#[test]
fn feedback_reuse_rejects_an_uninitialized_type_and_accepts_a_real_caller_initial_value() {
    let mut source = feedback_fixture();
    let nodes = index(&source).assert_value();
    for name in ["run", "rounds", "round"] {
        get_mut(&mut source, &nodes, name).assert_value()["state"]["fields"]["feedback"]["type"] =
            json!({"kind":"integer"});
    }
    get_mut(&mut source, &nodes, "draft").assert_value()["input"]["fields"]["feedback"]["type"] =
        json!({"kind":"integer"});
    get_mut(&mut source, &nodes, "repair").assert_value()["output"]["fields"]["text"]["type"] =
        json!({"kind":"integer"});
    assert!(reuse_feedback(source.clone()).is_err());
    source["initialInput"] =
        record(json!({"feedback":{"required":true,"type":{"kind":"integer"}}}));
    assert!(reuse_feedback(source).is_ok());
}

#[test]
fn feedback_reuse_cannot_cross_a_nested_loop_map_or_parallel_writer_peer() {
    for kind in ["loop", "map", "par"] {
        let mut source = feedback_fixture();
        let body = &mut source["root"]["children"][0]["body"];
        let receiver = body["children"][1].clone();
        let state = body["state"].clone();
        if kind == "par" {
            let repair = body["children"].as_array_mut().assert_value().remove(2);
            body["children"][1] = json!({"kind":"par","name":"peers","state":state,"branches":[receiver,repair],"join":{"kind":"all"},"promotedStatePaths":[["feedback"]]});
        } else {
            body["children"][1] = json!({"kind":kind,"name":"inner","state":state,"body":receiver,"maxIterations":2,"maxItems":2,"over":{"source":"state","path":["items"]},"promotedStatePaths":[]});
        }
        assert!(reuse_feedback(source).is_err(), "{kind} must reject");
    }
}

#[tokio::test]
async fn required_parallel_outputs_move_existing_failure_handling_before_a_new_consumer() {
    let errors = |name: &str| json!({"kind":"in","value":{"name":name,"source":"error","field":null},"labels":["timeout","crash","malformed","refusal"]});
    let parallel = |name: &str, names: &[&str]| json!({"kind":"par","name":name,"state":record(json!({})),"branches":names.iter().map(|name|worker(name)).collect::<Vec<_>>(),"join":{"kind":"all"},"promotedStatePaths":[]});
    let handler = json!({"kind":"choice","name":"result","state":record(json!({})),"branches":[{"when":{"kind":"any","guards":[errors("website"),errors("email"),errors("brand"),errors("quality")]},"node":{"kind":"fail","name":"failed","reason":"campaign_failed"}}],"otherwise":done(),"promotedStatePaths":[]});
    let source = graph(vec![
        parallel("drafts", &["website", "email"]),
        worker("assemble"),
        parallel("reviews", &["brand", "quality"]),
        handler,
    ]);
    let source = protect(source, "assemble");
    let source = connect(source, "website", "assemble");
    let source = action(source,json!({"kind":"connect","target":{"node":"assemble","input":"email"},"source":{"kind":"node_output","node":"email","channel":"out","path":["text"]}})).assert_value();
    let verified = verify(source).await.assert_value();
    let succeeded = reduce(
        &verified,
        &json!({}),
        &[
            settled("website", 1, success("website.md")),
            settled("email", 2, success("email.md")),
        ],
    );
    assert!(succeeded.decisions.iter().any(|decision| matches!(decision, Decision::Dispatch { occurrence, input, .. } if occurrence.node.as_str()=="assemble" && input==&json!({"text":"website.md","email":"email.md"}))));
    let failed = reduce(
        &verified,
        &json!({}),
        &[
            settled(
                "website",
                1,
                WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
            ),
            settled(
                "email",
                2,
                WorkerOutcome::Verified {
                    output: json!({"text":"email.md"}),
                    artifacts: Vec::new(),
                },
            ),
        ],
    );
    assert!(matches!(
        failed.terminal,
        Some(TerminalProjection::Failed { reason }) if reason.as_str() == "campaign_failed"
    ));
    assert!(!failed.decisions.iter().any(|decision| matches!(decision, Decision::Dispatch { occurrence, .. } if occurrence.node.as_str()=="assemble")));
    let mut assembly = settled(
        "assemble",
        3,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
    );
    assembly.input = json!({"text":"website.md","email":"email.md"});
    let failed = reduce(
        &verified,
        &json!({}),
        &[
            settled("website", 1, success("website.md")),
            settled("email", 2, success("email.md")),
            assembly,
        ],
    );
    assert!(
        matches!(failed.terminal, Some(TerminalProjection::Failed { reason }) if reason.as_str() == "execution_failed")
    );
}

#[tokio::test]
async fn existing_feedback_can_be_shared_from_parallel_readers_and_verifier_diagnostics() {
    let mut source = feedback_fixture();
    let body = &mut source["root"]["children"][0]["body"];
    let donor = body["children"][0].clone();
    let state = body["state"].clone();
    body["children"][0] = json!({"kind":"par","name":"drafts","state":state,"branches":[donor,worker("other")],"join":{"kind":"all"},"promotedStatePaths":[]});
    let repair = &mut body["children"][2];
    repair["kind"] = json!("verifier");
    repair["signals"] = json!({});
    repair["diagnostic"] = repair["output"].clone();
    repair["writeBindings"][0]["value"]["channel"] = json!("diagnostic");
    let changed = reuse_feedback(source).assert_value();
    assert!(verify(changed).await.is_ok());
}

#[test]
fn incomplete_other_groups_defer_required_output_protection_without_fabricating_nodes() {
    let source = graph(vec![
        worker("draft"),
        worker("read"),
        seq("unfinished", Vec::new()),
    ]);
    let connected = connect(source, "draft", "read");
    let nodes = index(&connected).assert_value();
    assert_eq!(nodes.len(), 4);
    assert_eq!(
        get(&connected, &nodes, "unfinished").assert_value()["children"],
        json!([])
    );
    assert_eq!(
        get(&connected, &nodes, "read").assert_value()["inputBindings"][0]["value"]["path"],
        json!(["__ui_data_1"])
    );
}

fn assert_action_error(graph: Value, action_value: Value, expected: &str) {
    let error = action(graph, action_value).err().assert_value();
    assert!(
        error.contains(expected),
        "expected {expected:?}, got {error:?}"
    );
}

#[test]
fn malformed_node_inventories_fail_closed_before_data_authoring() {
    let remove = |node: &str| json!({"kind":"remove_input","target":{"node":node,"input":"text"}});

    let mut missing_name = graph(vec![worker("draft"), done()]);
    missing_name["root"]["children"][0]
        .as_object_mut()
        .assert_value()
        .remove("name");
    assert_action_error(missing_name, remove("draft"), "A node has no name");

    let mut missing_kind = graph(vec![worker("draft"), done()]);
    missing_kind["root"]["children"][0]
        .as_object_mut()
        .assert_value()
        .remove("kind");
    assert_action_error(missing_kind, remove("draft"), "A node has no type");

    assert_action_error(
        graph(vec![worker("duplicate"), worker("duplicate"), done()]),
        remove("duplicate"),
        "Node names must be unique",
    );
    let mut invalid_name = graph(vec![worker("draft"), done()]);
    invalid_name["root"]["children"][0]["name"] = json!("bad/name");
    assert_action_error(invalid_name, remove("draft"), "invalid characters");
}

#[test]
fn invalid_data_sources_and_mappings_report_their_specific_contract_failure() {
    let remove = |node: &str| json!({"kind":"remove_input","target":{"node":node,"input":"text"}});

    assert_action_error(
        graph(vec![worker("draft"), done()]),
        json!({
            "kind":"map_collection", "node":"draft",
            "source":{"kind":"run_input","path":["items"]}
        }),
        "Select a map",
    );
    let map = || {
        json!({
            "kind":"map", "name":"items_map", "state":record(json!({})),
            "body":seq("body", vec![worker("inside")]), "over":null,
            "maxItems":8, "promotedStatePaths":[]
        })
    };
    let mut scalar_collection = graph(vec![map(), done()]);
    scalar_collection["initialInput"] = record(json!({
        "items":{"type":{"kind":"string"},"required":true}
    }));
    assert_action_error(
        scalar_collection,
        json!({
            "kind":"map_collection", "node":"items_map",
            "source":{"kind":"run_input","path":["items"]}
        }),
        "Select a list",
    );

    for (path, field, expected) in [
        (json!([]), None, "Select a named field"),
        (
            json!(["optional"]),
            Some(json!({"optional":{"type":{"kind":"string"},"required":false}})),
            "selected output must be required",
        ),
        (
            json!(["missing"]),
            Some(json!({"present":{"type":{"kind":"string"},"required":true}})),
            "selected field no longer exists",
        ),
    ] {
        let mut source = graph(vec![worker("draft"), done()]);
        if let Some(fields) = field {
            source["initialInput"] = record(fields);
        }
        assert_action_error(
            source,
            json!({
                "kind":"connect", "target":{"node":"draft","input":"text"},
                "source":{"kind":"run_input","path":path}
            }),
            expected,
        );
    }

    assert_action_error(
        graph(vec![worker("draft"), done()]),
        json!({
            "kind":"connect", "target":{"node":"draft","input":"text"},
            "source":{"kind":"map_item","path":["text"]}
        }),
        "inside a map",
    );
    assert_action_error(
        graph(vec![map(), done()]),
        json!({
            "kind":"connect", "target":{"node":"inside","input":"text"},
            "source":{"kind":"map_item","path":["text"]}
        }),
        "Configure the map list first",
    );

    assert_action_error(
        graph(vec![worker("draft"), done()]),
        remove("run"),
        "Select an agent input or run result",
    );
    let mut scalar_input = graph(vec![worker("draft"), done()]);
    scalar_input["root"]["children"][0]["input"] = json!({"kind":"string"});
    assert_action_error(
        scalar_input,
        remove("draft"),
        "This input uses a scalar schema",
    );
    let mut invalid_mappings = graph(vec![worker("draft"), done()]);
    invalid_mappings["root"]["children"][0]["inputBindings"] = json!({});
    assert_action_error(invalid_mappings, remove("draft"), "Invalid data mappings");
}

#[test]
fn output_routes_reject_ambiguous_or_conditionally_available_values() {
    let incomplete = || seq("unfinished", Vec::new());
    let error_guard = || {
        json!({
            "kind":"in", "value":{"name":"missing","source":"error","field":null},
            "labels":["crash"]
        })
    };
    let connect_from = |node: &str| {
        json!({
            "kind":"connect", "target":{"node":"consumer","input":"value"},
            "source":{"kind":"node_output","node":node,"channel":"out","path":["text"]}
        })
    };

    let ambiguous = seq("producers", vec![worker("first"), worker("second")]);
    assert_action_error(
        graph(vec![ambiguous, worker("consumer"), incomplete()]),
        connect_from("producers"),
        "Select an unambiguous producing node",
    );

    let failed = json!({"kind":"fail","name":"failed","reason":"failed"});
    let terminal_choice = json!({
        "kind":"choice", "name":"finished", "state":record(json!({})),
        "branches":[{"when":error_guard(),"node":failed}],
        "otherwise":done(), "promotedStatePaths":[]
    });
    assert_action_error(
        graph(vec![terminal_choice, worker("consumer"), incomplete()]),
        connect_from("finished"),
        "This decision has no continuing output",
    );

    let branch_map = json!({
        "kind":"map", "name":"mapped", "state":record(json!({})),
        "body":worker("mapped_value"), "over":null, "maxItems":2,
        "promotedStatePaths":[]
    });
    let differently_shaped = json!({
        "kind":"choice", "name":"shapes", "state":record(json!({})),
        "branches":[{"when":error_guard(),"node":branch_map}],
        "otherwise":worker("scalar_value"), "promotedStatePaths":[]
    });
    assert_action_error(
        graph(vec![differently_shaped, worker("consumer"), incomplete()]),
        connect_from("shapes"),
        "Selected outputs have different collection shapes",
    );

    let conditional = json!({
        "kind":"choice", "name":"conditional", "state":record(json!({})),
        "branches":[{"when":error_guard(),"node":worker("selected")}],
        "otherwise":worker("other"), "promotedStatePaths":[]
    });
    assert_action_error(
        graph(vec![conditional, worker("consumer"), incomplete()]),
        connect_from("selected"),
        "Select a common decision output",
    );

    let not_joined = json!({
        "kind":"par", "name":"race", "state":record(json!({})),
        "branches":[worker("winner"), worker("peer")],
        "join":{"kind":"any"}, "promotedStatePaths":[]
    });
    assert_action_error(
        graph(vec![not_joined, worker("consumer"), incomplete()]),
        connect_from("winner"),
        "Select an output from a completed group",
    );

    assert_action_error(
        graph(vec![worker("consumer"), worker("later"), incomplete()]),
        connect_from("later"),
        "Select an earlier output",
    );
}

#[test]
fn map_item_and_channel_routes_fail_at_their_specific_boundary() {
    let map = |state: Value, over: Value| {
        json!({
            "kind":"map", "name":"items_map", "state":state,
            "body":seq("body", vec![worker("inside")]), "over":over,
            "maxItems":8, "promotedStatePaths":[]
        })
    };
    let item_action = json!({
        "kind":"connect", "target":{"node":"inside","input":"item"},
        "source":{"kind":"map_item","path":["name"]}
    });

    assert_action_error(
        graph(vec![
            map(record(json!({})), json!({"source":"item","path":["name"]})),
            done(),
        ]),
        item_action.clone(),
        "Configure the map list first",
    );
    assert_action_error(
        graph(vec![
            map(
                record(json!({"items":{"type":{"kind":"string"},"required":true}})),
                json!({"source":"state","path":["items"]}),
            ),
            done(),
        ]),
        item_action,
        "Configure the map list first",
    );

    for (channel, path, expected) in [
        (
            "signal",
            json!(["verdict"]),
            "Select an output from this node",
        ),
        (
            "diagnostic",
            json!(["message"]),
            "Select an output from this node",
        ),
    ] {
        assert_action_error(
            incomplete_route(worker("source")),
            json!({
                "kind":"connect", "target":{"node":"consumer","input":"value"},
                "source":{"kind":"node_output","node":"source","channel":channel,"path":path}
            }),
            expected,
        );
    }

    let mut verifier = worker("verifier");
    verifier["kind"] = json!("verifier");
    verifier["signals"] = json!({"verdict":["accepted"]});
    verifier["diagnostic"] = record(json!({}));
    assert_action_error(
        incomplete_route(verifier),
        json!({
            "kind":"connect", "target":{"node":"consumer","input":"value"},
            "source":{"kind":"node_output","node":"verifier","channel":"signal","path":["missing"]}
        }),
        "The selected outcome no longer exists",
    );
}

#[test]
fn run_input_rewrites_reject_collisions_and_stale_nested_bindings() {
    let request_type = record(json!({
        "detail":{"type":{"kind":"string"},"required":true}
    }));
    let source = action(
        graph(vec![worker("consumer"), done()]),
        json!({
            "kind":"run_input_field", "name":"request", "type":request_type,
            "required":true
        }),
    )
    .assert_value();
    let source = action(
        source,
        json!({
            "kind":"connect", "target":{"node":"consumer","input":"detail"},
            "source":{"kind":"run_input","path":["request","detail"]}
        }),
    )
    .assert_value();
    assert_action_error(
        source.clone(),
        json!({"kind":"run_input_field","before":"request","name":"request","type":record(json!({})),"required":true}),
        "A connected field no longer exists",
    );

    let mut malformed_path = source.clone();
    malformed_path["root"]["children"][0]["inputBindings"][0]["value"]["path"] =
        json!(["request", 1]);
    assert_action_error(
        malformed_path,
        json!({"kind":"run_input_field","before":"request","name":"request","type":record(json!({})),"required":true}),
        "Invalid connected input",
    );

    let with_other = action(
        source.clone(),
        json!({"kind":"run_input_field","name":"other","type":{"kind":"string"},"required":true}),
    )
    .assert_value();
    assert_action_error(
        with_other,
        json!({"kind":"run_input_field","before":"request","name":"other","type":{"kind":"string"},"required":true}),
        "That input name already exists",
    );

    let mut malformed_write = source;
    malformed_write["root"]["children"][0]["writeBindings"] = json!([{
        "target":{}, "value":{"node":"consumer","channel":"out","path":["text"]}
    }]);
    assert_action_error(
        malformed_write,
        json!({"kind":"remove_run_input","name":"request"}),
        "This input is also authored as writable state",
    );
}
