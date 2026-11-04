//! Parallel specialist review of a pull request, verified findings, and one published report.
use openengine_cluster_protocol::InputBinding;

use super::*;

const TRIAGE: &str = "triage";
const FINALIZE_UNIT: &str = "finalize_unit";
const FINALIZE_REST: &str = "finalize_rest";
const REPORT: &str = "report";
const SUMMARY: &str = "summary";
const UNITS: &str = "units";
const FINDINGS: &str = "findings";
const UNIT_FINAL: &str = "unitFinal";
const REST_FINAL: &str = "restFinal";
/// Triage is asked for at most six units; the maps leave room for two more.
const MAX_UNITS: u64 = 8;
const MAX_PROMPT_DEPTH: u8 = 4;

/// Reviewers of every unit and their instructions. The two broad reviews run independently.
const UNIT_REVIEWERS: [(&str, &str); 4] = [
    ("unit_broad_a", "unit_review"),
    ("unit_broad_b", "unit_review"),
    ("unit_depth", "unit_depth"),
    ("unit_design", "unit_design"),
];
/// Reviewers of the whole change, each hunting one kind of failure with the same-named instructions.
const HUNTERS: [&str; 5] = [
    "hunt_logic",
    "hunt_state",
    "hunt_contracts",
    "hunt_errors",
    "hunt_claims",
];

/// Instruction files. `{{name}}` includes the named file.
const PROMPTS: [(&str, &str); 18] = [
    ("finalize", include_str!("code_review/finalize.md")),
    (
        "finalize_rest",
        include_str!("code_review/finalize_rest.md"),
    ),
    (
        "finalize_steps",
        include_str!("code_review/finalize_steps.md"),
    ),
    ("hunt", include_str!("code_review/hunt.md")),
    ("hunt_claims", include_str!("code_review/hunt_claims.md")),
    (
        "hunt_contracts",
        include_str!("code_review/hunt_contracts.md"),
    ),
    ("hunt_errors", include_str!("code_review/hunt_errors.md")),
    ("hunt_logic", include_str!("code_review/hunt_logic.md")),
    ("hunt_state", include_str!("code_review/hunt_state.md")),
    ("intro", include_str!("code_review/intro.md")),
    ("report", include_str!("code_review/report.md")),
    ("reporting", include_str!("code_review/reporting.md")),
    ("severity", include_str!("code_review/severity.md")),
    ("standards", include_str!("code_review/standards.md")),
    ("triage", include_str!("code_review/triage.md")),
    ("unit_depth", include_str!("code_review/unit_depth.md")),
    ("unit_design", include_str!("code_review/unit_design.md")),
    ("unit_review", include_str!("code_review/unit_review.md")),
];

fn prompt(name: &str) -> Result<String, BuiltinTemplateError> {
    expand(name, 0)
}

fn expand(name: &str, depth: u8) -> Result<String, BuiltinTemplateError> {
    let text = PROMPTS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, text)| text.trim())
        .filter(|_| depth <= MAX_PROMPT_DEPTH)
        .ok_or(BuiltinTemplateError::InvalidStaticContract)?;
    let mut expanded = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("{{") {
        let (included, after) = after
            .split_once("}}")
            .ok_or(BuiltinTemplateError::InvalidStaticContract)?;
        expanded.push_str(before);
        expanded.push_str(&expand(included, depth + 1)?);
        rest = after;
    }
    expanded.push_str(rest);
    Ok(expanded)
}

fn findings_type() -> Result<PayloadType, BuiltinTemplateError> {
    let severity = PayloadType::Enum {
        values: enum_labels(&["high", "medium", "low"])?,
    };
    Ok(array(record_type(vec![
        ("file", PayloadType::String, true),
        ("startLine", PayloadType::Integer, true),
        ("endLine", PayloadType::Integer, true),
        ("severity", severity, true),
        ("title", PayloadType::String, true),
        ("body", PayloadType::String, true),
    ])?))
}

