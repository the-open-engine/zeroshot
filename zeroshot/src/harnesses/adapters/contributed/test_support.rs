//! Shared private invocation setup for contributed adapter process fixtures.

use std::collections::BTreeMap;
use std::sync::Arc;

use openengine_cluster_protocol::{DeclaredConnections, DeclaredEnvironment, PayloadType};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use crate::execution::SessionScope;
use crate::native_v2_contract::NodeInstanceId;
use crate::native_v2_candidate::test_support::{NodeRequestFixture, environment_name};
use crate::native_v2_runner::{DriverInvocation, NodeResponseContract, NodeRole, NodeSession};

pub(super) fn invocation(
    harness: &str,
    fields: &[(&str, &str)],
    session: Arc<dyn NodeSession>,
) -> DriverInvocation {
    let environment = fields
        .iter()
        .map(|(name, value)| (environment_name(name), (*value).to_owned()))
        .collect::<BTreeMap<_, _>>();
    let connections = if environment.is_empty() {
        DeclaredConnections::empty()
    } else {
        DeclaredConnections::single(
            "provider",
            DeclaredEnvironment::new(fields.iter().map(|(name, _)| environment_name(name)))
                .assert_value(),
        )
        .assert_value()
    };
    let binding = serde_json::from_value(json!({
        "kind":"agent", "model":"opaque/model", "sessionScope":SessionScope::NodeInstance, "connections":connections
    })).assert_value();
    let request = NodeRequestFixture {
        run_id: &format!("run-{harness}"),
        node: "agent",
        node_instance: 1,
        execution: 1,
        worker: &format!("agent.{harness}@1"),
        instructions: "Exercise the adapter.",
        input: Value::Null,
        binding,
        environment,
    }
    .into_request();
    DriverInvocation {
        host_context: None,
        node: request.invocation,
        role: NodeRole::Worker,
        response: NodeResponseContract::Worker {
            output: PayloadType::Null,
        },
        environment: request.environment,
        session,
        provider_session_slot: NodeInstanceId::new(1).assert_value(),
    }
}
