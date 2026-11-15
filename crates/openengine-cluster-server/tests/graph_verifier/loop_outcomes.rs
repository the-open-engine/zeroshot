use super::*;

#[tokio::test]
async fn first_join_admission_keeps_loop_termination_correlated_with_its_final_exit() {
    let state = json!({"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}});
    for mapped in [false, true] {
        for compatible in [false, true] {
            let accepted = json!({"kind":"in","value":{"name":"gate","source":"signal","field":"verdict"},"labels":["accepted"]});
            let mut children = vec![test_support::verifier_node("gate")];
            if mapped {
                children.push(json!({"kind":"map","name":"items_map","state":state,"body":test_support::verifier_node("a"),"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]}));
            }
            let body = json!({"kind":"seq","name":"body","state":state,"children":children,"promotedStatePaths":[]});
            let repeated = json!({"kind":"loop","name":"twice","state":state,"body":body,"maxIterations":2,"until":accepted,"promotedStatePaths":[]});
            let mut guards = vec![
                accepted,
                json!({"kind":"in","value":{"name":"twice","source":"group","field":"terminated"},"labels":[if compatible { "converged" } else { "exhausted" }]}),
            ];
            if mapped {
                guards.push(json!({"kind":"k_of_map","count":1,"value":{"name":"a","source":"signal","field":"verdict"},"labels":["accepted"]}));
            }
            let definition = graph_with_state_children(
                state.clone(),
                json!([
                    {"kind":"par","name":"race","state":state,"branches":[repeated,test_support::verifier_node("b")],"join":{"kind":"first","when":{"kind":"all","guards":guards}},"promotedStatePaths":[]},
                    {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
                ]),
            );
            if compatible {
                assert_graph_accepted(&definition).await;
            } else {
                assert_graph_rejected_with(&definition, GraphDiagnosticCode::InvalidGraphShape)
                    .await;
            }
        }
    }
}
