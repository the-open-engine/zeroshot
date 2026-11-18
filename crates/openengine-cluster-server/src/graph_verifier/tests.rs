use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    GraphSpec, Guard, NodeName, NonEmptyVec, PositiveInteger, StructuralBounds, TerminationWitness,
};
use serde_json::json;

use super::{Analyzer, VerificationError, finalize_verified_with_invariant_probe};

#[test]
fn post_validation_invariant_failure_is_internal() -> Result<(), Box<dyn std::error::Error>> {
    let graph: GraphSpec = serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":{"kind":"null"},
        "policy":{"policy":"policy.strict@1","default":"deny"},
        "root":{
            "kind":"seq","name":"duplicate","state":{"kind":"null"},
            "children":[
                {"kind":"step","name":"duplicate","worker":"worker@1","input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1},
                {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
            ],
            "promotedStatePaths":[]
        }
    }))?;
    let one = PositiveInteger::new(1)?;
    let duplicate = NodeName::new("duplicate")?;
    let bounds = StructuralBounds {
        termination: TerminationWitness::Acyclic {
            order: NonEmptyVec::new(vec![duplicate.clone()])?,
        },
        max_node_executions: one,
        peak_concurrency: one,
        attempts_per_node: BTreeMap::from([(duplicate, one)]),
    };

    assert_eq!(
        finalize_verified_with_invariant_probe(&graph, bounds, true),
        Err(VerificationError::Internal(
            "injected post-validation invariant failure".to_owned()
        ))
    );
    Ok(())
}

#[test]
fn broad_aggregate_group_guards_preserve_both_refinement_outcomes()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted = json!({
        "kind":"in",
        "value":{"name":"verify","source":"signal","field":"verdict"},
        "labels":["accepted"]
    });
    for (join, field, labels) in [
        (
            json!({"kind":"all"}),
            "joined",
            json!(["reached", "quorum_unreachable"]),
        ),
        (
            json!({"kind":"first","when":accepted}),
            "raced",
            json!(["satisfied", "no_satisfier"]),
        ),
    ] {
        let state = json!({"kind":"record","fields":{
            "items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}
        }});
        let work = json!({
            "kind":"step","name":"work","worker":"work@1",
            "input":{"kind":"null"},"output":{"kind":"null"},
            "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1
        });
        let mut verify = work.clone();
        verify["kind"] = json!("verifier");
        verify["name"] = json!("verify");
        verify["worker"] = json!("verify@1");
        verify["signals"] = json!({"verdict":["accepted","rejected"]});
        verify["diagnostic"] = json!({"kind":"null"});
        let route = json!({
            "kind":"choice","name":"localRoute","state":state,
            "promotedStatePaths":[],"branches":[{"when":accepted,"node":work}],
            "otherwise":{"kind":"fail","name":"rejected","reason":"rejected"}
        });
        let candidate = json!({
            "kind":"seq","name":"candidate","state":state,
            "promotedStatePaths":[],"children":[verify,route]
        });
        let parallel = json!({
            "kind":"par","name":"parallel","state":state,
            "join":join,"promotedStatePaths":[],"branches":[candidate]
        });
        let mapped = json!({
            "kind":"map","name":"map","state":state,
            "over":{"source":"state","path":["items"]},"maxItems":1,
            "promotedStatePaths":[],"body":parallel
        });
        let graph: GraphSpec = serde_json::from_value(json!({
            "profile":"openengine.graph.full/v1",
            "initialInput":state,"policy":{"policy":"policy.strict@1","default":"deny"},
            "root":{
                "kind":"seq","name":"root","state":state,"promotedStatePaths":[],
                "children":[mapped,{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]
            }
        }))?;
        let guard: Guard = serde_json::from_value(json!({
            "kind":"k_of_map","count":1,
            "value":{"name":"parallel","source":"group","field":field},
            "labels":labels
        }))?;
        let mut analyzer = Analyzer::new(&graph);
        analyzer.analyze();
        assert!(
            analyzer.diagnostics.is_empty(),
            "{:?}",
            analyzer.diagnostics
        );
        let mut proof = analyzer.control_proof(&[&guard], &[]).unwrap();
        let satisfied = proof.guard(&guard).unwrap();
        let failed = proof
            .diagram
            .failure(&proof.controls, &NodeName::new("parallel")?)
            .unwrap();
        let succeeded = proof.diagram.not(failed).unwrap();
        for outcome in [failed, succeeded] {
            let both = proof.diagram.and(satisfied, outcome).unwrap();
            assert_eq!(proof.possible(both), Some(true), "{field}");
        }
    }
    Ok(())
}

