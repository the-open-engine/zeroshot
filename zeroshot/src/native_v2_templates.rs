//! Typed graph construction behind the native-v2 built-in template catalog.

use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    ChoiceBranch, ChoiceNode, ControlSelector, ControlSource, EnumLabel, FieldName, FieldPath,
    GraphNode, GraphSpec, Guard, Join, LoopNode, NodeInstructions, NodeName, NonEmptyEnumSet,
    NonEmptyVec, ParNode, PayloadType, PositiveInteger, StepNode, SucceedNode, VerifierNode,
    WorkerErrorCode, WorkerRef, WriteBinding,
};

use crate::native_v2_admission::MAX_AGENT_VERIFIER_ATTEMPTS;
use crate::native_v2_contract::{GIT_DELIVERY_MERGE_V2_WORKER_REF, GIT_DELIVERY_PR_WORKER_REF};
use crate::native_v2_delivery::contract::{
    delivery_diagnostic_schema, delivery_result_schema, delivery_signal_labels,
};
use crate::native_v2_delivery::{
    DeliveryMode, DELIVERY_CI_FAILED_LABEL, DELIVERY_CONFLICT_LABEL,
    DELIVERY_REPAIR_REQUIRED_LABEL, DELIVERY_SIGNAL_FIELD,
};

#[path = "native_v2_templates/catalog.rs"]
mod catalog;
pub use catalog::{BuiltinGraphTemplate, TemplateDelivery};
pub(crate) use catalog::BuiltinTemplateError;
#[path = "native_v2_templates/values.rs"]
mod values;
use values::*;

#[cfg(test)]
#[path = "native_v2_templates/tests.rs"]
mod tests;

const CHANGE_ITERATIONS: u64 = 10;
const TASK_FIELD: &str = "task";
const ACCEPTANCE_FEEDBACK_FIELD: &str = "acceptanceFeedback";
const CODE_FEEDBACK_FIELD: &str = "codeFeedback";
const DELIVERY_FEEDBACK_FIELD: &str = "deliveryFeedback";
const TITLE_FIELD: &str = "title";
const DESCRIPTION_FIELD: &str = "description";
const ISSUE_NUMBER_FIELD: &str = "issueNumber";
const DIAGNOSTIC_MESSAGE_FIELD: &str = "message";
const VERDICT_FIELD: &str = "verdict";
const ACCEPTED_LABEL: &str = "accepted";
const REJECTED_LABEL: &str = "rejected";
const DELIVERY_NODE: &str = "deliver";

fn single_worker_graph() -> Result<GraphSpec, BuiltinTemplateError> {
    let state = task_type()?;
    let worker = task_worker(
        "builtin.agent.worker@1",
        "Complete the requested task in the shared workspace. Follow repository guidance, make \
         focused changes, and run the relevant checks.",
    )?;
    let route = choice(
        "worker_result",
        state.clone(),
        vec![ChoiceBranch {
            when: executable_error_guard("worker")?,
            node: fail("worker_failed", "worker_failed")?,
        }],
        Some(succeed_null("done")?),
    )?;
    graph(
        state.clone(),
        sequence("run", state, vec![worker, route], Vec::new())?,
    )
}

fn software_change_graph(delivery: TemplateDelivery) -> Result<GraphSpec, BuiltinTemplateError> {
    let state = software_state(delivery)?;
    let worker = task_worker(
        "builtin.agent.software-worker@1",
        "Implement the requested software change fully in the shared workspace. Follow the \
         repository's guidance, keep the change focused, and run relevant checks. Delivery runs \
         `git add --all`; put downloaded tools and other files you do not want committed outside \
         the repository checkout.",
    )?;
    let worker_route = initial_worker_route(state.clone(), delivery)?;
    graph(
        software_input_type(delivery)?,
        sequence("run", state, vec![worker, worker_route], Vec::new())?,
    )
}

