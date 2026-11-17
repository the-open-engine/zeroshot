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
    let steps = [2, 4, 8, 16].map(|reviewers| {
        let graph = parallel_reviews(reviewers);
        let mut analyzer = Analyzer::new(&graph);
        assert!(analyzer.analyze().is_some(), "{:?}", analyzer.diagnostics);
        analyzer.search_steps
    });
    println!("search steps for 2, 4, 8 and 16 reviewers: {steps:?}");
    // One reviewer has 13 outcomes; listing joint outcomes would multiply by 169 per doubling
    // from two reviewers on. Each doubling here costs a small constant factor.
    for pair in steps.windows(2) {
        assert!(pair[1] > pair[0] && pair[1] < 4 * pair[0], "{steps:?}");
    }
    assert!(steps[3] < 50_000, "{steps:?}");
}
