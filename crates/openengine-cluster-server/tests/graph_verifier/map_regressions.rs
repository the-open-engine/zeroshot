use super::*;

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
    .unwrap()
}

fn nested_map_group_aggregate_graph() -> GraphSpec {
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
                    "kind":"map","name":"innerMap","state":state.clone(),
                    "over":{"source":"state","path":["innerItems"]},"maxItems":1,
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
                        "value":{"name":"innerMap","source":"group","field":"overflow"},
                        "labels":["overflow"]
                    },
                    "node":{
                        "kind":"succeed","name":"twoInnerOverflows",
                        "output":{"kind":"null"},"bindings":[]
                    }
                }],
                "otherwise":{
                    "kind":"succeed","name":"fewerInnerOverflows",
                    "output":{"kind":"null"},"bindings":[]
                },
                "promotedStatePaths":[]
            }
        ]),
    )
}

fn mapped_parallel_control_correlation_graph(reached_count: u64, rejected_count: u64) -> GraphSpec {
    let state = json!({
        "kind":"record",
        "fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "items":{
                "type":{"kind":"array","items":{"kind":"null"}},
                "required":true
            }
        }
    });
    let item_verify = json!({
        "kind":"verifier","name":"itemVerify","worker":"worker.verify@1",
        "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
        "signals":{"verdict":["accepted","rejected"]},
        "diagnostic":{"kind":"record","fields":{}}
    });
    let accepted_work = json!({
        "kind":"step","name":"acceptedWork","worker":"worker.main@1",
        "input":record(),
        "output":{"kind":"record","fields":{
            "result":{"type":{"kind":"integer"},"required":true}
        }},
        "inputBindings":[{
            "target":["value"],
            "value":{"source":"state","path":["value"]}
        }],
        "writeBindings":[],"timeoutMs":1,"attempts":1
    });
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
    let mapped_body = json!({
        "kind":"seq","name":"mappedBody","state":state.clone(),
        "children":[item_verify,inner_parallel],
        "promotedStatePaths":[]
    });
    graph_with_state_children(
        state.clone(),
        json!([
            {
                "kind":"map","name":"map","state":state.clone(),
                "over":{"source":"state","path":["items"]},"maxItems":2,
                "body":mapped_body,"promotedStatePaths":[]
            },
            {
                "kind":"choice","name":"afterMap","state":state,
                "branches":[{
                    "when":{
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
                    },
                    "node":{
                        "kind":"succeed","name":"impossible",
                        "output":{"kind":"null"},"bindings":[]
                    }
                }],
                "otherwise":{
                    "kind":"succeed","name":"possible",
                    "output":{"kind":"null"},"bindings":[]
                },
                "promotedStatePaths":[]
            }
        ]),
    )
}

