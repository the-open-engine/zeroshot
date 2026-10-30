use std::collections::{BTreeMap, BTreeSet};

use openengine_cluster_protocol::{
    GraphNode, GraphProfile, GraphSpec, NodeInstructions, NodeName, VerifierContract,
    WorkerContract, WorkerRef, PayloadType, MAX_DECLARED_ENVIRONMENT_NAMES, RUNTIME_WORKER_ERRORS,
};

use super::{DeliveryPolicy, NativeV2AdmissionError, RuntimePlan, MAX_AGENT_VERIFIER_ATTEMPTS};
use crate::native_v2_contract::{
    AdmittedRun, NodeRuntimeBinding, PullRequestFeedback, GIT_DELIVERY_MERGE_V2_WORKER_REF,
    GIT_DELIVERY_MERGE_V3_WORKER_REF, GIT_DELIVERY_MERGE_WORKER_REF, GIT_DELIVERY_PR_V2_WORKER_REF,
    GIT_DELIVERY_PR_WORKER_REF, GIT_DELIVERY_PUSH_WORKER_REF,
};
use crate::native_v2_delivery::{validate_delivery_contract, DeliveryMode};
use crate::native_v2_comment_delivery::{
    COMMENT_DELIVERY_WORKER_REF, comment_input_schema, validate_comment_contract,
};
use crate::native_v2_runner::NodeResponseContract;

pub(super) fn validate_graph_input(
    graph: &GraphSpec,
    initial_input: &serde_json::Value,
) -> Result<(), NativeV2AdmissionError> {
    if graph.profile != GraphProfile::Full {
        return Err(NativeV2AdmissionError::UnsupportedGraphProfile);
    }
    graph.initial_input.validate_value(initial_input)?;
    Ok(())
}

