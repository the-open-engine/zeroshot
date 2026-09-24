use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

fn state() -> Value {
    json!({"kind":"record","fields":{}})
}
fn worker(name: &str) -> Value {
    json!({"kind":"step","name":name,"worker":format!("{name}@1"),"instructions":"Write the requested file.","input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1})
}
fn error(name: &str) -> Value {
    serde_json::to_value(
        worker_errors(&NodeName::new(name).assert_value(), false)
            .map_err(|e| e.message)
            .assert_value(),
    )
    .assert_value()
}
fn done(name: &str) -> Value {
    json!({"kind":"succeed","name":name,"output":{"kind":"null"},"bindings":[]})
}
fn fail(name: &str, reason: &str) -> Value {
    json!({"kind":"fail","name":name,"reason":reason})
}
fn seq(name: &str, children: Vec<Value>) -> Value {
    json!({"kind":"seq","name":name,"state":state(),"children":children,"promotedStatePaths":[]})
}
fn graph(children: Vec<Value>) -> GraphSpec {
    serde_json::from_value(json!({"profile":"openengine.graph.full/v1","initialInput":state(),"policy":{"policy":"policy.native-v2@1","default":"deny"},"root":seq("run",children)})).assert_value()
}
fn choose(name: &str, guard: Value, otherwise: Value) -> Value {
    json!({"kind":"choice","name":name,"state":state(),"branches":[{"when":guard,"node":fail(&format!("{name}_failed"),"authored_failure")}],"otherwise":otherwise,"promotedStatePaths":[]})
}
fn fixture() -> GraphSpec {
    let mut review = worker("review");
    review["kind"] = json!("verifier");
    review["signals"] = json!({"verdict":["accepted","rejected"]});
    review["diagnostic"] = json!({"kind":"null"});
    let mut decision = choose(
        "business",
        json!({"kind":"any","guards":[error("left"),error("right"),error("review")]}),
        worker("repair"),
    );
    decision["branches"].as_array_mut().assert_value().push(json!({"when":{"kind":"in","value":{"name":"review","source":"signal","field":"verdict"},"labels":["accepted"]},"node":done("accepted")}));
    let protected = choose(
        "assembly_errors",
        error("assemble"),
        seq("after_assembly", vec![review, decision]),
    );
    graph(vec![
        json!({"kind":"par","name":"drafts","state":state(),"join":{"kind":"all"},"branches":[worker("left"),worker("right")],"promotedStatePaths":[]}),
        worker("assemble"),
        protected,
        done("finished"),
    ])
}
fn node_value(graph: &GraphSpec, name: &str) -> Value {
    serde_json::to_value(
        path_to(&graph.root, &NodeName::new(name).assert_value())
            .assert_value()
            .last()
            .assert_value(),
    )
    .assert_value()
}
fn ensure(graph: &mut GraphSpec) -> Result<(), ApiError> {
    ensure_required_output(
        graph,
        &json!({"nodes":{"on_error":{}}}),
        &NodeName::new("left").assert_value(),
        &NodeName::new("assemble").assert_value(),
    )
}

#[tokio::test]
async fn required_parallel_output_factors_only_complete_first_errors_and_preserves_business_routes()
{
    let mut graph = fixture();
    let before = node_value(&graph, "business");
    ensure(&mut graph).map_err(|e| e.message).assert_value();
    let after = node_value(&graph, "business");
    assert_eq!(after["branches"][0]["when"], error("review"));
    assert_eq!(after["branches"][0]["node"], before["branches"][0]["node"]);
    assert_eq!(after["branches"][1], before["branches"][1]);
    assert_eq!(after["otherwise"], before["otherwise"]);
    let root = serde_json::to_value(&graph.root).assert_value();
    assert_eq!(
        root["children"][1]["branches"][0]["node"]["reason"],
        json!("authored_failure")
    );
    assert_ne!(root["children"][1]["name"], json!("on_error"));
    let repeated = graph.clone();
    ensure(&mut graph).map_err(|e| e.message).assert_value();
    assert_eq!(
        serde_json::to_value(&graph).assert_value(),
        serde_json::to_value(repeated).assert_value()
    );
    let mut bindings = serde_json::Map::new();
    for name in ["left", "right", "assemble", "review", "repair"] {
        bindings.insert(name.into(), json!({"kind":"agent","model":"test-model"}));
    }
    let runtime = serde_json::from_value(
        json!({"harness":"codex","provider":"openai","size":"small","nodes":bindings}),
    )
    .assert_value();
    crate::native_v2_admission::NativeV2Admission
        .validate_profile(
            &graph,
            &runtime,
            crate::native_v2_admission::DeliveryPolicy::Optional,
        )
        .await
        .assert_value();
}

#[test]
fn partial_custom_nonfirst_and_competing_handlers_reject_atomically() {
    for variant in ["partial", "nonterminal", "nonfirst", "competing"] {
        let mut value = serde_json::to_value(fixture()).assert_value();
        let decision = &mut value["root"]["children"][2]["otherwise"]["children"][1];
        match variant {
            "partial" => decision["branches"][0]["when"]["guards"][0]["labels"] = json!(["crash"]),
            "nonterminal" => decision["branches"][0]["node"] = worker("recover"),
            "nonfirst" => decision["branches"].as_array_mut().assert_value().insert(
                0,
                json!({"when":error("assemble"),"node":done("custom_success")}),
            ),
            "competing" => decision["branches"].as_array_mut().assert_value().push(
                json!({"when":error("left"),"node":fail("different_failure","different_reason")}),
            ),
            _ => unreachable!(),
        }
        let mut graph: GraphSpec = serde_json::from_value(value).assert_value();
        let before = serde_json::to_value(&graph).assert_value();
        assert!(ensure(&mut graph).is_err(), "{variant}");
        assert_eq!(
            serde_json::to_value(graph).assert_value(),
            before,
            "{variant}"
        );
    }
}

#[test]
fn required_dependency_does_not_hoist_across_an_earlier_business_decision() {
    let mut value = serde_json::to_value(fixture()).assert_value();
    value["root"]["children"]
        .as_array_mut()
        .assert_value()
        .insert(
            2,
            choose("custom_route", error("assemble"), worker("optional_work")),
        );
    // An extra successful branch changes whether the later terminal handler is reached.
    value["root"]["children"][2]["branches"][0]["node"] = done("custom_success");
    let mut graph: GraphSpec = serde_json::from_value(value).assert_value();
    let before = serde_json::to_value(&graph).assert_value();
    assert!(ensure(&mut graph).is_err());
    assert_eq!(serde_json::to_value(graph).assert_value(), before);
}

#[test]
fn crossed_failure_reasons_and_unrelated_defaults_cannot_change_priority() {
    for variant in [
        "other_reason",
        "other_worker",
        "partial_consumer",
        "sibling",
    ] {
        let mut value = serde_json::to_value(fixture()).assert_value();
        let wrapper = &mut value["root"]["children"][2];
        match variant {
            "other_reason" => {
                wrapper["branches"][0]["node"]["reason"] = json!("consumer_custom_failure")
            }
            "other_worker" => {
                wrapper["branches"][0]["node"]["reason"] = json!("execution_failed");
                wrapper["branches"][0]["when"] = error("unrelated");
            }
            "partial_consumer" => {
                wrapper["branches"][0]["node"]["reason"] = json!("execution_failed");
                wrapper["branches"][0]["when"]["labels"] = json!(["crash"]);
            }
            "sibling" => {
                let continuation = wrapper["otherwise"].clone();
                wrapper["otherwise"] = worker("unrelated");
                wrapper["branches"][0]["node"]["reason"] = json!("earlier_reason");
                value["root"]["children"]
                    .as_array_mut()
                    .assert_value()
                    .insert(3, continuation);
            }
            _ => unreachable!(),
        }
        let mut graph: GraphSpec = serde_json::from_value(value).assert_value();
        let before = serde_json::to_value(&graph).assert_value();
        assert!(ensure(&mut graph).is_err(), "{variant}");
        assert_eq!(
            serde_json::to_value(graph).assert_value(),
            before,
            "{variant}"
        );
    }
}

#[test]
fn new_dependency_can_cross_only_its_consumers_exact_default_guard() {
    let mut value = serde_json::to_value(fixture()).assert_value();
    value["root"]["children"][2]["branches"][0]["node"]["reason"] = json!("execution_failed");
    let original_wrapper = value["root"]["children"][2]["branches"][0].clone();
    let mut graph: GraphSpec = serde_json::from_value(value).assert_value();
    ensure(&mut graph).map_err(|e| e.message).assert_value();
    assert_eq!(
        node_value(&graph, "assembly_errors")["branches"][0],
        original_wrapper
    );
    let root = serde_json::to_value(graph.root).assert_value();
    assert_eq!(
        root["children"][1]["branches"][0]["node"]["reason"],
        json!("authored_failure")
    );
}

#[test]
fn moving_entire_failure_branch_preserves_remaining_scope() {
    let source = worker("left");
    let handler = choose("late_handler", error("left"), worker("repair"));
    let mut graph = graph(vec![source, worker("assemble"), handler, done("done")]);
    ensure(&mut graph).map_err(|e| e.message).assert_value();
    let moved = node_value(&graph, "late_handler");
    assert_eq!(moved["kind"], json!("seq"));
    assert_eq!(moved["state"], state());
    assert_eq!(moved["promotedStatePaths"], json!([]));
    assert_eq!(moved["children"][0]["name"], json!("repair"));
}

#[test]
fn new_required_dependency_receives_default_checkpoint_without_existing_handler() {
    let mut graph = graph(vec![worker("left"), worker("assemble"), done("done")]);
    ensure(&mut graph).map_err(|e| e.message).assert_value();
    let root = serde_json::to_value(&graph.root).assert_value();
    assert_eq!(root["children"][1]["branches"][0]["when"], error("left"));
    assert_eq!(
        root["children"][1]["branches"][0]["node"]["reason"],
        json!("execution_failed")
    );
}

#[test]
fn invalid_required_dependency_selections_are_rejected_atomically() {
    let cases = [
        ("missing", "assemble", "Select uniquely named"),
        ("left", "left", "Select an earlier output"),
        ("assemble", "left", "Select an earlier output"),
        ("left", "right", "Select an earlier output"),
    ];
    for (source, consumer, expected) in cases {
        let mut graph = fixture();
        let before = serde_json::to_value(&graph).assert_value();
        let error = ensure_required_output(
            &mut graph,
            &json!({"nodes":{"on_error":{}}}),
            &NodeName::new(source).assert_value(),
            &NodeName::new(consumer).assert_value(),
        )
        .unwrap_err();

        assert!(error.message.contains(expected), "{source} -> {consumer}");
        assert_eq!(serde_json::to_value(graph).assert_value(), before);
    }
}
