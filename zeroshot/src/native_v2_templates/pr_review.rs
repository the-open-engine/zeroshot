//! A bounded reproduction decision, optional full-stack check, and final code review.
use super::*;

const CLASSIFIER: &str = "classifier";
const REPRODUCER: &str = "reproducer";
const CHECKER: &str = "code_checker";
const TASKS: &str = "reproductionTasks";
const REASON: &str = "reproductionReason";
const EVIDENCE: &str = "reproductionEvidence";

fn task_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![("plan", PayloadType::String, true)])
}

fn array(items: PayloadType) -> PayloadType {
    PayloadType::Array {
        items: Box::new(items),
    }
}

fn input_type() -> Result<PayloadType, BuiltinTemplateError> {
    let mut scope = static_value(crate::native_v2_comment_delivery::review_scope_schema())?;
    let PayloadType::Record { fields } = &mut scope else {
        return Err(BuiltinTemplateError::InvalidStaticContract);
    };
    for field in fields.values_mut() {
        field.required = true;
    }
    let issue = record_type(vec![
        ("url", PayloadType::String, true),
        ("title", PayloadType::String, true),
        ("body", PayloadType::String, true),
    ])?;
    let context = record_type(vec![
        ("title", PayloadType::String, true),
        ("body", PayloadType::String, true),
        ("reviewScope", scope, true),
        ("linkedIssues", array(issue), true),
    ])?;
    record_type(vec![
        ("url", PayloadType::String, true),
        ("context", context, true),
        ("instructions", PayloadType::String, false),
    ])
}

fn agent_input(extra: Vec<(&str, PayloadType, bool)>) -> Result<PayloadType, BuiltinTemplateError> {
    let mut fields = record_fields(input_type()?)?;
    fields.insert(field_name("instructions")?, required(PayloadType::String));
    fields.extend(record_fields(record_type(extra)?)?);
    Ok(PayloadType::Record { fields })
}

fn bindings(
    extra: &[&str],
) -> Result<Vec<openengine_cluster_protocol::InputBinding>, BuiltinTemplateError> {
    ["url", "context", "instructions"]
        .into_iter()
        .chain(extra.iter().copied())
        .map(|field| state_input(field, field))
        .collect()
}

fn classifier() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(CLASSIFIER)?,
        worker: worker_ref("builtin.agent.pr-classifier@1")?,
        instructions: Some(instructions(include_str!("pr_review/classifier.md"))?),
        input: agent_input(Vec::new())?,
        output: record_type(vec![
            (TASKS, array(task_type()?), true),
            (REASON, PayloadType::String, true),
        ])?,
        input_bindings: bindings(&[])?,
        write_bindings: vec![
            output_write(CLASSIFIER, TASKS, TASKS)?,
            output_write(CLASSIFIER, REASON, REASON)?,
        ],
        timeout_ms: None,
        attempts: positive(1)?,
        signals: BTreeMap::new(),
        diagnostic: PayloadType::Null,
    }))
}

fn reproducer() -> Result<GraphNode, BuiltinTemplateError> {
    let mut input_bindings = bindings(&[])?;
    input_bindings.push(openengine_cluster_protocol::InputBinding {
        target: field_path("plan")?,
        value: DataSelector::Item {
            path: field_path("plan")?,
        },
    });
    Ok(GraphNode::Step(StepNode {
        name: node_name(REPRODUCER)?,
        worker: worker_ref("builtin.agent.pr-reproducer@1")?,
        instructions: Some(instructions(include_str!("pr_review/reproducer.md"))?),
        input: agent_input(vec![("plan", PayloadType::String, true)])?,
        output: record_type(vec![(EVIDENCE, PayloadType::String, true)])?,
        input_bindings,
        write_bindings: vec![output_write(REPRODUCER, EVIDENCE, EVIDENCE)?],
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn checker() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(CHECKER)?,
        worker: worker_ref("builtin.agent.pr-code-checker@1")?,
        instructions: Some(instructions(include_str!("pr_review/code_checker.md"))?),
        input: agent_input(vec![
            (REASON, PayloadType::String, true),
            (EVIDENCE, array(PayloadType::String), true),
        ])?,
        output: record_type(vec![("body", PayloadType::String, true)])?,
        input_bindings: bindings(&[REASON, EVIDENCE])?,
        write_bindings: vec![output_write(CHECKER, "body", "body")?],
        timeout_ms: None,
        attempts: positive(1)?,
        signals: BTreeMap::new(),
        diagnostic: PayloadType::Null,
    }))
}

fn review_state(
    input: &PayloadType,
    delivery: TemplateDelivery,
) -> Result<(PayloadType, PayloadType), BuiltinTemplateError> {
    let (state, result) = super::review::review_state(input, delivery)?;
    let mut fields = record_fields(state)?;
    for (name, value_type) in [
        (TASKS, array(task_type()?)),
        (REASON, PayloadType::String),
        (EVIDENCE, array(PayloadType::String)),
    ] {
        fields.insert(field_name(name)?, required(value_type));
    }
    Ok((PayloadType::Record { fields }, result))
}

fn after_agent(
    node: &str,
    state: &PayloadType,
    continuation: GraphNode,
) -> Result<GraphNode, BuiltinTemplateError> {
    choice(
        &format!("{node}_result"),
        state.clone(),
        vec![ChoiceBranch {
            when: executable_error_guard(node)?,
            node: fail(&format!("{node}_failed"), &format!("{node}_failed"))?,
        }],
        Some(continuation),
    )
}

fn reproduction(state: &PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("reproduction")?,
        state: state.clone(),
        over: DataSelector::State {
            path: field_path(TASKS)?,
        },
        body: Box::new(reproducer()?),
        max_items: positive(1)?,
        promoted_state_paths: vec![field_path(EVIDENCE)?],
    }))
}

fn reproduction_error() -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::Any {
        guards: non_empty(vec![
            control_guard(
                "reproduction",
                ControlSource::Group,
                Some("overflow"),
                &["overflow"],
            )?,
            Guard::KOfMap {
                count: positive(1)?,
                value: error_selector(REPRODUCER)?,
                labels: worker_error_labels()?,
            },
        ])?,
    })
}

fn review_pipeline(
    state: &PayloadType,
    result: &PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let completed = sequence(
        "review_completed",
        state.clone(),
        super::review::review_continuation(state, result, delivery)?,
        Vec::new(),
    )?;
    let code_review = sequence(
        "code_review",
        state.clone(),
        vec![checker()?, after_agent(CHECKER, state, completed)?],
        Vec::new(),
    )?;
    let route = choice(
        "reproduction_result",
        state.clone(),
        vec![ChoiceBranch {
            when: reproduction_error()?,
            node: fail("reproduction_failed", "reproduction_failed")?,
        }],
        Some(code_review),
    )?;
    sequence(
        "classified",
        state.clone(),
        vec![reproduction(state)?, route],
        Vec::new(),
    )
}

pub(super) fn pr_review_graph(
    delivery: TemplateDelivery,
) -> Result<GraphSpec, BuiltinTemplateError> {
    let input = input_type()?;
    let (state, result) = review_state(&input, delivery)?;
    let pipeline = review_pipeline(&state, &result, delivery)?;
    let route = after_agent(CLASSIFIER, &state, pipeline)?;
    graph(
        input,
        sequence("run", state, vec![classifier()?, route], Vec::new())?,
    )
}

#[cfg(test)]
#[path = "pr_review/tests.rs"]
mod tests;
