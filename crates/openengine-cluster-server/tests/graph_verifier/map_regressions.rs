use super::*;

#[path = "map_regressions/fixtures.rs"]
mod fixtures;
use fixtures::{
    map_choice, map_node, map_verifier, mapped_control_graph, mapped_items_state, mapped_step,
};

fn mapped_error_read_graph() -> GraphSpec {
    let item = json!({
        "kind":"record",
        "fields":{"value":{"type":{"kind":"integer"},"required":true}}
    });
    let state = json!({
        "kind":"record",
        "fields":{
            "items":{"type":{"kind":"array","items":item},"required":true},
            "results":{
                "type":{"kind":"array","items":{"kind":"integer"}},"required":false
            }
        }
    });
    graph_with_state_children(
        state.clone(),
        json!([
                {
                    "kind":"map","name":"map","state":state,
                    "body":{
                        "kind":"step","name":"mapWork","worker":"worker.main@1",
                        "input":record(),
                        "output":{"kind":"record","fields":{
                            "result":{"type":{"kind":"integer"},"required":true}
                        }},
                        "inputBindings":[{
                            "target":["value"],
                            "value":{"source":"item","path":["value"]}
                        }],
                        "writeBindings":[{
                            "value":{"node":"mapWork","channel":"out","path":["result"]},
                            "target":["results"]
                        }],
                        "timeoutMs":1,"attempts":1
                    },
                    "over":{"source":"state","path":["items"]},
                    "maxItems":2,
                    "promotedStatePaths":[["results"]]
                },
                {
                    "kind":"choice","name":"afterMap","state":state,
                    "branches":[{
                        "when":{
                            "kind":"k_of_map","count":1,
                            "value":{"name":"mapWork","source":"error","field":null},
                            "labels":["timeout"]
                        },
                        "node":{
                            "kind":"succeed","name":"badRead",
                            "output":{"kind":"record","fields":{
                                "results":{
                                    "type":{"kind":"array","items":{"kind":"integer"}},
                                    "required":true
                                }
                            }},
                            "bindings":[{
                                "target":["results"],
                                "value":{"source":"state","path":["results"]}
                            }]
                        }
                    }],
                    "otherwise":{
                        "kind":"succeed","name":"done",
                        "output":{"kind":"null"},"bindings":[]
                    },
                    "promotedStatePaths":[]
                }
        ]),
    )
}

fn nested_map_state() -> Value {
    let item = json!({
        "kind":"record",
        "fields":{"value":{"type":{"kind":"integer"},"required":true}}
    });
    json!({
        "kind":"record",
        "fields":{
            "outerItems":{"type":{"kind":"array","items":item.clone()},"required":true},
            "innerItems":{"type":{"kind":"array","items":item},"required":true},
            "outerResults":{
                "type":{"kind":"array","items":{"kind":"integer"}},"required":true
            },
            "innerResults":{
                "type":{"kind":"array","items":{"kind":"integer"}},"required":false
            }
        }
    })
}

fn nested_map_body(state: &Value) -> Value {
    let inner_work = json!({
        "kind":"step","name":"innerWork","worker":"worker.main@1",
        "input":{"kind":"record","fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "outer":{"type":{"kind":"integer"},"required":true}
        }},
        "output":{"kind":"record","fields":{
            "result":{"type":{"kind":"integer"},"required":true}
        }},
        "inputBindings":[
            {"target":["value"],"value":{"source":"item","path":["value"]}},
            {"target":["outer"],"value":{"source":"state","path":["outerResults"]}}
        ],
        "writeBindings":[{
            "value":{"node":"innerWork","channel":"out","path":["result"]},
            "target":["innerResults"]
        }],
        "timeoutMs":1,"attempts":1
    });
    let inner_map = json!({
        "kind":"map","name":"innerMap","state":state.clone(),
        "over":{"source":"state","path":["innerItems"]},"maxItems":2,
        "body":inner_work,"promotedStatePaths":[["innerResults"]]
    });
    let outer_work = json!({
        "kind":"step","name":"outerWork","worker":"worker.main@1",
        "input":record(),
        "output":{"kind":"record","fields":{
            "result":{"type":{"kind":"integer"},"required":true}
        }},
        "inputBindings":[{
            "target":["value"],"value":{"source":"item","path":["value"]}
        }],
        "writeBindings":[{
            "value":{"node":"outerWork","channel":"out","path":["result"]},
            "target":["outerResults"]
        }],
        "timeoutMs":1,"attempts":1
    });
    json!({
        "kind":"seq","name":"outerBody","state":state.clone(),
        "children":[inner_map,outer_work],
        "promotedStatePaths":[["outerResults"]]
    })
}

