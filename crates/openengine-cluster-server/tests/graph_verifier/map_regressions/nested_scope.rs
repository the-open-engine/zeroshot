use super::*;

fn nested_scope_graph(shared_gate: bool) -> GraphSpec {
    let state = json!({"kind":"record","fields":{"items":{"required":true,"type":{
        "kind":"array","items":{"kind":"record","fields":{"kids":{"required":true,
            "type":{"kind":"array","items":{"kind":"null"}}}}}
    }}}});
    let route = json!({"kind":"choice","name":"route","state":state,"promotedStatePaths":[],
        "branches":[{"when":{"kind":"in","value":{"source":"signal","name":"a","field":"verdict"},
            "labels":["accepted"]},"node":map_verifier("b")}],"otherwise":map_verifier("c")});
    let inner = json!({"kind":"map","name":"inner","state":state,"promotedStatePaths":[],
        "over":{"source":"item","path":["kids"]},"maxItems":2,"body":route});
    let body = if shared_gate {
        inner
    } else {
        json!({"kind":"seq","name":"body","state":state,"promotedStatePaths":[],
            "children":[map_verifier("a"),inner]})
    };
    let outer = json!({"kind":"map","name":"outer","state":state,"promotedStatePaths":[],
        "over":{"source":"state","path":["items"]},"maxItems":2,"body":body});
    // Only inner outcomes are aggregated: explicitly aggregating a masks the scope bug.
    let both = json!({"kind":"all","guards":[
        {"kind":"k_of_map","count":1,"value":{"source":"signal","name":"b","field":"verdict"},"labels":["accepted"]},
        {"kind":"k_of_map","count":1,"value":{"source":"signal","name":"c","field":"verdict"},"labels":["accepted"]}
    ]});
    let mut after = map_choice(state.clone(), both, "selected", "otherwise");
    after["otherwise"] = json!({"kind":"fail","name":"otherwise","reason":"no_matching_signals"});
    let children = if shared_gate {
        json!([map_verifier("a"), outer, after])
    } else {
        json!([outer, after])
    };
    graph_with_state_children(state, children)
}

#[tokio::test]
async fn nested_map_implicit_outer_dependency_keeps_the_positive_branch_reachable() {
    assert_graph_accepted(&nested_scope_graph(false)).await;
}

#[tokio::test]
async fn nested_map_implicit_outer_dependency_rejects_a_nonexhaustive_choice() {
    let mut graph = serde_json::to_value(nested_scope_graph(false)).assert_value();
    let after = &mut graph["root"]["children"][1];
    let both = after["branches"][0]["when"].take();
    after["branches"][0]["when"] = json!({"kind":"not","guard":both});
    after.as_object_mut().assert_value().remove("otherwise");
    let graph = serde_json::from_value(graph).assert_value();
    let diagnostics = rejection_diagnostics(assert_graph_rejected(&graph).await);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == GraphDiagnosticCode::ChoiceExhaustiveness
                && diagnostic.message
                    == "choice branches do not cover every legal control assignment"
                && diagnostic.path.contains(
                    &openengine_cluster_protocol::DiagnosticPathSegment::Node {
                        name: "afterMap".parse().assert_value(),
                    },
                )
        }),
        "{diagnostics:?}"
    );
}

#[tokio::test]
async fn nested_map_dependency_before_the_outer_map_stays_shared() {
    // One shared a cannot select both b and c, even across different outer items.
    let diagnostics = rejection_diagnostics(assert_graph_rejected(&nested_scope_graph(true)).await);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == GraphDiagnosticCode::ChoiceExhaustiveness
                && diagnostic.message
                    == "choice branch is unreachable after excluding earlier branches"
                && diagnostic
                    .related_nodes
                    .iter()
                    .any(|name| name.as_str() == "afterMap")
        }),
        "{diagnostics:?}"
    );
}
