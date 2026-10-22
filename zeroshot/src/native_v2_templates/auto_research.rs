//! Five-role autoresearch: persistent scouts/researcher, independent review, optional push.

use openengine_cluster_protocol::InputBinding;

use super::*;
use crate::native_v2_delivery::DELIVERY_PUSHED_LABEL;

const ITERATIONS: u64 = 10;
const REVIEW_PASSES: u64 = 64;
const REPORT: &str = "researchReport";
const FEEDBACK: &str = "reviewFeedback";
const REVIEW: &str = "review";
const VERDICTS: &[&str] = &["accepted", "revise", "stop"];
const REVIEWER: &str = "reviewer";
const RESEARCHER: &str = "researcher";
const SCOUTS: [&str; 3] = ["explorer", "synthesizer", "challenger"];
const ACCEPTED: &[&str] = &["accepted", "stop"];
const SCOUT_PROMPT: &str = include_str!("auto_research/scout.md");
const RESEARCHER_PROMPT: &str = include_str!("auto_research/researcher.md");
const REVIEWER_PROMPT: &str = include_str!("auto_research/reviewer.md");

pub(super) fn auto_research_graph(
    delivery: TemplateDelivery,
) -> Result<GraphSpec, BuiltinTemplateError> {
    if !matches!(delivery, TemplateDelivery::None | TemplateDelivery::Push) {
        return Err(BuiltinTemplateError::UnsupportedDelivery {
            template: "auto-research",
            delivery,
        });
    }
    let state = research_state(delivery)?;
    graph(
        research_input_type()?,
        sequence(
            "run",
            state.clone(),
            vec![
                research_loop(state.clone(), delivery)?,
                terminal(state, delivery)?,
            ],
            Vec::new(),
        )?,
    )
}

fn research_loop(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let body = sequence(
        "research_round",
        state.clone(),
        vec![
            scouts(state.clone())?,
            scout_result(state.clone(), delivery)?,
        ],
        round_paths(delivery)?,
    )?;
    let mut exits = vec![
        // A returned `revise` means the inner review budget was exhausted.
        signal_guard(REVIEWER, VERDICT_FIELD, &["stop", "revise"])?,
        executable_error_guard(REVIEWER)?,
    ];
    if delivery == TemplateDelivery::Push {
        exits.push(executable_error_guard("checkpoint_delivery")?);
        exits.push(named_delivery_signal_guard(
            "checkpoint_delivery",
            &[DELIVERY_REPAIR_REQUIRED_LABEL],
        )?);
    }
    Ok(GraphNode::Loop(LoopNode {
        name: node_name("research_loop")?,
        state,
        body: Box::new(body),
        until: Some(Guard::Any {
            guards: non_empty(exits)?,
        }),
        max_iterations: positive(ITERATIONS)?,
        max_iterations_input: Some(static_value(FieldPath::new(vec![
            field_name("options")?,
            field_name("iterations")?,
        ]))?),
        promoted_state_paths: round_paths(delivery)?,
    }))
}

fn scouts(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let prompts = [
        include_str!("auto_research/explorer.md"),
        include_str!("auto_research/synthesizer.md"),
        include_str!("auto_research/challenger.md"),
    ];
    let branches = SCOUTS
        .into_iter()
        .zip(prompts)
        .map(|(role, prompt)| scout(role, prompt))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GraphNode::Par(ParNode {
        name: node_name("scouts")?,
        state,
        branches: non_empty(branches)?,
        join: Join::All {},
        promoted_state_paths: paths(&[
            "explorerProposal",
            "synthesizerProposal",
            "challengerProposal",
        ])?,
    }))
}

fn scout(role: &str, prompt: &str) -> Result<GraphNode, BuiltinTemplateError> {
    let output = format!("{role}Proposal");
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(role)?,
        worker: worker_ref(&format!("builtin.agent.research-{role}@1"))?,
        instructions: Some(instructions(&format!("{SCOUT_PROMPT}\n\n{prompt}"))?),
        input: research_input(&[])?,
        output: string_record(&[&output])?,
        input_bindings: research_bindings(&[])?,
        write_bindings: vec![output_write(role, &output, &output)?],
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: BTreeMap::new(),
        diagnostic: diagnostic_type()?,
    }))
}