fn initial_worker_route(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    choice(
        "worker_result",
        state.clone(),
        vec![ChoiceBranch {
            when: executable_error_guard("worker")?,
            node: fail("worker_failed", "worker_failed")?,
        }],
        Some(change_loop(state, delivery)?),
    )
}

fn task_worker(
    worker: &str,
    authored_instructions: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name("worker")?,
        worker: worker_ref(worker)?,
        instructions: Some(instructions(authored_instructions)?),
        input: task_type()?,
        output: PayloadType::Null,
        input_bindings: vec![state_input(TASK_FIELD, TASK_FIELD)?],
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn change_loop(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let parallel = parallel_reviewers(state.clone(), delivery)?;
    let route = review_route(state.clone(), delivery)?;
    let feedback_paths = feedback_paths()?;
    let body = sequence(
        "change_iteration",
        state.clone(),
        vec![parallel, route],
        feedback_paths.clone(),
    )?;
    let loop_node = GraphNode::Loop(LoopNode {
        name: node_name("change_loop")?,
        state: state.clone(),
        body: Box::new(body),
        until: None,
        max_iterations: positive(CHANGE_ITERATIONS)?,
        promoted_state_paths: feedback_paths,
    });
    sequence(
        "changes",
        state,
        vec![
            loop_node,
            fail("change_attempts_exhausted", "change_attempts_exhausted")?,
        ],
        Vec::new(),
    )
}

fn parallel_reviewers(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let delivery_metadata = delivery_mode(delivery).is_some();
    let acceptance = review_verifier(ReviewVerifierSpec {
        name: "acceptance",
        worker: "builtin.agent.acceptance-verifier@1",
        authored_instructions: if delivery_metadata {
            "Verify the change independently against the user's request and observable behavior. \
             Do not edit files. When delivery feedback is present, verify that the repair addresses \
             it. Accept only with concrete evidence; otherwise return actionable feedback. Provide \
             an accurate, informative prospective pull request title and description for the \
             complete change under review regardless of the verdict. Follow the repository's pull \
             request conventions, keep the title concise, and keep the description focused on \
             information useful to reviewers. If mentioning validation, use only results stated \
             directly by command output and do not infer counts. Do not add issue-closing \
             references because delivery owns them."
        } else {
            "Verify the change independently against the user's request and observable behavior. Do \
             not edit files. When delivery feedback is present, verify that the repair addresses it. \
             Accept only with concrete evidence; otherwise return actionable feedback."
        },
        feedback_target: ACCEPTANCE_FEEDBACK_FIELD,
        output: if delivery_metadata {
            change_manifest_type()?
        } else {
            PayloadType::Null
        },
        write_bindings: if delivery_metadata {
            vec![
                output_write("acceptance", TITLE_FIELD, TITLE_FIELD)?,
                output_write("acceptance", DESCRIPTION_FIELD, DESCRIPTION_FIELD)?,
            ]
        } else {
            Vec::new()
        },
    })?;
    let code = review_verifier(ReviewVerifierSpec {
        name: "code",
        worker: "builtin.agent.code-verifier@1",
        authored_instructions: "Review the change independently for correctness, safety, \
             integration, and substantive maintainability. Do not edit files or reject for \
             style-only preferences. When delivery feedback is present, verify that the repair \
             addresses it. Return actionable feedback when rejecting.",
        feedback_target: CODE_FEEDBACK_FIELD,
        output: PayloadType::Null,
        write_bindings: Vec::new(),
    })?;
    Ok(GraphNode::Par(ParNode {
        name: node_name("parallel_reviews")?,
        state,
        branches: non_empty(vec![acceptance, code])?,
        promoted_state_paths: review_promoted_paths(delivery)?,
        join: Join::All {},
    }))
}

struct ReviewVerifierSpec<'a> {
    name: &'a str,
    worker: &'a str,
    authored_instructions: &'a str,
    feedback_target: &'a str,
    output: PayloadType,
    write_bindings: Vec<WriteBinding>,
}

