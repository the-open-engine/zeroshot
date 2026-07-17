#[tokio::test]
async fn every_parallel_join_and_group_control_domain_is_admitted() {
    let base_work = valid_graph()["root"]["children"][0].clone();
    for (join, field, label, verifier_branch) in [
        (json!({"kind":"any"}), "joined", "reached", false),
        (
            json!({"kind":"quorum","count":1}),
            "joined",
            "reached",
            false,
        ),
        (
            json!({
                "kind":"first",
                "when":{"kind":"in","value":{"name":"raceVerify","source":"signal","field":"verdict"},"labels":["accepted"]}
            }),
            "raced",
            "satisfied",
            true,
        ),
    ] {
        let mut left = base_work.clone();
        left["name"] = json!("left");
        left["writeBindings"][0]["value"]["node"] = json!("left");
        let mut right = base_work.clone();
        right["name"] = json!("right");
        right["writeBindings"][0]["value"]["node"] = json!("right");
        if verifier_branch {
            right = valid_graph()["root"]["children"][1].clone();
            right["name"] = json!("raceVerify");
        }
        let otherwise =
            (field == "raced").then(|| json!({"kind":"fail","name":"failed","reason":"failed"}));
        let graph = graph_with_root_child(json!({
            "kind":"seq","name":"root","state":record(),"children":[
                {"kind":"par","name":"parallel","state":record(),"branches":[left,right],
                 "promotedStatePaths":[],"join":join},
                {"kind":"choice","name":"afterJoin","state":record(),"branches":[{
                    "when":{"kind":"in","value":{"name":"parallel","source":"group","field":field},"labels":[label]},
                    "node":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
                 }],"otherwise":otherwise,
                 "promotedStatePaths":[]}
            ],"promotedStatePaths":[]
        }));
        ProductionGraphVerifier::new(registry())
            .verify(&graph)
            .await
            .unwrap();
    }
}

fn quorum_flow_branch(name: &str, target: &str, state: &Value) -> Value {
    let writer = format!("{name}Writer");
    json!({
        "kind":"seq","name":name,"state":state.clone(),
        "children":[
            {
                "kind":"step","name":writer,"worker":"worker.main@1",
                "input":record(),
                "output":{"kind":"record","fields":{
                    "result":{"type":{"kind":"integer"},"required":true}
                }},
                "inputBindings":[
                    {"target":["value"],"value":{"source":"state","path":["value"]}}
                ],
                "writeBindings":[{
                    "value":{"node":writer,"channel":"out","path":["result"]},
                    "target":[target]
                }],
                "timeoutMs":1,"attempts":1
            },
            {
                "kind":"choice","name":format!("{name}Routed"),"state":state.clone(),
                "branches":[{
                    "when":{
                        "kind":"in",
                        "value":{"name":writer,"source":"error","field":null},
                        "labels":["timeout","crash","malformed","refusal"]
                    },
                    "node":{
                        "kind":"fail","name":format!("{name}Failed"),
                        "reason":"worker_failed"
                    }
                }],
                "otherwise":{
                    "kind":"step","name":format!("{name}Continuation"),
                    "worker":"worker.main@1","input":record(),
                    "output":{"kind":"record","fields":{
                        "result":{"type":{"kind":"integer"},"required":true}
                    }},
                    "inputBindings":[
                        {"target":["value"],"value":{"source":"state","path":["value"]}}
                    ],
                    "writeBindings":[],"timeoutMs":1,"attempts":1
                },
                "promotedStatePaths":[]
            }
        ],
        "promotedStatePaths":[[target]]
    })
}

fn quorum_flow_graph(count: u64) -> GraphSpec {
    let state = json!({
        "kind":"record",
        "fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "leftResult":{"type":{"kind":"integer"},"required":false},
            "rightResult":{"type":{"kind":"integer"},"required":false}
        }
    });
    graph_with_state_children(
        state.clone(),
        json!([
                {
                    "kind":"par","name":"parallel","state":state.clone(),
                    "branches":[
                        quorum_flow_branch("left","leftResult",&state),
                        quorum_flow_branch("right","rightResult",&state)
                    ],
                    "promotedStatePaths": [["leftResult"], ["rightResult"]],
                    "join": {"kind":"quorum","count":count}
                },
                {
                    "kind":"choice","name":"afterParallel","state":state,
                    "branches":[{
                        "when":{
                            "kind":"in",
                            "value":{"name":"parallel","source":"group","field":"joined"},
                            "labels":["reached"]
                        },
                        "node":{
                            "kind": "succeed", "name": "done",
                            "output": {
                                "kind": "record",
                                "fields": {
                                    "leftResult": { "type": { "kind": "integer" }, "required": true },
                                    "rightResult": { "type": { "kind": "integer" }, "required": true }
                                }
                            },
                            "bindings": [
                                {"target":["leftResult"],"value":{"source":"state","path":["leftResult"]}},
                                {"target":["rightResult"],"value":{"source":"state","path":["rightResult"]}}
                            ]
                        }
                    }],
                    "otherwise":{
                        "kind":"succeed","name":"parallelFailed",
                        "output":{"kind":"null"},"bindings":[]
                    },
                    "promotedStatePaths":[]
                }
        ]),
    )
}