fn scout_result(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let mut children = vec![work_review(state.clone(), delivery)?];
    if delivery == TemplateDelivery::Push {
        children.push(publish(state.clone())?);
    }
    let mut route = choice(
        "scout_result",
        state.clone(),
        vec![ChoiceBranch {
            when: any_executable_error_guard(&SCOUTS)?,
            node: fail("scout_failed", "research_scout_failed")?,
        }],
        Some(sequence(
            "research_work",
            state,
            children,
            round_paths(delivery)?,
        )?),
    )?;
    if let GraphNode::Choice(group) = &mut route {
        group.promoted_state_paths = round_paths(delivery)?;
    }
    Ok(route)
}

fn work_review(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let mut route = choice(
        "research_result",
        state.clone(),
        vec![ChoiceBranch {
            when: executable_error_guard(RESEARCHER)?,
            node: fail("research_failed", "research_failed")?,
        }],
        Some(reviewer(delivery)?),
    )?;
    if let GraphNode::Choice(group) = &mut route {
        group.promoted_state_paths = review_paths(delivery)?;
    }
    Ok(GraphNode::Loop(LoopNode {
        name: node_name("work_review")?,
        state: state.clone(),
        body: Box::new(sequence(
            "resolution",
            state,
            vec![researcher()?, route],
            research_paths(delivery)?,
        )?),
        until: Some(Guard::Any {
            guards: non_empty(vec![
                signal_guard(REVIEWER, VERDICT_FIELD, ACCEPTED)?,
                executable_error_guard(REVIEWER)?,
            ])?,
        }),
        max_iterations: positive(REVIEW_PASSES)?,
        max_iterations_input: None,
        promoted_state_paths: research_paths(delivery)?,
    }))
}

fn researcher() -> Result<GraphNode, BuiltinTemplateError> {
    let fields = &[
        "explorerProposal",
        "synthesizerProposal",
        "challengerProposal",
        REPORT,
        FEEDBACK,
        REVIEW,
    ];
    Ok(GraphNode::Step(StepNode {
        name: node_name(RESEARCHER)?,
        worker: worker_ref("builtin.agent.researcher@1")?,
        instructions: Some(instructions(RESEARCHER_PROMPT)?),
        input: research_state(TemplateDelivery::None)?,
        output: string_record(&[REPORT])?,
        input_bindings: research_bindings(fields)?,
        write_bindings: vec![output_write(RESEARCHER, REPORT, REPORT)?],
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn reviewer(delivery: TemplateDelivery) -> Result<GraphNode, BuiltinTemplateError> {
    let mut fields = vec![FEEDBACK];
    let mut prompt = REVIEWER_PROMPT.to_owned();
    if delivery == TemplateDelivery::Push {
        fields.extend([TITLE_FIELD, DESCRIPTION_FIELD]);
        prompt.push_str("\nInclude a concise Conventional Commit title and delivery description describing the actual retained result.");
    }
    let mut writes = fields
        .iter()
        .map(|field| output_write(REVIEWER, field, field))
        .collect::<Result<Vec<_>, _>>()?;
    let mut decision = signal_write(REVIEWER, VERDICT_FIELD, REVIEW)?;
    decision.target = static_value(FieldPath::new(vec![
        field_name(REVIEW)?,
        field_name(VERDICT_FIELD)?,
    ]))?;
    writes.push(decision);
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(REVIEWER)?,
        worker: worker_ref("builtin.agent.research-reviewer@1")?,
        instructions: Some(instructions(&prompt)?),
        input: research_input(&[REPORT])?,
        output: string_record(&fields)?,
        input_bindings: research_bindings(&[REPORT])?,
        write_bindings: writes,
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: BTreeMap::from([(field_name(VERDICT_FIELD)?, enum_labels(VERDICTS)?)]),
        diagnostic: diagnostic_type()?,
    }))
}

fn publish(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let mut route = choice(
        "review_result",
        state,
        vec![
            ChoiceBranch {
                when: executable_error_guard(REVIEWER)?,
                node: fail("review_failed", "research_review_failed")?,
            },
            ChoiceBranch {
                when: signal_guard(REVIEWER, VERDICT_FIELD, ACCEPTED)?,
                node: named_delivery_node("checkpoint_delivery", DeliveryMode::Push)?,
            },
        ],
        Some(fail(
            "review_budget_exhausted",
            "research_review_budget_exhausted",
        )?),
    )?;
    if let GraphNode::Choice(group) = &mut route {
        group.promoted_state_paths = push_paths()?;
    }
    Ok(route)
}

fn terminal(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    if delivery == TemplateDelivery::None {
        return choice(
            "review_result",
            state,
            vec![
                ChoiceBranch {
                    when: executable_error_guard(REVIEWER)?,
                    node: fail("review_failed", "research_review_failed")?,
                },
                ChoiceBranch {
                    when: signal_guard(REVIEWER, VERDICT_FIELD, &["revise"])?,
                    node: fail(
                        "review_budget_exhausted",
                        "research_review_budget_exhausted",
                    )?,
                },
            ],
            Some(succeed_null("done")?),
        );
    }
    choice(
        "delivery_result",
        state,
        vec![
            ChoiceBranch {
                when: executable_error_guard("checkpoint_delivery")?,
                node: fail("delivery_failed", "research_delivery_failed")?,
            },
            ChoiceBranch {
                when: named_delivery_signal_guard(
                    "checkpoint_delivery",
                    &[DELIVERY_REPAIR_REQUIRED_LABEL],
                )?,
                node: fail(
                    "delivery_requires_reconciliation",
                    "research_delivery_requires_reconciliation",
                )?,
            },
            ChoiceBranch {
                when: named_delivery_signal_guard("checkpoint_delivery", &[DELIVERY_PUSHED_LABEL])?,
                node: delivery_success("done", DeliveryMode::Push)?,
            },
        ],
        None,
    )
}

fn research_state(delivery: TemplateDelivery) -> Result<PayloadType, BuiltinTemplateError> {
    let PayloadType::Record { mut fields } = research_input(&[
        "explorerProposal",
        "synthesizerProposal",
        "challengerProposal",
        REPORT,
        FEEDBACK,
    ])?
    else {
        return Err(BuiltinTemplateError::InvalidStaticContract);
    };
    fields.insert(
        field_name(REVIEW)?,
        required(record_type(vec![(
            VERDICT_FIELD,
            PayloadType::Enum {
                values: enum_labels(VERDICTS)?,
            },
            false,
        )])?),
    );
    if delivery == TemplateDelivery::Push {
        add_delivery_state_fields(&mut fields, DeliveryMode::Push)?;
        fields.insert(
            field_name(DELIVERY_FEEDBACK_FIELD)?,
            required(PayloadType::String),
        );
    }
    Ok(PayloadType::Record { fields })
}

fn research_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        ("options", options_type()?, false),
    ])
}