/// Parallel reviewers that each report a verdict and an outcome, read by a choice on the
/// verdicts and a nested choice on the outcomes.
fn parallel_reviews(reviewers: usize) -> GraphSpec {
    let state = json!({"kind":"null"});
    let names = (0..reviewers)
        .map(|index| format!("review{index}"))
        .collect::<Vec<_>>();
    let reads = |source: &str, field: Option<&str>, labels: &[&str]| {
        names
            .iter()
            .map(|name| {
                json!({"kind":"in","value":{"name":name,"source":source,"field":field},"labels":labels})
            })
            .collect::<Vec<_>>()
    };
    let agree =
        |outcome: &str| json!({"kind":"all","guards":reads("signal", Some("outcome"), &[outcome])});
    let fail = |reason: &str| json!({"kind":"fail","name":reason,"reason":reason});
    let branches = names
        .iter()
        .map(|name| {
            json!({
                "kind":"verifier","name":name,"worker":"review@1",
                "input":{"kind":"null"},"output":{"kind":"null"},
                "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
                "signals":{
                    "verdict":["accepted","blocked","rejected"],
                    "outcome":["already_fixed","candidate","needs_attention"]
                },
                "diagnostic":{"kind":"null"}
            })
        })
        .collect::<Vec<_>>();
    let outcome = json!({
        "kind":"choice","name":"reviewed_outcome","state":state,"promotedStatePaths":[],
        "branches":[
            {"when":agree("already_fixed"),"node":fail("already_fixed")},
            {"when":agree("needs_attention"),"node":fail("needs_attention")},
            {"when":{"kind":"not","guard":{"kind":"any","guards":[
                agree("candidate"), agree("already_fixed"), agree("needs_attention")
            ]}},"node":fail("outcome_mismatch")}
        ],
        "otherwise":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
    });
    let errors = ["timeout", "crash", "malformed", "refusal"];
    serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":state,"policy":{"policy":"policy.strict@1","default":"deny"},
        "root":{
            "kind":"seq","name":"root","state":state,"promotedStatePaths":[],
            "children":[
                {"kind":"par","name":"reviews","state":state,"join":{"kind":"all"},
                    "promotedStatePaths":[],"branches":branches},
                {"kind":"choice","name":"review_result","state":state,"promotedStatePaths":[],
                    "branches":[
                        {"when":{"kind":"any","guards":reads("error", None, &errors)},
                            "node":fail("review_failed")},
                        {"when":{"kind":"any","guards":reads("signal", Some("verdict"), &["blocked"])},
                            "node":fail("blocked")},
                        {"when":{"kind":"all","guards":reads("signal", Some("verdict"), &["accepted"])},
                            "node":outcome}
                    ],
                    "otherwise":fail("rejected")}
            ]
        }
    }))
    .unwrap()
}

#[test]
fn parallel_reviewers_with_two_signals_add_search_work_instead_of_multiplying_it() {
    let steps = [2, 4, 8, 16].map(|reviewers| search_steps(&parallel_reviews(reviewers)));
    println!("search steps for 2, 4, 8 and 16 reviewers: {steps:?}");
    // One reviewer has 13 outcomes; listing joint outcomes would multiply by 169 per doubling
    // from two reviewers on. Each doubling here costs a small constant factor.
    for pair in steps.windows(2) {
        assert!(pair[1] > pair[0] && pair[1] < 4 * pair[0], "{steps:?}");
    }
    assert!(steps[3] < 50_000, "{steps:?}");
}

fn items_state() -> serde_json::Value {
    json!({"kind":"record","fields":{
        "items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}
    }})
}

fn mapped_reviewer(name: &str) -> serde_json::Value {
    json!({
        "kind":"verifier","name":name,"worker":"review@1",
        "input":{"kind":"null"},"output":{"kind":"null"},
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
        "signals":{"verdict":["accepted","rejected"]},"diagnostic":{"kind":"null"}
    })
}