fn review_verifier(spec: ReviewVerifierSpec<'_>) -> Result<GraphNode, BuiltinTemplateError> {
    let signals = review_signals()?;
    let mut write_bindings = spec.write_bindings;
    write_bindings.push(diagnostic_write(spec.name, spec.feedback_target)?);
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(spec.name)?,
        worker: worker_ref(spec.worker)?,
        input: review_input_type()?,
        output: spec.output,
        input_bindings: vec![
            state_input(TASK_FIELD, TASK_FIELD)?,
            state_input(DELIVERY_FEEDBACK_FIELD, DELIVERY_FEEDBACK_FIELD)?,
        ],
        write_bindings,
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals,
        diagnostic: diagnostic_type()?,
        instructions: Some(instructions(spec.authored_instructions)?),
    }))
}

fn review_route(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("review_result")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: any_executable_error_guard(&["acceptance", "code"])?,
                node: fail("review_failed", "review_failed")?,
            },
            ChoiceBranch {
                when: accepted_reviews_guard()?,
                node: accepted_change(state, delivery)?,
            },
        ])?,
        otherwise: Some(Box::new(review_repair()?)),
        promoted_state_paths: vec![field_path(DELIVERY_FEEDBACK_FIELD)?],
    }))
}

fn review_repair() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name("review_repair")?,
        worker: worker_ref("builtin.agent.review-repair@1")?,
        instructions: Some(instructions(
            "Address both verifier diagnostics in the shared workspace without weakening the \
             requested behavior. Account for any delivery feedback and run the relevant checks \
             before returning. Delivery runs `git add --all`; put downloaded tools and other files \
             you do not want committed outside the repository checkout.",
        )?),
        input: review_repair_input_type()?,
        output: PayloadType::Null,
        input_bindings: vec![
            state_input(TASK_FIELD, TASK_FIELD)?,
            state_input(ACCEPTANCE_FEEDBACK_FIELD, ACCEPTANCE_FEEDBACK_FIELD)?,
            state_input(CODE_FEEDBACK_FIELD, CODE_FEEDBACK_FIELD)?,
            state_input(DELIVERY_FEEDBACK_FIELD, DELIVERY_FEEDBACK_FIELD)?,
        ],
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn accepted_change(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    match delivery {
        TemplateDelivery::None => succeed_null("done"),
        TemplateDelivery::PullRequest => pull_request_delivery(state),
        TemplateDelivery::Merge => merge_delivery(state),
    }
}

fn pull_request_delivery(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    delivery_with_repair(state, DeliveryMode::PullRequest, "pull_request_delivery")
}

fn merge_delivery(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    delivery_with_repair(state, DeliveryMode::Merge, "merge_delivery")
}

fn delivery_with_repair(
    state: PayloadType,
    mode: DeliveryMode,
    name: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    let labels = match mode {
        DeliveryMode::PullRequest => vec![DELIVERY_REPAIR_REQUIRED_LABEL],
        DeliveryMode::MergeV1 | DeliveryMode::Merge => vec![
            DELIVERY_CI_FAILED_LABEL,
            DELIVERY_CONFLICT_LABEL,
            DELIVERY_REPAIR_REQUIRED_LABEL,
        ],
    };
    let route = choice(
        "delivery_result",
        state.clone(),
        vec![
            ChoiceBranch {
                when: executable_error_guard(DELIVERY_NODE)?,
                node: fail("delivery_failed", "delivery_failed")?,
            },
            ChoiceBranch {
                when: delivery_signal_guard(&labels)?,
                node: delivery_repair(mode)?,
            },
        ],
        Some(delivery_success("done", mode)?),
    )?;
    sequence(
        name,
        state,
        vec![delivery_node(mode)?, route],
        vec![field_path(DELIVERY_FEEDBACK_FIELD)?],
    )
}