fn nested_map_graph() -> GraphSpec {
    let state = nested_map_state();
    let outer_map = json!({
        "kind":"map","name":"outerMap","state":state.clone(),
        "over":{"source":"state","path":["outerItems"]},"maxItems":2,
        "body":nested_map_body(&state),"promotedStatePaths":[["outerResults"]]
    });
    serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":state,"policy":{"policy":"policy.strict@1","default":"deny"},
        "root":{
            "kind":"seq","name":"root","state":state,
            "children":[
                outer_map,
                {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
            ],
            "promotedStatePaths":[]
        }
    }))
    .assert_value()
}

fn mapped_loop_group_aggregate_graph() -> GraphSpec {
    let state = json!({
        "kind":"record",
        "fields":{
            "outerItems":{
                "type":{"kind":"array","items":{"kind":"null"}},
                "required":true
            },
            "innerItems":{
                "type":{"kind":"array","items":{"kind":"null"}},
                "required":true
            }
        }
    });
    graph_with_state_children(
        state.clone(),
        json!([
            {
                "kind":"map","name":"outerMap","state":state.clone(),
                "over":{"source":"state","path":["outerItems"]},"maxItems":2,
                "body":{
                    "kind":"loop","name":"innerLoop","state":state.clone(),
                    "maxIterations":1,
                    "body":{
                        "kind":"verifier","name":"innerVerify","worker":"worker.verify@1",
                        "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
                        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
                        "signals":{"verdict":["accepted","rejected"]},
                        "diagnostic":{"kind":"record","fields":{}}
                    },
                    "promotedStatePaths":[]
                },
                "promotedStatePaths":[]
            },
            {
                "kind":"choice","name":"afterOuterMap","state":state,
                "branches":[{
                    "when":{
                        "kind":"k_of_map","count":2,
                        "value":{"name":"innerLoop","source":"group","field":"terminated"},
                        "labels":["exhausted"]
                    },
                    "node":{
                        "kind":"succeed","name":"twoInnerExhaustions",
                        "output":{"kind":"null"},"bindings":[]
                    }
                }],
                "otherwise":{
                    "kind":"succeed","name":"fewerInnerExhaustions",
                    "output":{"kind":"null"},"bindings":[]
                },
                "promotedStatePaths":[]
            }
        ]),
    )
}

fn mapped_parallel_control_correlation_graph(reached_count: u64, rejected_count: u64) -> GraphSpec {
    let state = mapped_items_state();
    let item_verify = map_verifier("itemVerify");
    let accepted_work = mapped_step("acceptedWork");
    let controlled_branch = json!({
        "kind":"choice","name":"itemRoute","state":state.clone(),
        "branches":[{
            "when":{
                "kind":"in",
                "value":{"name":"itemVerify","source":"signal","field":"verdict"},
                "labels":["accepted"]
            },
            "node":accepted_work
        }],
        "otherwise":{
            "kind":"fail","name":"itemRejected","reason":"item_rejected"
        },
        "promotedStatePaths":[]
    });
    let inner_parallel = json!({
        "kind":"par","name":"innerPar","state":state.clone(),
        "branches":[
            controlled_branch,
            {"kind":"fail","name":"neverCompletes","reason":"never_completes"}
        ],
        "join":{"kind":"any"},
        "promotedStatePaths":[]
    });
    mapped_control_graph(
        state,
        json!([item_verify, inner_parallel]),
        json!({
            "kind":"all",
            "guards":[
                {
                    "kind":"k_of_map","count":reached_count,
                    "value":{"name":"innerPar","source":"group","field":"joined"},
                    "labels":["reached"]
                },
                {
                    "kind":"k_of_map","count":rejected_count,
                    "value":{"name":"itemVerify","source":"signal","field":"verdict"},
                    "labels":["rejected"]
                }
            ]
        }),
        ("impossible", "possible"),
    )
}

