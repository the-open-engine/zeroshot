#[tokio::test]
async fn every_guard_form_and_map_aggregate_is_admitted_when_satisfiable() {
    for guard in [
        json!({"kind":"any","guards":[
            {"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]},
            {"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["rejected"]}
        ]}),
        json!({"kind":"not","guard":
            {"kind":"in","value":{"name":"verify","source":"error","field":null},"labels":["timeout"]}
        }),
        json!({"kind":"k_of_n","count":1,"values":[
            {"name":"verify","source":"signal","field":"verdict"},
            {"name":"verify","source":"error","field":null}
        ],"labels":["accepted","timeout"]}),
    ] {
        let mut value = valid_graph();
        value["root"]["children"][2]["branches"][0]["when"] = guard;
        let graph: GraphSpec = serde_json::from_value(value).unwrap();
        ProductionGraphVerifier::new(registry())
            .verify(&graph)
            .await
            .unwrap();
    }

    let map_graph = map_control_graph(
        2,
        json!([
            {
                "when":{
                    "kind":"k_of_map","count":2,
                    "value":{"name":"mapVerify","source":"signal","field":"verdict"},
                    "labels":["accepted"]
                },
                "node":{
                    "kind":"succeed","name":"selectedTwice",
                    "output":{"kind":"null"},"bindings":[]
                }
            },
            {
                "when":{
                    "kind":"k_of_map","count":1,
                    "value":{"name":"mapVerify","source":"signal","field":"verdict"},
                    "labels":["accepted"]
                },
                "node":{
                    "kind":"succeed","name":"selectedOnce",
                    "output":{"kind":"null"},"bindings":[]
                }
            }
        ]),
        json!({"kind":"fail","name":"failed","reason":"failed"}),
    );
    ProductionGraphVerifier::new(registry())
        .verify(&map_graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn k_of_map_rejects_selector_without_bounded_map_scope() {
    let mut value = valid_graph();
    value["root"]["children"][2]["branches"][0]["when"] = json!({
        "kind":"k_of_map","count":1,
        "value":{"name":"verify","source":"signal","field":"verdict"},
        "labels":["accepted"]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("k_of_map without bounded map scope must be rejected")
    };

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == GraphDiagnosticCode::ChoiceExhaustiveness
            && diagnostic.message == "k_of_map selector has no bounded enclosing map scope"
    }));
}

#[tokio::test]
async fn k_of_map_rejects_selected_map_group_control_without_enclosing_map_scope() {
    let graph = map_control_graph(
        2,
        json!([{
            "when":{
                "kind":"k_of_map","count":1,
                "value":{"name":"map","source":"group","field":"overflow"},
                "labels":["overflow"]
            },
            "node":{
                "kind":"succeed","name":"selected",
                "output":{"kind":"null"},"bindings":[]
            }
        }]),
        json!({"kind":"fail","name":"failed","reason":"failed"}),
    );

    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("a map group control is not aggregated over its own items")
    };

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == GraphDiagnosticCode::ChoiceExhaustiveness
            && diagnostic.message == "k_of_map selector has no bounded enclosing map scope"
    }));
}

#[tokio::test]
async fn exhaustive_terminal_choice_without_otherwise_is_admitted() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted","rejected"]},
                "node":{"kind":"succeed","name":"completed","output":{"kind":"null"},"bindings":[]}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"failed","reason":"worker_error"}
            }
        ],
        "otherwise":null,
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn exhaustive_terminal_choice_rejects_dead_otherwise() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted"]},
                "node":{"kind":"succeed","name":"accepted","output":{"kind":"null"},"bindings":[]}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["rejected"]},
                "node":{"kind":"fail","name":"rejected","reason":"rejected"}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workerFailed","reason":"worker_error"}
            }
        ],
        "otherwise":{
            "kind":"succeed","name":"deadOtherwise","output":record(),
            "bindings":[{"target":["value"],"value":{"source":"state","path":["missing"]}}]
        },
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("dead otherwise must be rejected")
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "dead otherwise must not enter flow analysis"
    );
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == GraphDiagnosticCode::ChoiceExhaustiveness
            && serde_json::to_value(&diagnostic.path).unwrap() == dead_otherwise_diagnostic_path()
    }));
}