fn delivery_repair(mode: DeliveryMode) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name("delivery_repair")?,
        worker: worker_ref("builtin.agent.delivery-repair@1")?,
        instructions: Some(instructions(
            "Diagnose the reported Git, delivery, CI failure, or merge conflict using the original \
             diagnostics. Repair the shared workspace when needed and run relevant checks, \
             preserving the requested behavior and verifier-approved change. Delivery will retry \
             with the updated workspace. Delivery runs `git add --all`; put downloaded tools and \
             other files you do not want committed outside the repository checkout.",
        )?),
        input: delivery_repair_input_type(mode)?,
        output: PayloadType::Null,
        input_bindings: vec![
            state_input(TASK_FIELD, TASK_FIELD)?,
            state_input("outcome", "outcome")?,
            state_input(DELIVERY_FEEDBACK_FIELD, DELIVERY_FEEDBACK_FIELD)?,
        ],
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn delivery_node(mode: DeliveryMode) -> Result<GraphNode, BuiltinTemplateError> {
    let output = static_value(delivery_result_schema(mode))?;
    let write_bindings = delivery_write_bindings(&output)?;
    let signals = delivery_signals(mode)?;
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(DELIVERY_NODE)?,
        worker: worker_ref(delivery_worker(mode))?,
        input: delivery_input_type()?,
        output,
        input_bindings: vec![
            state_input(TITLE_FIELD, TITLE_FIELD)?,
            state_input(DESCRIPTION_FIELD, DESCRIPTION_FIELD)?,
            state_input(ISSUE_NUMBER_FIELD, ISSUE_NUMBER_FIELD)?,
        ],
        write_bindings,
        timeout_ms: None,
        attempts: positive(1)?,
        signals,
        diagnostic: static_value(delivery_diagnostic_schema())?,
        instructions: None,
    }))
}

fn delivery_write_bindings(
    output: &PayloadType,
) -> Result<Vec<WriteBinding>, BuiltinTemplateError> {
    output_fields(output)?
        .into_iter()
        .map(|field| output_write(DELIVERY_NODE, &field, &field))
        .chain(std::iter::once(diagnostic_write(
            DELIVERY_NODE,
            DELIVERY_FEEDBACK_FIELD,
        )))
        .collect()
}

fn delivery_signals(
    mode: DeliveryMode,
) -> Result<BTreeMap<FieldName, NonEmptyEnumSet>, BuiltinTemplateError> {
    Ok(BTreeMap::from([(
        field_name(DELIVERY_SIGNAL_FIELD)?,
        static_value(delivery_signal_labels(mode))?,
    )]))
}

fn delivery_success(name: &str, mode: DeliveryMode) -> Result<GraphNode, BuiltinTemplateError> {
    let output = static_value(delivery_result_schema(mode))?;
    let bindings = output_fields(&output)?
        .into_iter()
        .map(|field| state_input(&field, &field))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GraphNode::Succeed(SucceedNode {
        name: node_name(name)?,
        output,
        bindings,
    }))
}

fn accepted_reviews_guard() -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::All {
        guards: non_empty(vec![
            signal_guard("acceptance", VERDICT_FIELD, &[ACCEPTED_LABEL])?,
            signal_guard("code", VERDICT_FIELD, &[ACCEPTED_LABEL])?,
        ])?,
    })
}

fn delivery_signal_guard(labels: &[&str]) -> Result<Guard, BuiltinTemplateError> {
    signal_guard(DELIVERY_NODE, DELIVERY_SIGNAL_FIELD, labels)
}

fn signal_guard(node: &str, field: &str, labels: &[&str]) -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::In {
        value: ControlSelector {
            name: node_name(node)?,
            source: ControlSource::Signal,
            field: Some(field_name(field)?),
        },
        labels: enum_labels(labels)?,
    })
}

fn executable_error_guard(node: &str) -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::In {
        value: ControlSelector {
            name: node_name(node)?,
            source: ControlSource::Error,
            field: None,
        },
        labels: enum_labels(&[
            WorkerErrorCode::Timeout.as_str(),
            WorkerErrorCode::Crash.as_str(),
            WorkerErrorCode::Malformed.as_str(),
            WorkerErrorCode::Refusal.as_str(),
        ])?,
    })
}