pub(super) fn validate_executable_bindings(
    declarations: &[ExecutableDeclaration],
    runtime: &RuntimePlan,
    context: super::NativeV2AdmissionContext<'_>,
) -> Result<(), NativeV2AdmissionError> {
    context
        .harnesses
        .validate_runtime(runtime)
        .map_err(|error| NativeV2AdmissionError::InvalidRuntime(error.to_string()))?;
    runtime
        .active_memory()
        .validate()
        .map_err(|error| NativeV2AdmissionError::InvalidEnvironment(error.to_string()))?;
    runtime
        .global_agents()
        .postmortem
        .validate()
        .map_err(|error| NativeV2AdmissionError::InvalidEnvironment(error.to_string()))?;
    let bindings = runtime.nodes();
    if let Some(node) = &runtime.global_agents().postmortem.source_node {
        if !matches!(bindings.get(node), Some(NodeRuntimeBinding::Agent { .. })) {
            return Err(NativeV2AdmissionError::MissingRuntimeBinding { node: node.clone() });
        }
    }
    validate_attempts(declarations)?;
    validate_binding_coverage(declarations, bindings)?;
    validate_binding_kinds(declarations, bindings)?;
    validate_delivery_policy(context.delivery_policy, declarations)?;
    validate_declared_environment(runtime, None)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeafKind {
    Step,
    Verifier,
}

#[derive(Clone, Debug)]
pub(super) struct ExecutableDeclaration {
    pub(super) name: NodeName,
    pub(super) worker: WorkerRef,
    pub(super) instructions: Option<NodeInstructions>,
    pub(super) contract: WorkerContract,
    attempts: u64,
    kind: LeafKind,
}

pub(super) fn executable_declarations(root: &GraphNode) -> Vec<ExecutableDeclaration> {
    let mut declarations = Vec::new();
    collect_declarations(root, &mut declarations);
    declarations
}

pub(super) fn executable_runtime_roles(root: &GraphNode) -> Vec<(NodeName, bool)> {
    executable_declarations(root)
        .into_iter()
        .map(|declaration| {
            let delivery = is_trusted_delivery_worker(&declaration.worker);
            (declaration.name, delivery)
        })
        .collect()
}

fn collect_declarations(node: &GraphNode, declarations: &mut Vec<ExecutableDeclaration>) {
    match node {
        GraphNode::Step(step) => declarations.push(ExecutableDeclaration {
            name: step.name.clone(),
            worker: step.worker.clone(),
            instructions: step.instructions.clone(),
            contract: WorkerContract {
                input: step.input.clone(),
                output: step.output.clone(),
                verifier: None,
                errors: RUNTIME_WORKER_ERRORS.to_vec(),
            },
            attempts: step.attempts.get(),
            kind: LeafKind::Step,
        }),
        GraphNode::Verifier(verifier) => declarations.push(ExecutableDeclaration {
            name: verifier.name.clone(),
            worker: verifier.worker.clone(),
            instructions: verifier.instructions.clone(),
            contract: WorkerContract {
                input: verifier.input.clone(),
                output: verifier.output.clone(),
                verifier: Some(VerifierContract {
                    signals: verifier.signals.clone(),
                    diagnostic: verifier.diagnostic.clone(),
                }),
                errors: RUNTIME_WORKER_ERRORS.to_vec(),
            },
            attempts: verifier.attempts.get(),
            kind: LeafKind::Verifier,
        }),
        GraphNode::Seq(group) => group
            .children
            .as_slice()
            .iter()
            .for_each(|child| collect_declarations(child, declarations)),
        GraphNode::Choice(group) => {
            group
                .branches
                .as_slice()
                .iter()
                .for_each(|branch| collect_declarations(&branch.node, declarations));
            if let Some(otherwise) = &group.otherwise {
                collect_declarations(otherwise, declarations);
            }
        }
        GraphNode::Par(group) => group
            .branches
            .as_slice()
            .iter()
            .for_each(|branch| collect_declarations(branch, declarations)),
        GraphNode::Loop(group) => collect_declarations(&group.body, declarations),
        GraphNode::Map(group) => collect_declarations(&group.body, declarations),
        GraphNode::Succeed(_) | GraphNode::Fail(_) => {}
    }
}

fn validate_attempts(declarations: &[ExecutableDeclaration]) -> Result<(), NativeV2AdmissionError> {
    if let Some(declaration) = declarations.iter().find(|item| {
        item.attempts != 1
            && !(item.kind == LeafKind::Verifier
                && !is_git_delivery_worker(&item.worker)
                && item.attempts <= MAX_AGENT_VERIFIER_ATTEMPTS)
    }) {
        return Err(NativeV2AdmissionError::Attempts {
            node: declaration.name.clone(),
            attempts: declaration.attempts,
        });
    }
    Ok(())
}

fn validate_binding_coverage(
    declarations: &[ExecutableDeclaration],
    bindings: &BTreeMap<NodeName, NodeRuntimeBinding>,
) -> Result<(), NativeV2AdmissionError> {
    let executable = declarations
        .iter()
        .map(|declaration| declaration.name.clone())
        .collect::<BTreeSet<_>>();
    let bound = bindings.keys().cloned().collect::<BTreeSet<_>>();
    if let Some(node) = executable.difference(&bound).next() {
        return Err(NativeV2AdmissionError::MissingRuntimeBinding { node: node.clone() });
    }
    if let Some(node) = bindings.keys().find(|node| !executable.contains(*node)) {
        return Err(NativeV2AdmissionError::UnexpectedRuntimeBinding { node: node.clone() });
    }
    Ok(())
}

fn validate_binding_kinds(
    declarations: &[ExecutableDeclaration],
    bindings: &BTreeMap<NodeName, NodeRuntimeBinding>,
) -> Result<(), NativeV2AdmissionError> {
    for declaration in declarations {
        let binding = &bindings[&declaration.name];
        binding.validate_model_limits().map_err(|error| {
            NativeV2AdmissionError::InvalidRuntimeBinding {
                node: declaration.name.clone(),
                detail: error.to_string(),
            }
        })?;
        if let NodeRuntimeBinding::Decision { .. } = binding {
            validate_decision_declaration(declaration, binding)?;
        }
        validate_binding_kind(declaration, binding)?;
        validate_instructions(declaration, binding)?;
    }
    Ok(())
}

fn validate_binding_kind(
    declaration: &ExecutableDeclaration,
    binding: &NodeRuntimeBinding,
) -> Result<(), NativeV2AdmissionError> {
    if declaration.kind == LeafKind::Step
        && matches!(
            binding,
            NodeRuntimeBinding::GitDelivery { .. } | NodeRuntimeBinding::CommentDelivery { .. }
        )
    {
        return Err(NativeV2AdmissionError::DeliveryMustBeVerifier {
            node: declaration.name.clone(),
        });
    }
    if declaration.worker.as_str() == COMMENT_DELIVERY_WORKER_REF {
        if !matches!(binding, NodeRuntimeBinding::CommentDelivery { .. }) {
            return Err(NativeV2AdmissionError::DeliveryWorkerRequiresBinding {
                node: declaration.name.clone(),
                worker: declaration.worker.clone(),
            });
        }
        return validate_comment_declaration(declaration);
    }
    if matches!(binding, NodeRuntimeBinding::CommentDelivery { .. }) {
        return Err(NativeV2AdmissionError::UnsupportedDeliveryWorker {
            node: declaration.name.clone(),
            worker: declaration.worker.clone(),
        });
    }
    match (is_git_delivery_worker(&declaration.worker), binding) {
        (true, NodeRuntimeBinding::GitDelivery { .. }) => {
            validate_delivery_declaration(declaration, binding)
        }
        (true, _) => Err(NativeV2AdmissionError::DeliveryWorkerRequiresBinding {
            node: declaration.name.clone(),
            worker: declaration.worker.clone(),
        }),
        (false, NodeRuntimeBinding::GitDelivery { .. }) => {
            Err(NativeV2AdmissionError::UnsupportedDeliveryWorker {
                node: declaration.name.clone(),
                worker: declaration.worker.clone(),
            })
        }
        _ => Ok(()),
    }
}

fn validate_instructions(
    declaration: &ExecutableDeclaration,
    binding: &NodeRuntimeBinding,
) -> Result<(), NativeV2AdmissionError> {
    match (binding, &declaration.instructions) {
        (NodeRuntimeBinding::Agent { .. } | NodeRuntimeBinding::Decision { .. }, None) => {
            Err(NativeV2AdmissionError::MissingAgentInstructions {
                node: declaration.name.clone(),
            })
        }
        (
            NodeRuntimeBinding::GitDelivery { .. } | NodeRuntimeBinding::CommentDelivery { .. },
            Some(_),
        ) => Err(NativeV2AdmissionError::DeliveryInstructionsForbidden {
            node: declaration.name.clone(),
        }),
        _ => Ok(()),
    }
}

fn validate_delivery_policy(
    policy: DeliveryPolicy,
    declarations: &[ExecutableDeclaration],
) -> Result<(), NativeV2AdmissionError> {
    let found = declarations
        .iter()
        .filter(|declaration| is_trusted_delivery_worker(&declaration.worker))
        .count();
    let accepted = match policy {
        DeliveryPolicy::Optional => found <= 1,
        DeliveryPolicy::Required => found == 1,
    };
    if !accepted {
        return Err(NativeV2AdmissionError::DeliveryNodeCount { policy, found });
    }
    Ok(())
}

fn is_trusted_delivery_worker(worker: &WorkerRef) -> bool {
    worker.as_str() == COMMENT_DELIVERY_WORKER_REF || is_git_delivery_worker(worker)
}

fn is_git_delivery_worker(worker: &WorkerRef) -> bool {
    matches!(
        worker.as_str(),
        GIT_DELIVERY_PUSH_WORKER_REF
            | GIT_DELIVERY_PR_WORKER_REF
            | GIT_DELIVERY_PR_V2_WORKER_REF
            | GIT_DELIVERY_MERGE_WORKER_REF
            | GIT_DELIVERY_MERGE_V2_WORKER_REF
            | GIT_DELIVERY_MERGE_V3_WORKER_REF
    )
}

pub(crate) fn sole_delivery_node(admitted: &AdmittedRun) -> Option<(NodeName, DeliveryMode)> {
    let mut deliveries = executable_declarations(&admitted.graph.root)
        .into_iter()
        .filter_map(|declaration| {
            DeliveryMode::from_worker(&declaration.worker).map(|mode| (declaration.name, mode))
        });
    let selected = deliveries.next()?;
    deliveries.next().is_none().then_some(selected)
}

pub(crate) fn writer_nodes(admitted: &AdmittedRun) -> BTreeSet<NodeName> {
    executable_declarations(&admitted.graph.root)
        .into_iter()
        .map(|declaration| declaration.name)
        .collect()
}

fn validate_delivery_declaration(
    declaration: &ExecutableDeclaration,
    binding: &NodeRuntimeBinding,
) -> Result<(), NativeV2AdmissionError> {
    let Some(mode) = DeliveryMode::from_worker(&declaration.worker) else {
        return Ok(());
    };
    if matches!(
        binding,
        NodeRuntimeBinding::GitDelivery {
            pull_request_feedback: PullRequestFeedback::Ignore,
            ..
        }
    ) && !mode.considers_feedback()
    {
        return Err(NativeV2AdmissionError::InvalidDeliveryContract {
            node: declaration.name.clone(),
            worker: declaration.worker.clone(),
        });
    }
    let Some(verifier) = &declaration.contract.verifier else {
        return Err(NativeV2AdmissionError::InvalidDeliveryContract {
            node: declaration.name.clone(),
            worker: declaration.worker.clone(),
        });
    };
    let response = NodeResponseContract::Verifier {
        output: declaration.contract.output.clone(),
        signals: verifier.signals.clone(),
        diagnostic: verifier.diagnostic.clone(),
    };
    validate_delivery_contract(mode, &response).map_err(|_| {
        NativeV2AdmissionError::InvalidDeliveryContract {
            node: declaration.name.clone(),
            worker: declaration.worker.clone(),
        }
    })
}

pub(super) fn validate_declared_environment(
    runtime: &RuntimePlan,
    environment: Option<&openengine_cluster_protocol::RuntimeEnvironment>,
) -> Result<(), NativeV2AdmissionError> {
    let mut declared = runtime
        .nodes()
        .values()
        .flat_map(|binding| binding.declared_connections().environment_names())
        .collect::<BTreeSet<_>>();
    if let Some(environment) = environment {
        declared.extend(environment.connections.environment_names());
        declared.extend(environment.variables.keys());
    }
    if declared.len() > MAX_DECLARED_ENVIRONMENT_NAMES {
        return Err(NativeV2AdmissionError::DeclaredEnvironmentTooLarge {
            found: declared.len(),
        });
    }
    Ok(())
}

fn validate_comment_declaration(
    declaration: &ExecutableDeclaration,
) -> Result<(), NativeV2AdmissionError> {
    if comment_input_schema().ok().as_ref() != Some(&declaration.contract.input) {
        return Err(NativeV2AdmissionError::InvalidDeliveryContract {
            node: declaration.name.clone(),
            worker: declaration.worker.clone(),
        });
    }
    let verifier = declaration.contract.verifier.as_ref().ok_or_else(|| {
        NativeV2AdmissionError::InvalidDeliveryContract {
            node: declaration.name.clone(),
            worker: declaration.worker.clone(),
        }
    })?;
    let response = NodeResponseContract::Verifier {
        output: declaration.contract.output.clone(),
        signals: verifier.signals.clone(),
        diagnostic: verifier.diagnostic.clone(),
    };
    validate_comment_contract(&response).map_err(|_| {
        NativeV2AdmissionError::InvalidDeliveryContract {
            node: declaration.name.clone(),
            worker: declaration.worker.clone(),
        }
    })
}

fn validate_decision_declaration(
    declaration: &ExecutableDeclaration,
    binding: &NodeRuntimeBinding,
) -> Result<(), NativeV2AdmissionError> {
    let invalid = |detail: &str| NativeV2AdmissionError::InvalidRuntimeBinding {
        node: declaration.name.clone(),
        detail: detail.to_owned(),
    };
    let Some(verifier) = &declaration.contract.verifier else {
        return Err(invalid("decision bindings require a verifier node"));
    };
    if declaration.attempts != 1 {
        return Err(invalid("decision verifiers require attempts: 1"));
    }
    if !matches!(
        declaration.contract.input,
        PayloadType::Record { .. } | PayloadType::String | PayloadType::Array { .. }
    ) {
        return Err(invalid("decision state must be a string, record or array"));
    }
    crate::native_v2_candidate::decision::contract::validate_contract(
        &NodeResponseContract::Verifier {
            output: declaration.contract.output.clone(),
            signals: verifier.signals.clone(),
            diagnostic: verifier.diagnostic.clone(),
        },
        binding,
    )
    .map_err(|error| invalid(&error.to_string()))?;
    Ok(())
}