fn options_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![("iterations", PayloadType::Integer, false)])
}

fn research_input(extra: &[&str]) -> Result<PayloadType, BuiltinTemplateError> {
    let mut fields = vec![
        (TASK_FIELD, PayloadType::String, true),
        ("options", options_type()?, true),
    ];
    fields.extend(extra.iter().map(|name| (*name, PayloadType::String, true)));
    record_type(fields)
}

fn string_record(fields: &[&str]) -> Result<PayloadType, BuiltinTemplateError> {
    record_type(
        fields
            .iter()
            .map(|name| (*name, PayloadType::String, true))
            .collect(),
    )
}

fn research_bindings(extra: &[&str]) -> Result<Vec<InputBinding>, BuiltinTemplateError> {
    [TASK_FIELD, "options"]
        .into_iter()
        .chain(extra.iter().copied())
        .map(|name| state_input(name, name))
        .collect()
}

fn paths(names: &[&str]) -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    names.iter().map(|name| field_path(name)).collect()
}

fn review_paths(delivery: TemplateDelivery) -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    let mut result = paths(&[FEEDBACK, REVIEW])?;
    if delivery == TemplateDelivery::Push {
        result.extend(paths(&[TITLE_FIELD, DESCRIPTION_FIELD])?);
    }
    Ok(result)
}

fn research_paths(delivery: TemplateDelivery) -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    let mut result = review_paths(delivery)?;
    result.push(field_path(REPORT)?);
    Ok(result)
}

fn push_paths() -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    let PayloadType::Record { fields } = static_value(delivery_result_schema(DeliveryMode::Push))?
    else {
        return Err(BuiltinTemplateError::InvalidStaticContract);
    };
    let mut result = fields
        .keys()
        .map(|name| static_value(FieldPath::new(vec![name.clone()])))
        .collect::<Result<Vec<_>, _>>()?;
    result.push(field_path(DELIVERY_FEEDBACK_FIELD)?);
    Ok(result)
}

fn round_paths(delivery: TemplateDelivery) -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    let mut result = research_paths(delivery)?;
    if delivery == TemplateDelivery::Push {
        result.extend(push_paths()?);
    }
    Ok(result)
}