fn item_count(name: &str, field: Option<&str>, count: u64) -> serde_json::Value {
    let (source, labels) = match field {
        Some(_) => ("signal", json!(["accepted", "rejected"])),
        None => ("error", json!(["timeout", "crash", "malformed", "refusal"])),
    };
    json!({"kind":"k_of_map","count":count,
        "value":{"name":name,"source":source,"field":field},"labels":labels})
}

/// A map over `body`, then a choice that fails when `guard` holds over the map's items.
fn mapped_check(body: serde_json::Value, guard: serde_json::Value) -> GraphSpec {
    let state = items_state();
    serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1","initialInput":state,
        "policy":{"policy":"policy.strict@1","default":"deny"},
        "root":{"kind":"seq","name":"root","state":state,"promotedStatePaths":[],"children":[
            {"kind":"map","name":"each","state":state,"body":body,
                "over":{"source":"state","path":["items"]},"maxItems":3,"promotedStatePaths":[]},
            {"kind":"choice","name":"check","state":state,"promotedStatePaths":[],
                "branches":[{"when":guard,"node":{"kind":"fail","name":"failed","reason":"failed"}}],
                "otherwise":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}}
        ]}
    }))
    .unwrap()
}

fn search_steps(graph: &GraphSpec) -> u64 {
    let mut analyzer = Analyzer::new(graph);
    assert!(analyzer.analyze().is_some(), "{:?}", analyzer.diagnostics);
    analyzer.search_steps
}

#[test]
fn one_guard_over_many_nodes_of_a_map_adds_search_work_instead_of_multiplying_it() {
    for parallel in [false, true] {
        let steps = [2, 4, 8, 16].map(|reviewers| {
            let names = (0..reviewers)
                .map(|index| format!("review{index}"))
                .collect::<Vec<_>>();
            let nodes = names
                .iter()
                .map(|name| mapped_reviewer(name))
                .collect::<Vec<_>>();
            let body = if parallel {
                json!({"kind":"par","name":"reviews","state":items_state(),"join":{"kind":"all"},
                    "promotedStatePaths":[],"branches":nodes})
            } else {
                json!({"kind":"seq","name":"reviews","state":items_state(),
                    "promotedStatePaths":[],"children":nodes})
            };
            let failed = names
                .iter()
                .map(|name| item_count(name, None, 1))
                .collect::<Vec<_>>();
            search_steps(&mapped_check(body, json!({"kind":"any","guards":failed})))
        });
        println!(
            "search steps for 2, 4, 8 and 16 mapped reviewers (parallel: {parallel}): {steps:?}"
        );
        // Listing the joint outcomes of one item multiplies by about four per reviewer and
        // passed the ceiling at nine or ten of them. The parallel shape also needs thresholds
        // to count equal conditions once: its join rule repeats one condition per branch.
        for pair in steps.windows(2) {
            assert!(pair[1] > pair[0] && pair[1] < 4 * pair[0], "{steps:?}");
        }
        assert!(steps[3] < 10_000, "{steps:?}");
    }
}

#[test]
fn independent_map_parts_still_share_the_number_of_items() {
    let body = json!({"kind":"seq","name":"body","state":items_state(),"promotedStatePaths":[],
        "children":[mapped_reviewer("first"), mapped_reviewer("second")]});
    // Every item makes `first` report a verdict or an error. `second` failing while `first`
    // reported nothing would take an item that only one of them saw.
    let silent_first = json!({"kind":"not","guard":{"kind":"any","guards":[
        item_count("first", Some("verdict"), 1), item_count("first", None, 1)
    ]}});
    let impossible = mapped_check(
        body.clone(),
        json!({"kind":"all","guards":[silent_first, item_count("second", None, 1)]}),
    );
    let mut analyzer = Analyzer::new(&impossible);
    assert!(analyzer.analyze().is_none());
    assert!(
        analyzer.diagnostics.iter().any(|diagnostic| {
            diagnostic.message == "choice branch is unreachable after excluding earlier branches"
        }),
        "{:?}",
        analyzer.diagnostics
    );

    // Folding the two apart must not rule out what can happen: two items, one failing `second`.
    let two_items = json!({"kind":"all","guards":[
        item_count("first", Some("verdict"), 2), item_count("second", None, 1)
    ]});
    search_steps(&mapped_check(body, two_items));
}