#[tokio::test]
async fn dead_nonterminal_otherwise_does_not_cause_terminal_fallthrough() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted","rejected"]},
                "node":{"kind":"succeed","name":"completed","output":{"kind":"null"},"bindings":[]}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workerFailed","reason":"worker_error"}
            }
        ],
        "otherwise":{
            "kind":"step", "name":"deadOtherwise", "worker":"worker.main@1",
            "input":{"kind":"null"}, "output":{"kind":"null"},
            "inputBindings":[], "writeBindings":[], "timeoutMs":1, "attempts":1
        },
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("dead otherwise must be rejected")
    };
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        GraphDiagnosticCode::ChoiceExhaustiveness
    );
    assert_eq!(
        serde_json::to_value(&diagnostics[0].path).unwrap(),
        dead_otherwise_diagnostic_path()
    );
}

#[tokio::test]
async fn dead_nonterminal_guarded_branch_does_not_cause_terminal_fallthrough() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted","rejected"]},
                "node":{"kind":"succeed","name":"completed","output":{"kind":"null"},"bindings":[]}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted"]},
                "node":{
                    "kind":"step", "name":"deadBranch", "worker":"worker.main@1",
                    "input":record(), "output":{"kind":"null"},
                    "inputBindings":[{"target":["value"],"value":{"source":"state","path":["missing"]}}],
                    "writeBindings":[], "timeoutMs":1, "attempts":1
                }
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workerFailed","reason":"worker_error"}
            }
        ],
        "otherwise":null,
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("dead guarded branch must be rejected")
    };
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        GraphDiagnosticCode::ChoiceExhaustiveness
    );
    assert_eq!(
        diagnostics[0].message,
        "choice branch is unreachable after excluding earlier branches"
    );
}

fn dead_otherwise_diagnostic_path() -> Value {
    json!([
        {"kind":"field","name":"root"},
        {"kind":"node","name":"root"},
        {"kind":"field","name":"children"},
        {"kind":"index","index":2},
        {"kind":"node","name":"decision"},
        {"kind":"field","name":"otherwise"},
        {"kind":"node","name":"deadOtherwise"}
    ])
}

#[tokio::test]
async fn choice_residual_outcomes_protect_unavailable_verifier_outputs() {
    let mut value = valid_graph();
    value["root"]["children"][1]["output"] =
        json!({"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}});
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[{
            "when":{"kind":"not","guard":{"kind":"in",
                "value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]}},
            "node":{
                "kind":"step", "name":"recover", "worker":"worker.main@1",
                "input":{"kind":"null"}, "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
                "inputBindings":[],
                "writeBindings":[{"value":{"node":"verify","channel":"out","path":["result"]},"target":["result"]}],
                "timeoutMs":1, "attempts":1
            }
        }],
        "otherwise":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]},
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}
#[tokio::test]
async fn terminal_error_paths_do_not_poison_success_only_continuations() {
    let mut value = valid_graph();
    value["root"]["children"][1]["output"] =
        json!({"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}});
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"routeError", "state":record(),
        "branches":[{
            "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                "labels":["timeout","crash","malformed","refusal"]},
            "node":{"kind":"fail","name":"workerFailed","reason":"worker_error"}
        }],
        "otherwise":{
            "kind":"step", "name":"consume", "worker":"worker.main@1",
            "input":record(), "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
            "inputBindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}],
            "writeBindings":[{"value":{"node":"verify","channel":"out","path":["result"]},"target":["result"]}],
            "timeoutMs":1, "attempts":1
        },
        "promotedStatePaths":[]
    });
    value["root"]["children"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]
        }));
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn output_backed_writes_are_undefined_until_success_is_guaranteed() {
    let mut value = valid_graph();
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        {
            "kind":"succeed", "name":"done",
            "output":{"kind":"record","fields":{
                "result":{"type":{"kind":"number"},"required":true}
            }},
            "bindings":[{
                "target":["result"],
                "value":{"source":"state","path":["result"]}
            }]
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();

    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}
use super::*;
