use super::*;
use test_support::{integer_step, verifier_node};

fn error_guard(name: &str) -> Value {
    json!({"kind":"in","value":{"name":name,"source":"error","field":null},"labels":["crash","malformed","refusal","timeout"]})
}

fn loop_graph(body: Value, until: Value) -> GraphSpec {
    graph_with_state_children(
        record(),
        json!([
            {"kind":"loop","name":"repeat","state":record(),"body":body,"until":until,"maxIterations":3,"promotedStatePaths":[]},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ]),
    )
}

#[tokio::test]
async fn guaranteed_step_errors_can_stop_direct_sequential_and_all_parallel_rounds() {
    for body in [
        integer_step("work", false),
        json!({"kind":"seq","name":"round","state":record(),"children":[integer_step("work",false),verifier_node("review")],"promotedStatePaths":[]}),
        json!({"kind":"par","name":"round","state":record(),"branches":[integer_step("work",false),verifier_node("review")],"join":{"kind":"all"},"promotedStatePaths":[]}),
    ] {
        assert_graph_accepted(&loop_graph(body, error_guard("work"))).await;
    }
}

#[tokio::test]
async fn verifier_verdicts_and_errors_remain_valid_loop_exits() {
    let until = json!({"kind":"any","guards":[
        {"kind":"in","value":{"name":"review","source":"signal","field":"verdict"},"labels":["accepted"]},
        error_guard("review")
    ]});
    assert_graph_accepted(&loop_graph(verifier_node("review"), until)).await;
}

#[tokio::test]
async fn conditional_step_errors_are_not_guaranteed_on_every_completing_round() {
    let body = json!({"kind":"seq","name":"round","state":record(),"children":[
        verifier_node("review"),
        {"kind":"choice","name":"route","state":record(),"branches":[{
            "when":{"kind":"in","value":{"name":"review","source":"signal","field":"verdict"},"labels":["accepted"]},
            "node":integer_step("conditional_work",false)
        }],"otherwise":verifier_node("other_review"),"promotedStatePaths":[]}
    ],"promotedStatePaths":[]});
    assert_graph_rejected_with(
        &loop_graph(body, error_guard("conditional_work")),
        GraphDiagnosticCode::LoopExitSatisfiability,
    )
    .await;
}

#[tokio::test]
async fn step_signal_and_group_selectors_remain_invalid_loop_exits() {
    let step_signal = json!({"kind":"in","value":{"name":"work","source":"signal","field":"verdict"},"labels":["accepted"]});
    assert_graph_rejected_with(
        &loop_graph(integer_step("work", false), step_signal),
        GraphDiagnosticCode::LoopExitSatisfiability,
    )
    .await;
    let nested = json!({"kind":"loop","name":"inner","state":record(),"body":integer_step("work",false),"maxIterations":2,"promotedStatePaths":[]});
    let group = json!({"kind":"in","value":{"name":"inner","source":"group","field":"terminated"},"labels":["exhausted"]});
    assert_graph_rejected_with(
        &loop_graph(nested, group),
        GraphDiagnosticCode::LoopExitSatisfiability,
    )
    .await;
}