fn controlled_step(
    state: &Value,
    name: &str,
    selector: (&str, &str),
    outcomes: (&str, &str),
) -> Value {
    let (selector_name, signal_field) = selector;
    let (accepted_name, rejected_name) = outcomes;
    json!({
            "kind":"choice","name":name,"state":state,
            "branches":[{
                "when":{
                    "kind":"in",
                    "value":{
                        "name":selector_name,
                        "source":"signal",
                        "field":signal_field
                    },
                    "labels":["accepted"]
                },
                "node":mapped_step(accepted_name)
            }],
            "otherwise":{
                "kind":"fail","name":rejected_name,"reason":"rejected"
            },
            "promotedStatePaths":[]
    })
}

fn mapped_parallel_multicontrol_correlation_graph() -> GraphSpec {
    let state = mapped_items_state();
    let verdict = map_verifier("itemVerdict");
    let decision = json!({
        "kind":"verifier","name":"itemDecision","worker":"worker.decision@1",
        "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
        "signals":{"decision":["accepted","rejected"]},
        "diagnostic":{"kind":"record","fields":{}}
    });
    let completing_branch = json!({
        "kind":"seq","name":"requiresBoth","state":state.clone(),
        "children":[
            controlled_step(
                &state,
                "verdictRoute",
                ("itemVerdict", "verdict"),
                ("acceptedVerdict", "rejectedVerdict")
            ),
            controlled_step(
                &state,
                "decisionRoute",
                ("itemDecision", "decision"),
                ("acceptedDecision", "rejectedDecision")
            )
        ],
        "promotedStatePaths":[]
    });
    let inner_parallel = json!({
        "kind":"par","name":"innerPar","state":state.clone(),
        "branches":[
            completing_branch,
            {"kind":"fail","name":"neverCompletes","reason":"never_completes"}
        ],
        "join":{"kind":"any"},
        "promotedStatePaths":[]
    });
    mapped_control_graph(
        state,
        json!([verdict, decision, inner_parallel]),
        json!({
            "kind":"all",
            "guards":[
                {
                    "kind":"k_of_map","count":2,
                    "value":{
                        "name":"innerPar",
                        "source":"group",
                        "field":"joined"
                    },
                    "labels":["reached"]
                },
                {
                    "kind":"k_of_map","count":1,
                    "value":{
                        "name":"itemVerdict",
                        "source":"signal",
                        "field":"verdict"
                    },
                    "labels":["rejected"]
                }
            ]
        }),
        ("impossible", "possible"),
    )
}

#[path = "map_regressions/core_tests.rs"]
mod core_tests;
#[path = "map_regressions/incoming_control.rs"]
mod incoming_control;
#[path = "map_regressions/outcome_presence.rs"]
mod outcome_presence;

