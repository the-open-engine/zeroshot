use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    GraphSpec, Guard, NodeName, NonEmptyVec, PositiveInteger, StructuralBounds, TerminationWitness,
};
use serde_json::json;

use super::{
    Analyzer, VerificationError, assignment_has_failure, evaluate_guard,
    finalize_verified_with_invariant_probe,
};

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
        let assignments = analyzer.assignments_for_guards(&[&guard], &[]).unwrap();
        let parallel = NodeName::new("parallel")?;
        let failures = assignments
            .iter()
            .filter(|assignment| evaluate_guard(&guard, assignment))
            .map(|assignment| assignment_has_failure(assignment, &parallel))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            failures,
            std::collections::BTreeSet::from([false, true]),
            "{field}"
        );
    }
    Ok(())
}
