//! Public process labels owned by one dispatch, never credentials or controller state.
use std::collections::BTreeMap;

use crate::native_v2_runner::{DriverInvocation, NodeRole, NodeRunnerError};

const NAMES: [&str; 7] = [
    "ZEROSHOT_RUN_ID",
    "ZEROSHOT_NODE_NAME",
    "ZEROSHOT_NODE_KIND",
    "ZEROSHOT_NODE_INSTANCE_ID",
    "ZEROSHOT_EXECUTION_REF",
    "ZEROSHOT_NODE_ATTEMPT",
    "ZEROSHOT_NODE_ITERATION",
];

pub(crate) fn is_execution_environment_name(name: &str, windows: bool) -> bool {
    NAMES.iter().any(|owned| {
        if windows {
            owned.eq_ignore_ascii_case(name)
        } else {
            *owned == name
        }
    })
}

/// Apply after native, base and declared environment merges. Never changes the host environment.
pub(crate) fn apply_execution_environment(
    environment: &mut BTreeMap<String, String>,
    invocation: &DriverInvocation,
) -> Result<(), NodeRunnerError> {
    apply_for_platform(environment, invocation, cfg!(windows))
}

fn apply_for_platform(
    environment: &mut BTreeMap<String, String>,
    invocation: &DriverInvocation,
    windows: bool,
) -> Result<(), NodeRunnerError> {
    let kind = match invocation.role {
        NodeRole::Worker => "step",
        NodeRole::Verifier => "verifier",
        NodeRole::GitDelivery | NodeRole::CommentDelivery => {
            return Err(NodeRunnerError::InvalidRole);
        }
    };
    let reference = &invocation.node.reference;
    let execution = reference
        .public_execution()
        .map_err(|_| NodeRunnerError::Driver)?;
    let values = [
        reference.run_id.as_str().to_owned(),
        reference.node.as_str().to_owned(),
        kind.to_owned(),
        reference.node_instance.to_string(),
        execution.as_str().to_owned(),
        invocation.dispatch.attempt.to_string(),
        invocation.dispatch.iteration.to_string(),
    ];
    environment.retain(|name, _| !is_execution_environment_name(name, windows));
    environment.extend(
        NAMES
            .into_iter()
            .zip(values)
            .map(|(name, value)| (name.to_owned(), value)),
    );
    Ok(())
}

#[cfg(test)]
mod tests;