#[tokio::test]
async fn item_local_aggregate_counts_cannot_combine_a_signal_with_its_own_error() {
    let state = mapped_items_state();
    let guard = json!({"kind":"all","guards":[
        {"kind":"k_of_map","count":1,"value":{"name":"itemVerify","source":"signal","field":"verdict"},"labels":["accepted"]},
        {"kind":"in","value":{"name":"itemVerify","source":"error","field":null},"labels":["crash"]}
    ]});
    let body = json!({"kind":"seq","name":"body","state":state,"promotedStatePaths":[],"children":[
        map_verifier("itemVerify"),{"kind":"choice","name":"route","state":state,"promotedStatePaths":[],
            "branches":[{"when":guard,"node":mapped_step("selected")}],"otherwise":mapped_step("other")}
    ]});
    let graph = graph_with_state_children(
        state.clone(),
        json!([map_node(&state,body),{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]),
    );
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .assert_error();
    let codes = rejection_codes(error);
    assert!(
        codes.contains(&GraphDiagnosticCode::ChoiceExhaustiveness),
        "{codes:?}"
    );
}

#[tokio::test]
async fn aggregate_counts_discard_unobservable_label_classes_after_correlation() {
    let state = mapped_items_state();
    let graph = mapped_control_graph(
        state,
        json!([map_verifier("itemVerify")]),
        json!({"kind":"k_of_map","count":100,"value":{"name":"itemVerify","source":"signal","field":"verdict"},"labels":["accepted"]}),
        ("many", "few"),
    );
    assert_graph_accepted(&graph).await;
}

#[tokio::test]
async fn nested_maps_preserve_aggregate_scope_and_conditional_inner_execution() {
    for gate in [None, Some(true), Some(false)] {
        let state = mapped_items_state();
        let inner = json!({"kind":"map","name":"inner","state":state,"over":{"source":"state","path":["items"]},"maxItems":1,"body":map_verifier("innerVerify"),"promotedStatePaths":[]});
        let body = if gate.is_some() {
            json!({"kind":"seq","name":"outer_body","state":state,"promotedStatePaths":[],"children":[map_verifier("gate"),
                {"kind":"choice","name":"inner_route","state":state,"promotedStatePaths":[],"branches":[{"when":{"kind":"in","value":{"name":"gate","source":"signal","field":"verdict"},"labels":["accepted"]},"node":inner}],"otherwise":mapped_step("skipped_inner")}
            ]})
        } else {
            inner
        };
        let inner_accepted = json!({"kind":"k_of_map","count":1,"value":{"name":"innerVerify","source":"signal","field":"verdict"},"labels":["accepted"]});
        let guard = if let Some(accepted) = gate {
            let gate_accepted = json!({"kind":"k_of_map","count":1,"value":{"name":"gate","source":"signal","field":"verdict"},"labels":["accepted"]});
            json!({"kind":"all","guards":[inner_accepted, if accepted {gate_accepted} else {json!({"kind":"not","guard":gate_accepted})}]})
        } else {
            inner_accepted
        };
        let graph = graph_with_state_children(
            state.clone(),
            json!([
                map_node(&state, body),
                map_choice(state, guard, "selected", "otherwise")
            ]),
        );
        if gate == Some(false) {
            assert_graph_rejected_with(&graph, GraphDiagnosticCode::ChoiceExhaustiveness).await;
        } else {
            ProductionGraphVerifier::new(registry())
                .verify(&graph)
                .await
                .unwrap_or_else(|error| panic!("{error:?}"));
        }
    }
}

#[tokio::test]
async fn first_join_uses_aggregate_selectors_for_deeper_map_facts() {
    for aggregate in [false, true] {
        let state = mapped_items_state();
        let mut guard = json!({"kind":"in","value":{"name":"innerVerify","source":"signal","field":"verdict"},"labels":["accepted"]});
        if aggregate {
            guard["kind"] = json!("k_of_map");
            guard["count"] = json!(1);
        }
        let graph = graph_with_state_children(
            state.clone(),
            json!([
                {"kind":"par","name":"race","state":state,"promotedStatePaths":[],"branches":[map_node(&state,map_verifier("innerVerify"))],"join":{"kind":"first","when":guard}},
                {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
            ]),
        );
        if aggregate {
            assert_graph_accepted(&graph).await;
        } else {
            assert_graph_rejected_with(&graph, GraphDiagnosticCode::InvalidGraphShape).await;
        }
    }
}