fn mapped_parallel_multicontrol_correlation_graph() -> GraphSpec {
    let state = json!({
        "kind":"record",
        "fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "items":{
                "type":{"kind":"array","items":{"kind":"null"}},
                "required":true
            }
        }
    });
    let verdict = json!({
        "kind":"verifier","name":"itemVerdict","worker":"worker.verify@1",
        "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
        "signals":{"verdict":["accepted","rejected"]},
        "diagnostic":{"kind":"record","fields":{}}
    });
    let decision = json!({
        "kind":"verifier","name":"itemDecision","worker":"worker.decision@1",
        "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
        "signals":{"decision":["accepted","rejected"]},
        "diagnostic":{"kind":"record","fields":{}}
    });
    let accepted_step = |name: &str| {
        json!({
            "kind":"step","name":name,"worker":"worker.main@1",
            "input":record(),
            "output":{"kind":"record","fields":{
                "result":{"type":{"kind":"integer"},"required":true}
            }},
            "inputBindings":[{
                "target":["value"],
                "value":{"source":"state","path":["value"]}
            }],
            "writeBindings":[],"timeoutMs":1,"attempts":1
        })
    };
    let controlled_step = |name: &str,
                           selector_name: &str,
                           signal_field: &str,
                           accepted_name: &str,
                           rejected_name: &str| {
        json!({
            "kind":"choice","name":name,"state":state.clone(),
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
                "node":accepted_step(accepted_name)
            }],
            "otherwise":{
                "kind":"fail","name":rejected_name,"reason":"rejected"
            },
            "promotedStatePaths":[]
        })
    };
    let completing_branch = json!({
        "kind":"seq","name":"requiresBoth","state":state.clone(),
        "children":[
            controlled_step(
                "verdictRoute",
                "itemVerdict",
                "verdict",
                "acceptedVerdict",
                "rejectedVerdict"
            ),
            controlled_step(
                "decisionRoute",
                "itemDecision",
                "decision",
                "acceptedDecision",
                "rejectedDecision"
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
    let mapped_body = json!({
        "kind":"seq","name":"mappedBody","state":state.clone(),
        "children":[verdict,decision,inner_parallel],
        "promotedStatePaths":[]
    });
    graph_with_state_children(
        state.clone(),
        json!([
            {
                "kind":"map","name":"map","state":state.clone(),
                "over":{"source":"state","path":["items"]},"maxItems":2,
                "body":mapped_body,"promotedStatePaths":[]
            },
            {
                "kind":"choice","name":"afterMap","state":state,
                "branches":[{
                    "when":{
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
                    },
                    "node":{
                        "kind":"succeed","name":"impossible",
                        "output":{"kind":"null"},"bindings":[]
                    }
                }],
                "otherwise":{
                    "kind":"succeed","name":"possible",
                    "output":{"kind":"null"},"bindings":[]
                },
                "promotedStatePaths":[]
            }
        ]),
    )
}

fn mapped_parallel_with_incoming_control_graph(first: bool) -> GraphSpec {
    let state = json!({
        "kind":"record",
        "fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "items":{
                "type":{"kind":"array","items":{"kind":"null"}},
                "required":true
            }
        }
    });
    let gate = json!({
        "kind":"verifier","name":"gate","worker":"worker.verify@1",
        "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
        "signals":{"verdict":["accepted","rejected"]},
        "diagnostic":{"kind":"record","fields":{}}
    });
    let work = json!({
        "kind":"step","name":"mappedWork","worker":"worker.main@1",
        "input":record(),
        "output":{"kind":"record","fields":{
            "result":{"type":{"kind":"integer"},"required":true}
        }},
        "inputBindings":[{
            "target":["value"],
            "value":{"source":"state","path":["value"]}
        }],
        "writeBindings":[],"timeoutMs":1,"attempts":1
    });
    let (join, field, failure_label, branches) = if first {
        (
            json!({
                "kind":"first",
                "when":{
                    "kind":"in",
                    "value":{"name":"gate","source":"signal","field":"verdict"},
                    "labels":["accepted"]
                }
            }),
            "raced",
            "no_satisfier",
            json!([work]),
        )
    } else {
        (
            json!({"kind":"any"}),
            "joined",
            "quorum_unreachable",
            json!([
                {
                    "kind":"choice","name":"mappedRoute","state":state.clone(),
                    "branches":[{
                        "when":{
                            "kind":"in",
                            "value":{"name":"gate","source":"signal","field":"verdict"},
                            "labels":["accepted"]
                        },
                        "node":work
                    }],
                    "otherwise":{
                        "kind":"fail","name":"gateRejected","reason":"gate_rejected"
                    },
                    "promotedStatePaths":[]
                },
                {"kind":"fail","name":"neverCompletes","reason":"never_completes"}
            ]),
        )
    };
    graph_with_state_children(
        state.clone(),
        json!([
            gate,
            {
                "kind":"map","name":"map","state":state.clone(),
                "over":{"source":"state","path":["items"]},"maxItems":2,
                "body":{
                    "kind":"par","name":"mappedParallel","state":state.clone(),
                    "branches":branches,"join":join,"promotedStatePaths":[]
                },
                "promotedStatePaths":[]
            },
            {
                "kind":"choice","name":"afterMap","state":state,
                "branches":[{
                    "when":{
                        "kind":"all",
                        "guards":[
                            {
                                "kind":"in",
                                "value":{"name":"gate","source":"signal","field":"verdict"},
                                "labels":["accepted"]
                            },
                            {
                                "kind":"k_of_map","count":1,
                                "value":{
                                    "name":"mappedParallel",
                                    "source":"group",
                                    "field":field
                                },
                                "labels":[failure_label]
                            }
                        ]
                    },
                    "node":{
                        "kind":"succeed","name":"impossible",
                        "output":{"kind":"null"},"bindings":[]
                    }
                }],
                "otherwise":{
                    "kind":"succeed","name":"possible",
                    "output":{"kind":"null"},"bindings":[]
                },
                "promotedStatePaths":[]
            }
        ]),
    )
}

fn mapped_parallel_with_shared_and_item_controls_graph(first: bool) -> GraphSpec {
    let state = json!({
        "kind":"record",
        "fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "items":{
                "type":{"kind":"array","items":{"kind":"null"}},
                "required":true
            }
        }
    });
    let verifier = |name: &str| {
        json!({
            "kind":"verifier","name":name,"worker":"worker.verify@1",
            "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
            "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
            "signals":{"verdict":["accepted","rejected"]},
            "diagnostic":{"kind":"record","fields":{}}
        })
    };
    let work = json!({
        "kind":"step","name":"mappedWork","worker":"worker.main@1",
        "input":record(),
        "output":{"kind":"record","fields":{
            "result":{"type":{"kind":"integer"},"required":true}
        }},
        "inputBindings":[{
            "target":["value"],
            "value":{"source":"state","path":["value"]}
        }],
        "writeBindings":[],"timeoutMs":1,"attempts":1
    });
    let completion_guard = json!({
        "kind":"all",
        "guards":[
            {
                "kind":"in",
                "value":{"name":"gate","source":"signal","field":"verdict"},
                "labels":["accepted"]
            },
            {
                "kind":"in",
                "value":{"name":"itemVerify","source":"signal","field":"verdict"},
                "labels":["accepted"]
            }
        ]
    });
    let (join, field, success_label, failure_label, branches) = if first {
        (
            json!({"kind":"first","when":completion_guard}),
            "raced",
            "satisfied",
            "no_satisfier",
            json!([work]),
        )
    } else {
        (
            json!({"kind":"any"}),
            "joined",
            "reached",
            "quorum_unreachable",
            json!([
                {
                    "kind":"choice","name":"mappedRoute","state":state.clone(),
                    "branches":[{
                        "when":completion_guard,
                        "node":work
                    }],
                    "otherwise":{
                        "kind":"fail","name":"routeRejected","reason":"route_rejected"
                    },
                    "promotedStatePaths":[]
                },
                {"kind":"fail","name":"neverCompletes","reason":"never_completes"}
            ]),
        )
    };
    let count = |name: &str, source: &str, field: &str, label: &str| {
        json!({
            "kind":"k_of_map","count":1,
            "value":{"name":name,"source":source,"field":field},
            "labels":[label]
        })
    };
    graph_with_state_children(
        state.clone(),
        json!([
            verifier("gate"),
            {
                "kind":"map","name":"map","state":state.clone(),
                "over":{"source":"state","path":["items"]},"maxItems":2,
                "body":{
                    "kind":"seq","name":"mappedBody","state":state.clone(),
                    "children":[
                        verifier("itemVerify"),
                        {
                            "kind":"par","name":"mappedParallel","state":state.clone(),
                            "branches":branches,"join":join,"promotedStatePaths":[]
                        }
                    ],
                    "promotedStatePaths":[]
                },
                "promotedStatePaths":[]
            },
            {
                "kind":"choice","name":"afterMap","state":state,
                "branches":[{
                    "when":{
                        "kind":"all",
                        "guards":[
                            {
                                "kind":"in",
                                "value":{"name":"gate","source":"signal","field":"verdict"},
                                "labels":["accepted"]
                            },
                            count("mappedParallel","group",field,success_label),
                            count("mappedParallel","group",field,failure_label),
                            count("itemVerify","signal","verdict","accepted"),
                            count("itemVerify","signal","verdict","rejected")
                        ]
                    },
                    "node":{
                        "kind":"succeed","name":"mixedOutcomes",
                        "output":{"kind":"null"},"bindings":[]
                    }
                }],
                "otherwise":{
                    "kind":"succeed","name":"otherOutcomes",
                    "output":{"kind":"null"},"bindings":[]
                },
                "promotedStatePaths":[]
            }
        ]),
    )
}

