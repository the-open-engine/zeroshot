use super::*;
use std::sync::Arc;
use openengine_cluster_protocol::PayloadType;
use openengine_cluster_testkit::assertions::AssertValue;
use crate::native_v2_contract::NodeInstanceId;
use crate::native_v2_runner::{NodeDispatchMetadata, NodeResponseContract};
use crate::native_v2_runner::test_support::{CountingSession, request};

fn invocation() -> DriverInvocation {
    let request = request("own-run", "worker", (17, 29));
    DriverInvocation {
        dispatch: NodeDispatchMetadata {
            attempt: 2,
            iteration: 3,
        },
        host_context: None,
        provider_session_slot: NodeInstanceId::new(999).assert_value(),
        provider_home_slot: NodeInstanceId::new(999).assert_value(),
        auxiliary: false,
        node: request.invocation,
        role: NodeRole::Worker,
        response: NodeResponseContract::Worker {
            output: PayloadType::Null,
        },
        environment: request.environment,
        session: Arc::new(CountingSession {
            closed: Arc::default(),
        }),
    }
}

#[test]
fn execution_labels_replace_stale_values_with_windows_case_folding() {
    for windows in [false, true] {
        let invocation = invocation();
        let mut environment = BTreeMap::from([("UNRELATED".to_owned(), "preserved".to_owned())]);
        for name in NAMES {
            environment.insert(name.to_owned(), "spoofed".to_owned());
            if windows {
                environment.insert(name.to_ascii_lowercase(), "mixed-case spoof".to_owned());
            }
        }
        apply_for_platform(&mut environment, &invocation, windows).assert_value();
        assert_eq!(environment.len(), 8);
        assert_eq!(environment["UNRELATED"], "preserved");
        assert_eq!(environment["ZEROSHOT_RUN_ID"], "own-run");
        assert_eq!(environment["ZEROSHOT_NODE_NAME"], "worker");
        assert_eq!(environment["ZEROSHOT_NODE_KIND"], "step");
        assert_eq!(environment["ZEROSHOT_NODE_INSTANCE_ID"], "17");
        assert_eq!(
            environment["ZEROSHOT_EXECUTION_REF"],
            invocation
                .node
                .reference
                .public_execution()
                .assert_value()
                .as_str()
        );
        assert_eq!(environment["ZEROSHOT_NODE_ATTEMPT"], "2");
        assert_eq!(environment["ZEROSHOT_NODE_ITERATION"], "3");
        assert!(invocation.environment.iter().next().is_none());
    }
}

#[test]
fn auxiliary_verifier_replaces_primary_context_without_reusing_its_session_slot() {
    let mut invocation = invocation();
    let mut environment = BTreeMap::new();
    apply_execution_environment(&mut environment, &invocation).assert_value();
    invocation.node.reference = request("auxiliary-run", "verify", (42, 55))
        .invocation
        .reference;
    invocation.role = NodeRole::Verifier;
    invocation.dispatch = NodeDispatchMetadata::default();
    apply_execution_environment(&mut environment, &invocation).assert_value();
    assert_eq!(environment["ZEROSHOT_RUN_ID"], "auxiliary-run");
    assert_eq!(environment["ZEROSHOT_NODE_NAME"], "verify");
    assert_eq!(environment["ZEROSHOT_NODE_KIND"], "verifier");
    assert_eq!(environment["ZEROSHOT_NODE_INSTANCE_ID"], "42");
    assert_eq!(environment["ZEROSHOT_NODE_ATTEMPT"], "1");
    assert_eq!(environment["ZEROSHOT_NODE_ITERATION"], "0");
    invocation.role = NodeRole::GitDelivery;
    assert_eq!(
        apply_execution_environment(&mut environment, &invocation),
        Err(NodeRunnerError::InvalidRole)
    );
}