#[test]
fn item_conditions_that_an_outer_control_tells_apart_share_one_item_count() {
    // `second` runs only for an accepted gate, so the gate leaves two conditions for one item.
    // `first` reports on every item and is read with a threshold of two, so both conditions
    // count items.
    let item = json!({"kind":"seq","name":"body","state":items_state(),"promotedStatePaths":[],
    "children":[mapped_reviewer("first"), {
        "kind":"choice","name":"route","state":items_state(),"promotedStatePaths":[],
        "branches":[{"when":{"kind":"in","labels":["accepted"],
            "value":{"name":"gate","source":"signal","field":"verdict"}},
            "node":mapped_reviewer("second")}],
        "otherwise":{"kind":"fail","name":"stopped","reason":"stopped"}
    }]});
    let gate = |label: &str| {
        json!({"kind":"in","labels":[label],
        "value":{"name":"gate","source":"signal","field":"verdict"}})
    };
    let first_reported = json!({"kind":"any","guards":[
        item_count("first", Some("verdict"), 2), item_count("first", None, 1)
    ]});
    let check = |guard: serde_json::Value| -> GraphSpec {
        let mut graph = serde_json::to_value(mapped_check(item.clone(), guard)).unwrap();
        let children = graph["root"]["children"].as_array_mut().unwrap();
        children.insert(0, mapped_reviewer("gate"));
        serde_json::from_value(graph).unwrap()
    };

    search_steps(&check(json!({"kind":"all","guards":[
        gate("accepted"), first_reported.clone(), item_count("second", None, 1)
    ]})));

    let without_second = check(json!({"kind":"all","guards":[
        gate("rejected"), first_reported, item_count("second", None, 1)
    ]}));
    let mut analyzer = Analyzer::new(&without_second);
    assert!(analyzer.analyze().is_none());
    assert!(
        analyzer.diagnostics.iter().any(|diagnostic| {
            diagnostic.message == "choice branch is unreachable after excluding earlier branches"
        }),
        "{:?}",
        analyzer.diagnostics
    );
}

#[test]
fn a_part_that_counts_many_items_one_by_one_is_folded_with_the_rest() {
    let body = json!({"kind":"seq","name":"body","state":items_state(),"promotedStatePaths":[],
        "children":[mapped_reviewer("first"), mapped_reviewer("second")]});
    // `first` reports on every item, so beside `second` a threshold of 3,000 would take 3,000
    // separate item counts, more than one check may spend. Folding the item whole, as before
    // parts existed, takes about 50,000 steps; this guards that the fallback is taken.
    let guard = json!({"kind":"any","guards":[
        item_count("first", Some("verdict"), 3_000),
        item_count("first", None, 1),
        item_count("second", None, 1)
    ]});
    let steps = search_steps(&mapped_check(body, guard));
    assert!(steps < 100_000, "{steps}");
}

#[test]
fn thresholds_count_the_conditions_that_hold() {
    use super::{ALWAYS, Diagram, NEVER};
    let mut diagram = Diagram::default();
    let variables = (0..5)
        .map(|_| diagram.variable(2).unwrap())
        .collect::<Vec<_>>();
    let atoms = variables
        .iter()
        .map(|variable| diagram.test(*variable, [NEVER, ALWAYS]).unwrap())
        .collect::<Vec<_>>();
    let both = diagram.and(atoms[1], atoms[3]).unwrap();
    let either = diagram.or(atoms[0], atoms[4]).unwrap();
    // Repeated, constant and overlapping conditions, in an order unlike the variables'.
    let conditions = [
        atoms[2], either, atoms[2], ALWAYS, both, NEVER, atoms[0], atoms[4], atoms[2], atoms[1],
    ];
    for count in 0..=conditions.len() as u64 + 1 {
        let threshold = diagram.at_least(count, &conditions).unwrap();
        for values in 0..1_usize << variables.len() {
            let holds = |mut node| {
                for (index, variable) in variables.iter().enumerate() {
                    node = diagram.follow(node, *variable, (values >> index) & 1);
                }
                node == ALWAYS
            };
            let holding = conditions
                .iter()
                .filter(|condition| holds(**condition))
                .count();
            assert_eq!(
                holds(threshold),
                holding as u64 >= count,
                "{count} of {values:05b}"
            );
        }
    }
}