#[tokio::test]
async fn quorum_promotion_and_flow_use_the_authored_completion_count() {
    let quorum_one = ProductionGraphVerifier::new(registry())
        .verify(&quorum_flow_graph(1))
        .await
        .unwrap_err();
    assert!(rejection_codes(quorum_one).contains(&GraphDiagnosticCode::UndefinedRead));

    ProductionGraphVerifier::new(registry())
        .verify(&quorum_flow_graph(2))
        .await
        .unwrap();
}

fn shared_quorum_flow_graph(count: u64) -> GraphSpec {
    let mut value = serde_json::to_value(quorum_flow_graph(2)).unwrap();
    let mut third = value["root"]["children"][0]["branches"][0].clone();
    third["name"] = json!("third");
    third["children"][0]["name"] = json!("thirdWriter");
    third["children"][0]["writeBindings"][0]["value"]["node"] = json!("thirdWriter");
    third["children"][1]["name"] = json!("thirdRouted");
    third["children"][1]["branches"][0]["when"]["value"]["name"] = json!("thirdWriter");
    third["children"][1]["branches"][0]["node"]["name"] = json!("thirdFailed");
    third["children"][1]["otherwise"]["name"] = json!("thirdContinuation");
    let left = value["root"]["children"][0]["branches"][0].clone();
    let right = value["root"]["children"][0]["branches"][1].clone();
    value["root"]["children"][0]["branches"] = json!([left, right, third]);
    value["root"]["children"][0]["join"]["count"] = json!(count);
    value["root"]["children"][0]["promotedStatePaths"] = json!([["leftResult"]]);
    value["root"]["children"][1]["branches"][0]["node"]["output"]["fields"] = json!({
        "leftResult": { "type": { "kind": "integer" }, "required": true }
    });
    value["root"]["children"][1]["branches"][0]["node"]["bindings"] = json!([{
        "target":["leftResult"],"value":{"source":"state","path":["leftResult"]}
    }]);
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn quorum_effects_cover_every_size_count_completion_set() {
    let quorum_one = ProductionGraphVerifier::new(registry())
        .verify(&shared_quorum_flow_graph(1))
        .await
        .unwrap_err();
    assert!(rejection_codes(quorum_one).contains(&GraphDiagnosticCode::UndefinedRead));

    ProductionGraphVerifier::new(registry())
        .verify(&shared_quorum_flow_graph(2))
        .await
        .unwrap();
}

fn parallel_terminal_graph(join: Value) -> GraphSpec {
    let work = valid_graph()["root"]["children"][0].clone();
    graph_with_root_child(json!({
        "kind":"seq", "name":"root", "state":record(), "children":[
            {
                "kind":"par", "name":"parallel", "state":record(),
                "branches":[
                    work,
                    {"kind":"succeed","name":"leftDone","output":{"kind":"null"},"bindings":[]},
                    {"kind":"succeed","name":"rightDone","output":{"kind":"null"},"bindings":[]}
                ],
                "promotedStatePaths":[], "join":join
            },
            {"kind":"succeed","name":"afterParallel","output":{"kind":"null"},"bindings":[]}
        ],
        "promotedStatePaths":[]
    }))
}

#[tokio::test]
async fn parallel_terminal_reachability_uses_the_join_completion_count() {
    ProductionGraphVerifier::new(registry())
        .verify(&parallel_terminal_graph(json!({"kind":"quorum","count":1})))
        .await
        .unwrap();

    for join in [json!({"kind":"quorum","count":2}), json!({"kind":"all"})] {
        let error = ProductionGraphVerifier::new(registry())
            .verify(&parallel_terminal_graph(join))
            .await
            .unwrap_err();
        let VerificationError::Rejected { diagnostics } = error else {
            panic!("unreachable successor must be a graph rejection")
        };
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == GraphDiagnosticCode::Reachability
                && diagnostic
                    .related_nodes
                    .iter()
                    .any(|name| name.as_str() == "afterParallel")
        }));
    }
}