fn mapped_missing_successor_outcome_graph(conditional: bool) -> GraphSpec {
    let state = json!({
        "kind":"record",
        "fields":{
            "items":{
                "type":{"kind":"array","items":{"kind":"null"}},
                "required":true
            }
        }
    });
    let verifier = |name: &str| {
        json!({
            "kind":"verifier","name":name,"worker":"worker.verify@1",
            "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
            "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
            "signals":{"verdict":["accepted","rejected"]},
            "diagnostic":{"kind":"record","fields":{}}
        })
    };
    let mapped_signal = |name: &str, labels: Value| {
        json!({
            "kind":"k_of_map","count":1,
            "value":{"name":name,"source":"signal","field":"verdict"},
            "labels":labels
        })
    };
    let mapped_error = |name: &str| {
        json!({
            "kind":"k_of_map","count":1,
            "value":{"name":name,"source":"error","field":null},
            "labels":["timeout","crash","malformed","refusal"]
        })
    };
    let body = if conditional {
        json!({
            "kind":"seq","name":"mappedSequence","state":state.clone(),
            "children":[
                verifier("firstVerify"),
                {
                    "kind":"choice","name":"mappedRoute","state":state.clone(),
                    "branches":[{
                        "when":{
                            "kind":"in",
                            "value":{
                                "name":"firstVerify",
                                "source":"signal",
                                "field":"verdict"
                            },
                            "labels":["accepted"]
                        },
                        "node":verifier("secondVerify")
                    }],
                    "otherwise":{
                        "kind":"fail","name":"firstRejected","reason":"first_rejected"
                    },
                    "promotedStatePaths":[]
                }
            ],
            "promotedStatePaths":[]
        })
    } else {
        json!({
            "kind":"seq","name":"mappedSequence","state":state.clone(),
            "children":[verifier("firstVerify"),verifier("secondVerify")],
            "promotedStatePaths":[]
        })
    };
    graph_with_state_children(
        state.clone(),
        json!([
            {
                "kind":"map","name":"map","state":state.clone(),
                "over":{"source":"state","path":["items"]},"maxItems":1,
                "body":body,
                "promotedStatePaths":[]
            },
            {
                "kind":"choice","name":"afterMap","state":state,
                "branches":[{
                    "when":{
                        "kind":"all",
                        "guards":[
                            mapped_signal("firstVerify", json!(["accepted"])),
                            {
                                "kind":"not",
                                "guard":{
                                    "kind":"any",
                                    "guards":[
                                        mapped_signal(
                                            "secondVerify",
                                            json!(["accepted","rejected"])
                                        ),
                                        mapped_error("secondVerify")
                                    ]
                                }
                            }
                        ]
                    },
                    "node":{
                        "kind":"succeed","name":"impossible",
                        "output":{"kind":"null"},"bindings":[]
                    }
                }],
                "otherwise":{
                    "kind":"succeed","name":"possible",
                    "output":{"kind":"null"},"bindings":[]
                },
                "promotedStatePaths":[]
            }
        ]),
    )
}