fn unit_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        ("name", PayloadType::String, true),
        ("files", array(PayloadType::String), true),
        ("focus", PayloadType::String, true),
    ])
}

/// State field holding a reviewer's findings: `unit_broad_a` becomes `unitBroadAFindings`.
fn findings_field(node: &str) -> String {
    let mut field = String::new();
    for (index, part) in node.split('_').enumerate() {
        let mut chars = part.chars();
        if index > 0 {
            field.extend(chars.next().map(|first| first.to_ascii_uppercase()));
        }
        field.extend(chars);
    }
    field + "Findings"
}

/// Every reviewer's findings, as the finalizers read them.
fn reviewer_findings() -> Result<Vec<(String, PayloadType)>, BuiltinTemplateError> {
    let findings = findings_type()?;
    let units = UNIT_REVIEWERS
        .iter()
        .map(|(node, _)| (findings_field(node), array(findings.clone())));
    let hunters = HUNTERS
        .iter()
        .map(|node| (findings_field(node), findings.clone()));
    Ok(units.chain(hunters).collect())
}

fn state_type(
    input: &PayloadType,
    delivery: TemplateDelivery,
) -> Result<(PayloadType, PayloadType), BuiltinTemplateError> {
    let (state, result) = super::review::review_state(input, delivery)?;
    let mut fields = record_fields(state)?;
    let findings = findings_type()?;
    let derived = [
        (SUMMARY.to_owned(), PayloadType::String),
        (UNITS.to_owned(), array(unit_type()?)),
        (UNIT_FINAL.to_owned(), array(findings.clone())),
        (REST_FINAL.to_owned(), findings),
    ];
    // Every derived field is required and starts at its type's empty value.
    for (name, value_type) in derived.into_iter().chain(reviewer_findings()?) {
        fields.insert(field_name(&name)?, required(value_type));
    }
    Ok((PayloadType::Record { fields }, result))
}

fn from_state(name: &str) -> Result<DataSelector, BuiltinTemplateError> {
    Ok(DataSelector::State {
        path: field_path(name)?,
    })
}

fn from_unit(name: &str) -> Result<DataSelector, BuiltinTemplateError> {
    Ok(DataSelector::Item {
        path: field_path(name)?,
    })
}

/// A read-only agent with the review input, the optional caller instructions and `extra` fields.
/// Each `(field, type, state field)` output is written to state.
fn agent(
    name: &str,
    instructions_name: &str,
    extra: Vec<(String, PayloadType, DataSelector)>,
    outputs: Vec<(&str, PayloadType, &str)>,
) -> Result<GraphNode, BuiltinTemplateError> {
    let output = record_type(
        outputs
            .iter()
            .map(|(field, value_type, _)| (*field, value_type.clone(), true))
            .collect(),
    )?;
    let write_bindings = outputs
        .iter()
        .map(|(field, _, target)| output_write(name, field, target))
        .collect::<Result<_, _>>()?;
    let mut input_bindings = super::pr_review::bindings(&[])?;
    for (target, _, value) in &extra {
        input_bindings.push(InputBinding {
            target: field_path(target)?,
            value: value.clone(),
        });
    }
    let input = super::pr_review::agent_input(
        extra
            .iter()
            .map(|(target, value_type, _)| (target.as_str(), value_type.clone(), true))
            .collect(),
    )?;
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(name)?,
        worker: worker_ref(&format!(
            "builtin.agent.code-review-{}@1",
            name.replace('_', "-")
        ))?,
        instructions: Some(instructions(&prompt(instructions_name)?)?),
        input,
        output,
        input_bindings,
        write_bindings,
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: BTreeMap::new(),
        diagnostic: PayloadType::Null,
    }))
}

fn findings_agent(
    name: &str,
    instructions_name: &str,
    extra: Vec<(String, PayloadType, DataSelector)>,
    target: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    agent(
        name,
        instructions_name,
        extra,
        vec![(FINDINGS, findings_type()?, target)],
    )
}