fn any_executable_error_guard(nodes: &[&str]) -> Result<Guard, BuiltinTemplateError> {
    let guards = nodes
        .iter()
        .map(|name| executable_error_guard(name))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Guard::Any {
        guards: non_empty(guards)?,
    })
}

fn feedback_paths() -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    let mut paths = review_feedback_paths()?;
    paths.push(field_path(DELIVERY_FEEDBACK_FIELD)?);
    Ok(paths)
}

fn review_feedback_paths() -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    Ok(vec![
        field_path(ACCEPTANCE_FEEDBACK_FIELD)?,
        field_path(CODE_FEEDBACK_FIELD)?,
    ])
}

fn review_promoted_paths(
    delivery: TemplateDelivery,
) -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    let mut paths = review_feedback_paths()?;
    if delivery_mode(delivery).is_some() {
        paths.push(field_path(TITLE_FIELD)?);
        paths.push(field_path(DESCRIPTION_FIELD)?);
    }
    Ok(paths)
}

fn review_signals() -> Result<BTreeMap<FieldName, NonEmptyEnumSet>, BuiltinTemplateError> {
    Ok(BTreeMap::from([(
        field_name(VERDICT_FIELD)?,
        verdict_labels()?,
    )]))
}

fn verdict_labels() -> Result<NonEmptyEnumSet, BuiltinTemplateError> {
    enum_labels(&[ACCEPTED_LABEL, REJECTED_LABEL])
}

fn delivery_mode(delivery: TemplateDelivery) -> Option<DeliveryMode> {
    match delivery {
        TemplateDelivery::None => None,
        TemplateDelivery::PullRequest => Some(DeliveryMode::PullRequest),
        TemplateDelivery::Merge => Some(DeliveryMode::Merge),
    }
}

fn delivery_worker(mode: DeliveryMode) -> &'static str {
    match mode {
        DeliveryMode::PullRequest => GIT_DELIVERY_PR_WORKER_REF,
        DeliveryMode::Merge => GIT_DELIVERY_MERGE_V2_WORKER_REF,
        DeliveryMode::MergeV1 => unreachable!("built-in templates never author merge@1"),
    }
}

fn node_name(value: &str) -> Result<NodeName, BuiltinTemplateError> {
    static_value(NodeName::new(value))
}

fn field_name(value: &str) -> Result<FieldName, BuiltinTemplateError> {
    static_value(FieldName::new(value))
}

fn enum_label(value: &str) -> Result<EnumLabel, BuiltinTemplateError> {
    static_value(EnumLabel::new(value))
}

fn field_path(value: &str) -> Result<FieldPath, BuiltinTemplateError> {
    static_value(FieldPath::new(vec![field_name(value)?]))
}

fn worker_ref(value: &str) -> Result<WorkerRef, BuiltinTemplateError> {
    static_value(WorkerRef::new(value))
}

fn instructions(value: &str) -> Result<NodeInstructions, BuiltinTemplateError> {
    static_value(NodeInstructions::new(value))
}

fn positive(value: u64) -> Result<PositiveInteger, BuiltinTemplateError> {
    static_value(PositiveInteger::new(value))
}

fn enum_labels(values: &[&str]) -> Result<NonEmptyEnumSet, BuiltinTemplateError> {
    let labels = values
        .iter()
        .map(|value| enum_label(value))
        .collect::<Result<Vec<_>, _>>()?;
    static_value(NonEmptyEnumSet::new(labels))
}

fn non_empty<T>(values: Vec<T>) -> Result<NonEmptyVec<T>, BuiltinTemplateError> {
    static_value(NonEmptyVec::new(values))
}

fn static_value<T, E>(value: Result<T, E>) -> Result<T, BuiltinTemplateError> {
    value.map_err(|_| BuiltinTemplateError::InvalidStaticContract)
}