#[tokio::test]
async fn quorum_completion_sets_preserve_shared_guard_correlations() {
    let mut value = valid_graph();
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        value["root"]["children"][1].clone(),
        {
            "kind":"par", "name":"correlatedQuorum", "state":record(),
            "branches":[
                conditional_quorum_branch(
                    "acceptedBranch","accepted","acceptedWork","rejectedDone"
                ),
                conditional_quorum_branch(
                    "rejectedBranch","rejected","rejectedWork","acceptedDone"
                )
            ],
            "promotedStatePaths":[],
            "join":{"kind":"quorum","count":2}
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

fn conditional_quorum_branch(name: &str, label: &str, worker: &str, terminal: &str) -> Value {
    json!({
        "kind":"choice","name":name,"state":record(),
        "branches":[{
            "when":{
                "kind":"in",
                "value":{"name":"verify","source":"signal","field":"verdict"},
                "labels":[label]
            },
            "node":{
                "kind":"step","name":worker,"worker":"worker.main@1",
                "input":record(),
                "output":{"kind":"record","fields":{
                    "result":{"type":{"kind":"number"},"required":true}
                }},
                "inputBindings":[
                    {"target":["value"],"value":{"source":"state","path":["value"]}}
                ],
                "writeBindings":[],"timeoutMs":1,"attempts":1
            }
        }],
        "otherwise":{
            "kind":"succeed","name":terminal,"output":{"kind":"null"},"bindings":[]
        },
        "promotedStatePaths":[]
    })
}

fn correlated_flow_writer() -> Value {
    json!({
        "kind":"seq","name":"writerBranch","state":record(),
        "children":[
            {
                "kind":"step","name":"sharedWriter","worker":"worker.main@1",
                "input":record(),
                "output":{"kind":"record","fields":{
                    "result":{"type":{"kind":"number"},"required":true}
                }},
                "inputBindings":[
                    {"target":["value"],"value":{"source":"state","path":["value"]}}
                ],
                "writeBindings":[{
                    "value":{"node":"sharedWriter","channel":"out","path":["result"]},
                    "target":["result"]
                }],
                "timeoutMs":1,"attempts":1
            },
            {
                "kind":"choice","name":"writerOutcome","state":record(),
                "branches":[{
                    "when":{
                        "kind":"in","value":{"name":"sharedWriter","source":"error"},
                        "labels":["timeout","crash","malformed","refusal"]
                    },
                    "node":{
                        "kind":"succeed","name":"writerFailed",
                        "output":{"kind":"null"},"bindings":[]
                    }
                }],
                "otherwise":{
                    "kind":"step","name":"writerContinuation","worker":"worker.main@1",
                    "input":record(),
                    "output":{"kind":"record","fields":{
                        "result":{"type":{"kind":"number"},"required":true}
                    }},
                    "inputBindings":[
                        {"target":["value"],"value":{"source":"state","path":["value"]}}
                    ],
                    "writeBindings":[],"timeoutMs":1,"attempts":1
                },
                "promotedStatePaths":[]
            }
        ],
        "promotedStatePaths":[["result"]]
    })
}

#[tokio::test]
async fn quorum_flow_uses_only_jointly_satisfiable_completion_sets() {
    let mut value = valid_graph();
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        value["root"]["children"][1].clone(),
        {
            "kind":"par", "name":"correlatedFlowQuorum", "state":record(),
            "branches":[
                conditional_quorum_branch(
                    "acceptedBranch","accepted","acceptedWork","rejectedDone"
                ),
                correlated_flow_writer(),
                conditional_quorum_branch(
                    "rejectedBranch","rejected","rejectedWork","acceptedDone"
                )
            ],
            "promotedStatePaths":[["result"]],
            "join":{"kind":"quorum","count":2}
        },
        {
            "kind":"choice","name":"afterCorrelatedQuorum","state":record(),
            "branches":[{
                "when":{
                    "kind":"in",
                    "value":{
                        "name":"correlatedFlowQuorum",
                        "source":"group",
                        "field":"joined"
                    },
                    "labels":["reached"]
                },
                "node":{
                    "kind":"succeed", "name":"done",
                    "output":{
                        "kind":"record",
                        "fields":{"result":{"type":{"kind":"number"},"required":true}}
                    },
                    "bindings":[{
                        "target":["result"],
                        "value":{"source":"state","path":["result"]}
                    }]
                }
            }],
            "otherwise":{
                "kind":"succeed","name":"correlatedQuorumFailed",
                "output":{"kind":"null"},"bindings":[]
            },
            "promotedStatePaths":[]
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn output_signal_and_diagnostic_binding_channels_are_type_checked_and_admitted() {
    let graph = graph_with_root_child(json!({
        "kind":"seq","name":"root","state":record(),"children":[
            {"kind":"verifier","name":"verify","worker":"worker.verify@1",
             "input":{"kind":"null"},
             "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
             "inputBindings":[],
             "writeBindings":[
                {"value":{"node":"verify","channel":"out","path":["result"]},"target":["result"]},
                {"value":{"node":"verify","channel":"signal","path":["verdict"]},"target":["verdict"]},
                {"value":{"node":"verify","channel":"diagnostic","path":["code"]},"target":["diagnostic"]}
             ],
             "timeoutMs":1,"attempts":1,
             "signals":{"verdict":["accepted","rejected"]},
             "diagnostic":{"kind":"record","fields":{"code":{"type":{"kind":"number"},"required":true}}}},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ],"promotedStatePaths":[]
    }));
    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}
use super::*;