fn mapped_missing_group_descendant_outcome_graph(loop_group: bool) -> GraphSpec {
    let mut value = serde_json::to_value(mapped_missing_successor_outcome_graph(true)).unwrap();
    let selected =
        value["root"]["children"][0]["body"]["children"][1]["branches"][0]["node"].clone();
    let state = value["root"]["children"][0]["body"]["state"].clone();
    value["root"]["children"][0]["body"]["children"][1]["branches"][0]["node"] = if loop_group {
        json!({
            "kind":"loop","name":"secondLoop","state":state,
            "body":selected,"maxIterations":1,
            "until":{
                "kind":"in",
                "value":{
                    "name":"secondVerify",
                    "source":"signal",
                    "field":"verdict"
                },
                "labels":["accepted"]
            },
            "promotedStatePaths":[]
        })
    } else {
        json!({
            "kind":"par","name":"secondParallel","state":state,
            "branches":[selected],"join":{"kind":"all"},
            "promotedStatePaths":[]
        })
    };
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn mapped_error_outcome_does_not_expose_success_only_results() {
    let error = ProductionGraphVerifier::new(registry())
        .verify(&mapped_error_read_graph())
        .await
        .unwrap_err();
    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn nested_maps_preserve_outer_index_definition_isolation() {
    let error = ProductionGraphVerifier::new(registry())
        .verify(&nested_map_graph())
        .await
        .unwrap_err();
    let codes = rejection_codes(error);
    assert!(codes.contains(&GraphDiagnosticCode::UndefinedRead));
    assert!(!codes.contains(&GraphDiagnosticCode::SchemaSafety));
}

#[tokio::test]
async fn k_of_map_counts_group_controls_across_the_enclosing_map() {
    ProductionGraphVerifier::new(registry())
        .verify(&nested_map_group_aggregate_graph())
        .await
        .unwrap();
}

#[tokio::test]
async fn mapped_parallel_controls_preserve_per_item_branch_correlation() {
    let error = ProductionGraphVerifier::new(registry())
        .verify(&mapped_parallel_control_correlation_graph(2, 1))
        .await
        .unwrap_err();
    let codes = rejection_codes(error);
    assert!(
        codes.contains(&GraphDiagnosticCode::ChoiceExhaustiveness),
        "unexpected rejection codes: {codes:?}"
    );
}

#[tokio::test]
async fn mapped_parallel_controls_allow_jointly_possible_item_counts() {
    ProductionGraphVerifier::new(registry())
        .verify(&mapped_parallel_control_correlation_graph(1, 1))
        .await
        .unwrap();
}

#[tokio::test]
async fn mapped_parallel_controls_correlate_dependencies_omitted_from_outer_guard() {
    let error = ProductionGraphVerifier::new(registry())
        .verify(&mapped_parallel_multicontrol_correlation_graph())
        .await
        .unwrap_err();
    let codes = rejection_codes(error);
    assert!(
        codes.contains(&GraphDiagnosticCode::ChoiceExhaustiveness),
        "unexpected rejection codes: {codes:?}"
    );
}

#[tokio::test]
async fn mapped_parallel_controls_correlate_incoming_controls_per_item() {
    for first in [false, true] {
        let error = ProductionGraphVerifier::new(registry())
            .verify(&mapped_parallel_with_incoming_control_graph(first))
            .await
            .unwrap_err();
        let codes = rejection_codes(error);
        assert!(
            codes.contains(&GraphDiagnosticCode::ChoiceExhaustiveness),
            "mapped {} control admitted an outcome impossible for the incoming gate: {codes:?}",
            if first { "raced" } else { "joined" }
        );
    }
}

#[tokio::test]
async fn mapped_parallel_controls_keep_item_variation_with_incoming_controls() {
    for first in [false, true] {
        ProductionGraphVerifier::new(registry())
            .verify(&mapped_parallel_with_shared_and_item_controls_graph(first))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn mapped_sequence_success_cannot_omit_guaranteed_successor_outcome() {
    let error = ProductionGraphVerifier::new(registry())
        .verify(&mapped_missing_successor_outcome_graph(false))
        .await
        .unwrap_err();
    let codes = rejection_codes(error);
    assert!(
        codes.contains(&GraphDiagnosticCode::ChoiceExhaustiveness),
        "mapped sequence admitted a missing successor outcome: {codes:?}"
    );
}

#[tokio::test]
async fn mapped_choice_success_cannot_omit_selected_successor_outcome() {
    let error = ProductionGraphVerifier::new(registry())
        .verify(&mapped_missing_successor_outcome_graph(true))
        .await
        .unwrap_err();
    let codes = rejection_codes(error);
    assert!(
        codes.contains(&GraphDiagnosticCode::ChoiceExhaustiveness),
        "mapped choice admitted a missing selected outcome: {codes:?}"
    );
}

#[tokio::test]
async fn mapped_selected_par_all_cannot_omit_guaranteed_descendant_outcome() {
    let error = ProductionGraphVerifier::new(registry())
        .verify(&mapped_missing_group_descendant_outcome_graph(false))
        .await
        .unwrap_err();
    let codes = rejection_codes(error);
    assert!(
        codes.contains(&GraphDiagnosticCode::ChoiceExhaustiveness),
        "mapped par-all admitted a missing guaranteed descendant outcome: {codes:?}"
    );
}

#[tokio::test]
async fn mapped_selected_loop_cannot_omit_guaranteed_body_outcome() {
    let error = ProductionGraphVerifier::new(registry())
        .verify(&mapped_missing_group_descendant_outcome_graph(true))
        .await
        .unwrap_err();
    let codes = rejection_codes(error);
    assert!(
        codes.contains(&GraphDiagnosticCode::ChoiceExhaustiveness),
        "mapped loop admitted a missing guaranteed body outcome: {codes:?}"
    );
}

#[tokio::test]
async fn mapped_group_descendants_remain_absent_when_their_route_is_not_selected() {
    for loop_group in [false, true] {
        let mut value =
            serde_json::to_value(mapped_missing_group_descendant_outcome_graph(loop_group))
                .unwrap();
        value["root"]["children"][1]["branches"][0]["when"]["guards"][0]["labels"] =
            json!(["rejected"]);
        ProductionGraphVerifier::new(registry())
            .verify(&serde_json::from_value(value).unwrap())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn mapped_sequence_keeps_successor_outcomes_after_predecessor_errors() {
    let mut value = serde_json::to_value(mapped_missing_successor_outcome_graph(false)).unwrap();
    value["root"]["children"][1]["branches"][0]["when"] = json!({
        "kind":"all",
        "guards":[
            {
                "kind":"not",
                "guard":{
                    "kind":"k_of_map","count":1,
                    "value":{
                        "name":"firstVerify",
                        "source":"signal",
                        "field":"verdict"
                    },
                    "labels":["accepted","rejected"]
                }
            },
            {
                "kind":"k_of_map","count":1,
                "value":{
                    "name":"secondVerify",
                    "source":"signal",
                    "field":"verdict"
                },
                "labels":["accepted"]
            }
        ]
    });
    ProductionGraphVerifier::new(registry())
        .verify(&serde_json::from_value(value).unwrap())
        .await
        .unwrap();
}