fn triage() -> Result<GraphNode, BuiltinTemplateError> {
    agent(
        TRIAGE,
        TRIAGE,
        Vec::new(),
        vec![
            (SUMMARY, PayloadType::String, SUMMARY),
            (UNITS, array(unit_type()?), UNITS),
        ],
    )
}

fn summary_input() -> Result<(String, PayloadType, DataSelector), BuiltinTemplateError> {
    Ok((
        SUMMARY.to_owned(),
        PayloadType::String,
        from_state(SUMMARY)?,
    ))
}

fn units_input() -> Result<(String, PayloadType, DataSelector), BuiltinTemplateError> {
    Ok((UNITS.to_owned(), array(unit_type()?), from_state(UNITS)?))
}

fn unit_inputs(
    with_focus: bool,
) -> Result<Vec<(String, PayloadType, DataSelector)>, BuiltinTemplateError> {
    let mut inputs = vec![
        summary_input()?,
        (
            "unitName".to_owned(),
            PayloadType::String,
            from_unit("name")?,
        ),
        (
            "unitFiles".to_owned(),
            array(PayloadType::String),
            from_unit("files")?,
        ),
    ];
    if with_focus {
        inputs.push((
            "unitFocus".to_owned(),
            PayloadType::String,
            from_unit("focus")?,
        ));
    }
    Ok(inputs)
}

fn findings_inputs() -> Result<Vec<(String, PayloadType, DataSelector)>, BuiltinTemplateError> {
    reviewer_findings()?
        .into_iter()
        .map(|(name, value_type)| {
            let value = from_state(&name)?;
            Ok((name, value_type, value))
        })
        .collect()
}

fn unit_map(
    name: &str,
    state: &PayloadType,
    body: GraphNode,
    promoted: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name(name)?,
        state: state.clone(),
        over: from_state(UNITS)?,
        body: Box::new(body),
        max_items: positive(MAX_UNITS)?,
        promoted_state_paths: vec![field_path(promoted)?],
    }))
}

/// The map for a unit reviewer: `unit_depth` runs in `units_depth`.
fn map_name(node: &str) -> String {
    format!("units{}", node.trim_start_matches("unit"))
}

fn parallel(
    name: &str,
    state: &PayloadType,
    branches: Vec<GraphNode>,
    promoted: &[String],
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Par(ParNode {
        name: node_name(name)?,
        state: state.clone(),
        branches: non_empty(branches)?,
        promoted_state_paths: promoted
            .iter()
            .map(|path| field_path(path))
            .collect::<Result<_, _>>()?,
        join: Join::All {},
    }))
}

fn discover(state: &PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let mut branches = Vec::new();
    for (node, instructions_name) in UNIT_REVIEWERS {
        let target = findings_field(node);
        let reviewer = findings_agent(node, instructions_name, unit_inputs(true)?, &target)?;
        branches.push(unit_map(&map_name(node), state, reviewer, &target)?);
    }
    for node in HUNTERS {
        let inputs = vec![summary_input()?, units_input()?];
        branches.push(findings_agent(node, node, inputs, &findings_field(node))?);
    }
    let promoted = reviewer_findings()?
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    parallel("discover", state, branches, &promoted)
}

fn finalize(state: &PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let mut unit_fields = unit_inputs(false)?;
    unit_fields.extend(findings_inputs()?);
    let finalize_unit = findings_agent(FINALIZE_UNIT, "finalize", unit_fields, UNIT_FINAL)?;
    let mut rest_inputs = vec![summary_input()?, units_input()?];
    rest_inputs.extend(findings_inputs()?);
    let finalize_rest = findings_agent(FINALIZE_REST, FINALIZE_REST, rest_inputs, REST_FINAL)?;
    parallel(
        "finalize",
        state,
        vec![
            unit_map("finalize_units", state, finalize_unit, UNIT_FINAL)?,
            finalize_rest,
        ],
        &[UNIT_FINAL.to_owned(), REST_FINAL.to_owned()],
    )
}

