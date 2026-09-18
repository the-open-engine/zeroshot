use super::*;
use test_support::{integer_step, verifier_node};

fn errors(name: &str) -> Value {
    json!({"kind":"in","value":{"name":name,"source":"error","field":null},"labels":["timeout","crash","malformed","refusal"]})
}
fn route() -> Value {
    json!({"kind":"in","value":{"name":"router","source":"signal","field":"verdict"},"labels":["accepted"]})
}
fn masked_errors() -> Value {
    json!({"kind":"any","guards":[
        {"kind":"all","guards":[route(),errors("left")]},
        {"kind":"all","guards":[{"kind":"not","guard":route()},errors("right")]}
    ]})
}
fn source() -> Value {
    let mut value = valid_graph();
    value["root"]["children"] = json!([
        verifier_node("router"),
        {"kind":"choice","name":"choose","state":record(),"branches":[{"when":route(),"node":integer_step("left",true)}],"otherwise":integer_step("right",true),"promotedStatePaths":[["result"]]},
        {"kind":"choice","name":"check","state":record(),"branches":[{"when":masked_errors(),"node":{"kind":"fail","name":"failed","reason":"branch_failed"}}],"otherwise":{
            "kind":"succeed","name":"done","output":{"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":true}}},"bindings":[{"target":["result"],"value":{"source":"state","path":["result"]}}]
        },"promotedStatePaths":[]}
    ]);
    value
}

fn pending_writes_before_failure_only_choice() -> Value {
    let mut value = valid_graph();
    let prefixes = (0..6).map(|i| format!("prepare_{i}")).collect::<Vec<_>>();
    let mut children = prefixes
        .iter()
        .map(|name| integer_step(name, false))
        .collect::<Vec<_>>();
    children.push(json!({"kind":"choice","name":"prepare_result","state":record(),
        "branches":[{"when":{"kind":"any","guards":prefixes.iter().map(|name|errors(name)).collect::<Vec<_>>()},
            "node":{"kind":"fail","name":"prepare_failed","reason":"prepare_failed"}}],
        "otherwise":integer_step("final_writer",true),"promotedStatePaths":[["result"]]}));
    children.push(json!({"kind":"choice","name":"finish","state":record(),
        "branches":[{"when":errors("final_writer"),"node":{"kind":"fail","name":"writer_failed","reason":"writer_failed"}}],
        "otherwise":{"kind":"fail","name":"needs_review","reason":"needs_review"},"promotedStatePaths":[]}));
    value["root"]["children"] = json!(children);
    value
}

#[tokio::test]
async fn failure_only_choice_does_not_expand_unread_pending_outputs() {
    // Six upstream error dimensions are bounded (5^6). Correlating their pending result with
    // final_writer would add a seventh (5^7), though this decision can only fail and reads no data.
    let value = pending_writes_before_failure_only_choice();
    assert_graph_accepted(&serde_json::from_value(value).assert_value()).await;
}

#[tokio::test]
async fn failure_only_choice_keeps_actual_guard_validation_and_assignment_ceiling() {
    let mut nonexhaustive = pending_writes_before_failure_only_choice();
    nonexhaustive["root"]["children"][7]
        .as_object_mut()
        .assert_value()
        .remove("otherwise");
    assert_graph_rejected_with(
        &serde_json::from_value(nonexhaustive).assert_value(),
        GraphDiagnosticCode::ChoiceExhaustiveness,
    )
    .await;
    let mut unavailable = pending_writes_before_failure_only_choice();
    unavailable["root"]["children"][7]["branches"][0]["when"] = errors("missing");
    assert_graph_rejected_with(
        &serde_json::from_value(unavailable).assert_value(),
        GraphDiagnosticCode::UndefinedRead,
    )
    .await;
    let mut illegal = pending_writes_before_failure_only_choice();
    illegal["root"]["children"][7]["branches"][0]["when"]["labels"] = json!(["invented"]);
    assert_graph_rejected_with(
        &serde_json::from_value(illegal).assert_value(),
        GraphDiagnosticCode::ChoiceExhaustiveness,
    )
    .await;
    let mut over_budget = pending_writes_before_failure_only_choice();
    let mut guards = (0..6)
        .map(|i| errors(&format!("prepare_{i}")))
        .collect::<Vec<_>>();
    guards.push(errors("final_writer"));
    over_budget["root"]["children"][7]["branches"][0]["when"] =
        json!({"kind":"any","guards":guards});
    assert_graph_rejected_with(
        &serde_json::from_value(over_budget).assert_value(),
        GraphDiagnosticCode::CeilingExceeded,
    )
    .await;
}

#[tokio::test]
async fn selected_branch_success_proves_its_common_output_after_choice() {
    assert_graph_accepted(&serde_json::from_value(source()).assert_value()).await;
}

#[tokio::test]
async fn common_choice_output_stays_undefined_without_every_selected_success() {
    let mut unguarded = source();
    unguarded["root"]["children"][2] = unguarded["root"]["children"][2]["otherwise"].clone();
    assert_graph_rejected_with(
        &serde_json::from_value(unguarded).assert_value(),
        GraphDiagnosticCode::UndefinedRead,
    )
    .await;
    let mut partial = source();
    partial["root"]["children"][2]["branches"][0]["when"] =
        json!({"kind":"all","guards":[route(),errors("left")]});
    assert_graph_rejected_with(
        &serde_json::from_value(partial).assert_value(),
        GraphDiagnosticCode::UndefinedRead,
    )
    .await;
    let mut missing = source();
    missing["root"]["children"][1]["otherwise"]["writeBindings"] = json!([]);
    assert_graph_rejected(&serde_json::from_value(missing).assert_value()).await;
}

