use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    GraphSpec, NodeName, NonEmptyVec, PositiveInteger, StructuralBounds, TerminationWitness,
};
use serde_json::json;

use super::{VerificationError, finalize_verified_with_invariant_probe};

#[test]
fn post_validation_invariant_failure_is_internal() {
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
    }))
    .unwrap();
    let one = PositiveInteger::new(1).unwrap();
    let bounds = StructuralBounds {
        termination: TerminationWitness::Acyclic {
            order: NonEmptyVec::new(vec![NodeName::new("duplicate").unwrap()]).unwrap(),
        },
        max_node_executions: one,
        peak_concurrency: one,
        attempts_per_node: BTreeMap::from([(NodeName::new("duplicate").unwrap(), one)]),
    };

    assert_eq!(
        finalize_verified_with_invariant_probe(&graph, bounds, true),
        Err(VerificationError::Internal(
            "injected post-validation invariant failure".to_owned()
        ))
    );
}