fn report() -> Result<GraphNode, BuiltinTemplateError> {
    let findings = findings_type()?;
    agent(
        REPORT,
        REPORT,
        vec![
            (
                UNIT_FINAL.to_owned(),
                array(findings.clone()),
                from_state(UNIT_FINAL)?,
            ),
            (REST_FINAL.to_owned(), findings, from_state(REST_FINAL)?),
        ],
        vec![("body", PayloadType::String, "body")],
    )
}

/// A failed map item or a map with more items than it allows.
fn map_failed(map: &str, body: &str) -> Result<(String, Guard), BuiltinTemplateError> {
    let guard = Guard::Any {
        guards: non_empty(vec![
            control_guard(map, ControlSource::Group, Some("overflow"), &["overflow"])?,
            Guard::KOfMap {
                count: positive(1)?,
                value: error_selector(body)?,
                labels: worker_error_labels()?,
            },
        ])?,
    };
    Ok((map.to_owned(), guard))
}

fn agent_failed(node: &str) -> Result<(String, Guard), BuiltinTemplateError> {
    Ok((node.to_owned(), executable_error_guard(node)?))
}

/// Continue only when `when` does not hold; otherwise fail before anything is published.
fn unless_failed(
    name: &str,
    when: Guard,
    state: &PayloadType,
    continuation: GraphNode,
) -> Result<GraphNode, BuiltinTemplateError> {
    choice(
        &format!("{name}_result"),
        state.clone(),
        vec![ChoiceBranch {
            when,
            node: fail(&format!("{name}_failed"), &format!("{name}_failed"))?,
        }],
        Some(continuation),
    )
}

/// One choice per check keeps each guard's control space small enough to verify.
fn unless_any_failed(
    checks: Vec<(String, Guard)>,
    state: &PayloadType,
    continuation: GraphNode,
) -> Result<GraphNode, BuiltinTemplateError> {
    checks
        .into_iter()
        .rev()
        .try_fold(continuation, |next, (name, when)| {
            unless_failed(&name, when, state, next)
        })
}

pub(super) fn code_review_graph(
    delivery: TemplateDelivery,
) -> Result<GraphSpec, BuiltinTemplateError> {
    let input = super::pr_review::input_type()?;
    let (state, result) = state_type(&input, delivery)?;
    let completed = sequence(
        "review_completed",
        state.clone(),
        super::review::review_continuation(&state, &result, delivery)?,
        Vec::new(),
    )?;
    let reported = sequence(
        "reported",
        state.clone(),
        vec![
            report()?,
            unless_any_failed(vec![agent_failed(REPORT)?], &state, completed)?,
        ],
        Vec::new(),
    )?;
    let finalize_checks = vec![
        map_failed("finalize_units", FINALIZE_UNIT)?,
        agent_failed(FINALIZE_REST)?,
    ];
    let finalized = sequence(
        "finalized",
        state.clone(),
        vec![
            finalize(&state)?,
            unless_any_failed(finalize_checks, &state, reported)?,
        ],
        Vec::new(),
    )?;
    let mut review_checks = Vec::new();
    for (node, _) in UNIT_REVIEWERS {
        review_checks.push(map_failed(&map_name(node), node)?);
    }
    for node in HUNTERS {
        review_checks.push(agent_failed(node)?);
    }
    let reviewed = sequence(
        "reviewed",
        state.clone(),
        vec![
            discover(&state)?,
            unless_any_failed(review_checks, &state, finalized)?,
        ],
        Vec::new(),
    )?;
    graph(
        input,
        sequence(
            "run",
            state.clone(),
            vec![
                triage()?,
                unless_any_failed(vec![agent_failed(TRIAGE)?], &state, reviewed)?,
            ],
            Vec::new(),
        )?,
    )
}

#[cfg(test)]
#[path = "code_review/tests.rs"]
mod tests;