#[tokio::test]
async fn conditional_errors_require_correct_route_before_the_control_read() {
    for guard in [
        json!({"kind":"any","guards":[errors("left"),errors("right")]}),
        json!({"kind":"all","guards":[errors("left"),route()]}),
        json!({"kind":"all","guards":[{"kind":"not","guard":route()},errors("left")]}),
    ] {
        let mut value = source();
        value["root"]["children"][2]["branches"][0]["when"] = guard;
        assert_graph_rejected_with(
            &serde_json::from_value(value).assert_value(),
            GraphDiagnosticCode::UndefinedRead,
        )
        .await;
    }
}

#[tokio::test]
async fn later_overlapping_writes_cannot_restore_an_old_common_choice_type() {
    let mut value = source();
    let mut overwrite = integer_step("overwrite", true);
    overwrite["output"]["fields"]["result"]["type"] = json!({"kind":"number"});
    value["root"]["children"]
        .as_array_mut()
        .assert_value()
        .insert(2, overwrite);
    assert_graph_rejected_with(
        &serde_json::from_value(value).assert_value(),
        GraphDiagnosticCode::UndefinedRead,
    )
    .await;
}

#[tokio::test]
async fn selected_success_does_not_define_a_private_output_of_an_unselected_branch() {
    let mut value = source();
    let field = json!({"type":{"kind":"integer"},"required":false});
    value["root"]["state"]["fields"]["private"] = field.clone();
    value["root"]["children"][1]["state"]["fields"]["private"] = field.clone();
    value["root"]["children"][2]["state"]["fields"]["private"] = field;
    value["root"]["children"][1]["branches"][0]["node"]["writeBindings"]
        .as_array_mut()
        .assert_value()
        .push(
            json!({"target":["private"],"value":{"node":"left","channel":"out","path":["result"]}}),
        );
    value["root"]["children"][1]["promotedStatePaths"]
        .as_array_mut()
        .assert_value()
        .push(json!(["private"]));
    value["root"]["children"][2]["otherwise"]["bindings"][0]["value"]["path"] = json!(["private"]);
    assert_graph_rejected_with(
        &serde_json::from_value(value).assert_value(),
        GraphDiagnosticCode::UndefinedRead,
    )
    .await;
}

#[tokio::test]
async fn a_route_mask_cannot_make_a_future_choice_worker_available() {
    let mut value = source();
    value["root"]["children"]
        .as_array_mut()
        .assert_value()
        .swap(1, 2);
    assert_graph_rejected_with(
        &serde_json::from_value(value).assert_value(),
        GraphDiagnosticCode::UndefinedRead,
    )
    .await;
}

#[tokio::test]
async fn earlier_pending_choice_write_cannot_override_a_later_proven_write_type() {
    let mut value = source();
    let mut overwrite = integer_step("overwrite", true);
    overwrite["worker"] = json!("worker.number@1");
    overwrite["output"]["fields"]["result"]["type"] = json!({"kind":"number"});
    let finished = json!({
        "kind":"choice", "name":"overwrite_check", "state":record(),
        "branches":[{"when":errors("overwrite"),"node":{"kind":"fail","name":"overwrite_failed","reason":"overwrite_failed"}}],
        "otherwise":integer_step("end_left",false), "promotedStatePaths":[]
    });
    value["root"]["children"][1]["branches"][0]["node"] = json!({
        "kind":"seq", "name":"left_branch", "state":record(),
        "children":[integer_step("left",true),overwrite,finished],
        "promotedStatePaths":[["result"]]
    });
    value["root"]["children"][2]["branches"][0]["when"] = json!({
        "kind":"any", "guards":[
            {"kind":"all","guards":[route(),{"kind":"any","guards":[errors("left"),errors("overwrite")]}]},
            {"kind":"all","guards":[{"kind":"not","guard":route()},errors("right")]}
        ]
    });
    let mut descriptors = (*registry().descriptors).clone();
    let mut number_descriptor =
        serde_json::to_value(descriptor("worker.number@1", false)).assert_value();
    number_descriptor["contract"]["output"]["fields"]["result"]["type"] = json!({"kind":"number"});
    descriptors.insert(
        WorkerRef::new("worker.number@1").assert_value(),
        serde_json::from_value(number_descriptor).assert_value(),
    );
    let registry = MemoryRegistry {
        descriptors: Arc::new(descriptors),
        resolutions: Arc::new(AtomicUsize::new(0)),
    };
    let result = ProductionGraphVerifier::new(registry.clone())
        .verify(&serde_json::from_value(value.clone()).assert_value())
        .await;
    assert!(
        result.is_err(),
        "An earlier integer write must not hide the later proven number write"
    );
    value["root"]["children"][2]["otherwise"]["output"]["fields"]["result"]["type"] =
        json!({"kind":"number"});
    ProductionGraphVerifier::new(registry)
        .verify(&serde_json::from_value(value).assert_value())
        .await
        .assert_value();
}
