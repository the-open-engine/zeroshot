//! Built-in bounded autoresearch graph.

use std::collections::BTreeMap;

use openengine_cluster_protocol::{DataSelector, InputBinding, RecordField};

use crate::native_v2_delivery::DELIVERY_PUSHED_LABEL;

use super::*;

pub(super) const AUTO_RESEARCH_ITERATIONS: u64 = 10;
const OPTIONS_FIELD: &str = "options";
const ITERATIONS_FIELD: &str = "iterations";
const SCOUT_ROLES_FIELD: &str = "scoutRoles";
const PROPOSALS_FIELD: &str = "proposals";
const PLAN_REVIEWS_FIELD: &str = "planReviews";
const JUDGE_ROLES_FIELD: &str = "judgeRoles";
const WORK_ITEMS_FIELD: &str = "workItems";
const CONTINUATION_ITEMS_FIELD: &str = "continuationItems";
const ROLE_FIELD: &str = "role";
const REVIEWS_FIELD: &str = "reviews";
const VERDICTS_FIELD: &str = "verdicts";
const ADOPT_LABEL: &str = "adopt";
const RECORD_ONLY_LABEL: &str = "record_only";
const ABORT_LABEL: &str = "abort";
const SCOUT_ROLE_LABELS: [&str; 3] = ["explorer", "synthesizer", "challenger"];
const JUDGE_ROLE_LABELS: [&str; 3] = ["evidence", "method", "progress"];
const WORK_ITEM_LABELS: [&str; 1] = ["experiment"];
const RESEARCH_VERDICT_LABELS: [&str; 3] = [ADOPT_LABEL, RECORD_ONLY_LABEL, ABORT_LABEL];

pub(super) fn auto_research_graph(
    delivery: TemplateDelivery,
) -> Result<GraphSpec, BuiltinTemplateError> {
    let root_state = auto_research_state(delivery, false)?;
    let research_state = auto_research_state(delivery, true)?;
    graph(
        auto_research_input_type()?,
        sequence(
            "run",
            root_state.clone(),
            vec![
                bootstrap()?,
                after_bootstrap(root_state, research_state, delivery)?,
            ],
            Vec::new(),
        )?,
    )
}

fn auto_research_delivery_mode(
    delivery: TemplateDelivery,
) -> Result<Option<DeliveryMode>, BuiltinTemplateError> {
    match delivery {
        TemplateDelivery::None => Ok(None),
        TemplateDelivery::Push => Ok(Some(DeliveryMode::Push)),
        TemplateDelivery::PullRequest | TemplateDelivery::Merge => {
            Err(BuiltinTemplateError::UnsupportedDelivery {
                template: "auto-research",
                delivery,
            })
        }
    }
}

fn bootstrap() -> Result<GraphNode, BuiltinTemplateError> {
    let write_bindings = [
        SCOUT_ROLES_FIELD,
        JUDGE_ROLES_FIELD,
        WORK_ITEMS_FIELD,
        CONTINUATION_ITEMS_FIELD,
        PROPOSALS_FIELD,
        PLAN_REVIEWS_FIELD,
        REVIEWS_FIELD,
        VERDICTS_FIELD,
    ]
    .into_iter()
    .map(|field| output_write("bootstrap", field, field))
    .collect::<Result<Vec<_>, _>>()?;
    Ok(GraphNode::Step(StepNode {
        name: node_name("bootstrap")?,
        worker: worker_ref("builtin.agent.research-bootstrap@1")?,
        instructions: Some(instructions(
            "Create or resume '.zeroshot/research' without changing candidate source files. The \
         directory is the durable memory for this task. Keep 'charter.md', 'state.json', \
         'summary.md', and 'backlog.json' at its root. The backlog indexes live, tested, \
         rejected, and parked directions with evidence references and possible next tests. Preserve \
         restorable candidate files under 'archive/<artifact-id>/files/' with an immutable \
         'manifest.json' listing the complete mutable workspace scope, relative paths, SHA-256 hashes, \
         file types, modes and symlink targets, absent paths, provenance, parent artifact ID, and a \
         reason to revisit. Include actual source \
         bytes; an idea or score alone is not a restorable artifact. Never include protected paths, \
         secrets, large generated output, or Git data. Mark oversize or incomplete candidates \
         nonrestorable in the backlog. Treat old backlog-only entries as nonrestorable on resume; \
         never invent missing candidate bytes. Capture a restorable baseline and incumbent before switching \
         parents; immutable archive entries are append-only. Put finalized, append-only iteration records \
         under 'iterations/NNNNNN/'; each record contains 'proposal.json', 'experiment.json', \
         'evaluations.json', 'decision.json', and 'artifacts.json'. Keep reversible backups under \
         'scratch/' and add that directory to the research '.gitignore'. Initialize 'state.json' \
         with schema version 1, the next iteration number, the retained workspace identity, its \
         artifact hashes, invariant status, and open invariant violations, any task-specific adopted \
         measures, and the last finalized disposition. The charter defines the research question, \
         boundaries, protected material, evidence standard, resource limits, and stopping rules from \
         the caller's task. Distinguish non-negotiable charter invariants from optional progress \
         measures. Track evidence that the retained workspace violates an invariant in 'state.json', \
         'summary.md', and 'backlog.json'; a verified repair of that violation takes priority over \
         optional improvement. Keep 'summary.md' explicit about the retained workspace identity and \
         invariant status separately from the best supported historical findings; never attribute a \
         finding from a restored artifact to the retained workspace. Check that prior finalized \
         records agree with mutable state and summary. If an earlier process left a draft, restore \
         its backup and record an aborted iteration before proceeding. Treat provider sessions as \
         disposable; files are authoritative. Do not use Git, do not rewrite a finalized iteration, \
         and keep large logs or binaries out of the committed research directory. Read \
         'options.iterations' from input, using ten when absent. Admission has already checked \
         that an authored value is a positive safe integer and set the loop to that exact count. \
         Record the resolved value as 'iterationLimit' in 'state.json' and reject a resumed state \
         with a different value. Return exactly three ordered scout roles (explorer, synthesizer, \
         challenger), exactly three ordered judge roles (evidence, method, progress), exactly one \
         experiment work item, and empty continuationItems, proposals, planReviews, reviews, and \
         verdicts arrays. \
             The graph validates role and work-item coverage before \
         dependent work and rejects missing, duplicate, extra, or failed entries.",
        )?),
        input: research_task_input_type()?,
        output: bootstrap_output_type()?,
        input_bindings: research_task_input_bindings()?,
        write_bindings,
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn after_bootstrap(
    route_state: PayloadType,
    research_state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let validated = validated_research(research_state, delivery)?;
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("bootstrap_result")?,
        state: route_state,
        branches: non_empty(vec![ChoiceBranch {
            when: executable_error_guard("bootstrap")?,
            node: fail("bootstrap_failed", "bootstrap_failed")?,
        }])?,
        otherwise: Some(Box::new(validated)),
        promoted_state_paths: Vec::new(),
    }))
}

fn validated_research(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let research = research_sequence(state.clone(), delivery)?;
    sequence(
        "validated_research",
        state.clone(),
        vec![topology_validation()?, topology_result(state)?, research],
        terminal_paths(delivery)?,
    )
}

fn research_sequence(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let terminal = auto_research_terminal(delivery)?;
    sequence(
        "research",
        state.clone(),
        vec![
            empty_continuation("bootstrap_validated", state.clone())?,
            research_loop(state, delivery)?,
            terminal,
        ],
        terminal_paths(delivery)?,
    )
}

fn auto_research_terminal(delivery: TemplateDelivery) -> Result<GraphNode, BuiltinTemplateError> {
    match auto_research_delivery_mode(delivery)? {
        Some(mode) => delivery_success("done", mode),
        None => succeed_null("done"),
    }
}

fn topology_validation() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name("topology_validation")?,
        worker: worker_ref("builtin.agent.research-topology@1")?,
        input: topology_input_type()?,
        output: PayloadType::Null,
        input_bindings: topology_input_bindings()?,
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: review_signals()?,
        diagnostic: diagnostic_type()?,
        instructions: Some(instructions(
            "Read only. Accept only when scoutRoles is exactly [explorer, synthesizer, challenger], \
             judgeRoles is exactly [evidence, method, progress], and workItems is exactly [experiment], \
             including cardinality, uniqueness, and order. Reject every other value with an actionable \
             diagnostic. Confirm state.json iterationLimit equals ten when options.iterations \
             is absent or the exact authored value otherwise. This preflight gates the \
             iteration loop. Do not edit files.",
        )?),
    }))
}

fn topology_result(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("topology_result")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: executable_error_guard("topology_validation")?,
                node: fail("topology_validation_failed", "invalid_research_topology")?,
            },
            ChoiceBranch {
                when: signal_guard("topology_validation", VERDICT_FIELD, &[REJECTED_LABEL])?,
                node: fail("topology_validation_rejected", "invalid_research_topology")?,
            },
            ChoiceBranch {
                when: signal_guard("topology_validation", VERDICT_FIELD, &[ACCEPTED_LABEL])?,
                node: empty_continuation("topology_validation_accepted", state)?,
            },
        ])?,
        otherwise: None,
        promoted_state_paths: Vec::new(),
    }))
}

fn research_loop(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Loop(LoopNode {
        name: node_name("research_loop")?,
        state: state.clone(),
        body: Box::new(research_iteration(state, delivery)?),
        until: None,
        max_iterations: positive(AUTO_RESEARCH_ITERATIONS)?,
        max_iterations_input: Some(static_value(FieldPath::new(vec![
            field_name(OPTIONS_FIELD)?,
            field_name(ITERATIONS_FIELD)?,
        ]))?),
        promoted_state_paths: terminal_paths(delivery)?,
    }))
}

fn research_iteration(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let mut children = vec![
        scout_stage(state.clone())?,
        research_phase(state.clone())?,
        disposition_audit_phase(state.clone())?,
    ];
    if auto_research_delivery_mode(delivery)?.is_some() {
        children.push(checkpoint(state.clone())?);
    }
    children.push(iteration_exit()?);
    children.push(exit_route(state.clone(), delivery)?);
    sequence(
        "research_iteration",
        state,
        children,
        terminal_paths(delivery)?,
    )
}

fn scout_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("hypothesis_scouts")?,
        state,
        body: Box::new(scout()?),
        over: DataSelector::State {
            path: field_path(SCOUT_ROLES_FIELD)?,
        },
        max_items: positive(3)?,
        promoted_state_paths: paths(&[PROPOSALS_FIELD])?,
    }))
}

fn scout() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name("research_scout")?,
        worker: worker_ref("builtin.agent.research-scout@1")?,
        input: scout_input_type()?,
        output: proposal_type()?,
        input_bindings: vec![state_input(TASK_FIELD, TASK_FIELD)?, item_role_binding()?],
        write_bindings: vec![output_write("research_scout", "proposal", PROPOSALS_FIELD)?],
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: BTreeMap::new(),
        diagnostic: PayloadType::Null,
        instructions: Some(instructions(
            "Act only in the assigned role. Explorer searches for a distinct, high-information direction \
             that the ledger has not tried. Synthesizer combines supported findings and targets the most \
             consequential open gap. Challenger develops a rival explanation, counterexample, boundary \
             case, or cheap discriminating test. Read the task, charter, ledger, current workspace, and \
             available evidence and the candidate archive. Tag the proposal with the current state.json \
             next iteration number. Do not edit anything. Return one bounded proposal with its question or \
             hypothesis, expected knowledge or artifact change, exact scope, procedure, evidence needed, \
             risks, falsification condition, and relation to prior attempts. It must fit in one iteration.",
        )?),
    }))
}

fn task_reviewer(
    name: &str,
    worker: &str,
    signals: BTreeMap<FieldName, NonEmptyEnumSet>,
    authored_instructions: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(name)?,
        worker: worker_ref(worker)?,
        input: task_type()?,
        output: PayloadType::Null,
        input_bindings: vec![state_input(TASK_FIELD, TASK_FIELD)?],
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals,
        diagnostic: diagnostic_type()?,
        instructions: Some(instructions(authored_instructions)?),
    }))
}

fn iteration_exit() -> Result<GraphNode, BuiltinTemplateError> {
    task_reviewer(
        "iteration_exit",
        "builtin.agent.research-exit-auditor@1",
        verdict_signals(&["continue", "stop"])?,
        "Read only. Check the newest finalized iteration, disposition audit, archive, state, and \
            charter. Return stop only for a reviewed and audited stop proposal when no concrete \
            affordable alternative remains, including archived parents. Return continue for every other \
            finalized iteration. Reject inconsistent ledger, workspace identity, or archive hashes with \
            an actionable diagnostic. Do not edit files or use Git.",
    )
}

fn exit_route(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let terminal = match auto_research_delivery_mode(delivery)? {
        Some(mode) => delivery_success("done_early", mode)?,
        None => succeed_null("done_early")?,
    };
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("exit_route")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: signal_guard("iteration_exit", VERDICT_FIELD, &["stop"])?,
                node: terminal,
            },
            ChoiceBranch {
                when: signal_guard("iteration_exit", VERDICT_FIELD, &["continue"])?,
                node: empty_continuation("iteration_continues", state)?,
            },
        ])?,
        otherwise: Some(Box::new(fail(
            "iteration_exit_failed",
            "iteration_exit_failed",
        )?)),
        promoted_state_paths: Vec::new(),
    }))
}

fn research_phase(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    sequence(
        "research_phase",
        state.clone(),
        vec![
            planner()?,
            planning_phase(state.clone())?,
            research_route(state)?,
        ],
        Vec::new(),
    )
}

fn planning_phase(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("planning_phase")?,
        state: state.clone(),
        body: Box::new(plan_reviewer()?),
        over: DataSelector::State {
            path: field_path(WORK_ITEMS_FIELD)?,
        },
        max_items: positive(1)?,
        promoted_state_paths: paths(&[PLAN_REVIEWS_FIELD])?,
    }))
}

fn research_route(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("research_route")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: mapped_error_guard("plan_review")?,
                node: recovery_stage(
                    state.clone(),
                    "abort_plan",
                    "Plan review failed. Leave the incumbent intact, finalize an aborted iteration with \
                        the review error, and advance once. Do not use Git.",
                )?,
            },
            ChoiceBranch {
                when: mapped_signal_guard("plan_review", 1, &["rejected"])?,
                node: recovery_stage(
                    state.clone(),
                    "abort_plan_rejected",
                    "Plan review rejected the selection. Leave the incumbent intact, finalize an \
                        aborted iteration with the diagnostic, and advance once. Do not use Git.",
                )?,
            },
            ChoiceBranch {
                when: mapped_signal_guard("plan_review", 1, &["stop"])?,
                node: stop_stage(state.clone())?,
            },
            ChoiceBranch {
                when: mapped_signal_guard("plan_review", 1, &["work"])?,
                node: work_phase(state.clone())?,
            },
        ])?,
        otherwise: Some(Box::new(fail(
            "plan_decision_missing",
            "plan_decision_missing",
        )?)),
        promoted_state_paths: Vec::new(),
    }))
}

fn work_phase(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("work_phase")?,
        state: state.clone(),
        body: Box::new(staging_stage(state)?),
        over: DataSelector::State {
            path: field_path(WORK_ITEMS_FIELD)?,
        },
        max_items: positive(1)?,
        promoted_state_paths: Vec::new(),
    }))
}

fn plan_reviewer() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name("plan_review")?,
        worker: worker_ref("builtin.agent.research-plan-review@1")?,
        input: selection_input_type()?,
        output: review_output_type()?,
        input_bindings: selection_input_bindings()?,
        write_bindings: vec![output_write("plan_review", "review", PLAN_REVIEWS_FIELD)?],
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: verdict_signals(&["work", "stop", "rejected"])?,
        diagnostic: diagnostic_type()?,
        instructions: Some(instructions(
            "Read only. Require exactly three nonempty, current-iteration scout proposals and a complete \
             selection.json. Reject a failed or incomplete planner handoff. \
             Check that the selected proposal or independent alternative is falsifiable, bounded, and \
             compared against the live archive and current incumbent. For work, require a named \
             parentArtifactId: incumbent or a restorable immutable archive ID; verify the archive \
             manifest exists, its scope is complete, and file hashes match before returning work. \
             Reject missing or nonrestorable parents. For stop, challenge the planner with the strongest \
             concrete affordable alternative from the scouts or archive; return stop only if none \
             survives the charter's cost and risk limits. Otherwise reject with that alternative. \
             Return a concise review string tagged with the current iteration number, naming the \
             strongest counterproposal, the selected parent check, and the reason for work, stop, \
             or rejection. Do not edit files or use Git.",
        )?),
    }))
}

fn stop_stage(_state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    research_recorder(
        "record_stop",
        "The plan reviewer approved stop. Do not change candidate files. Finalize this iteration with \
         the five standard records. decision.json records disposition stop and the challenged \
         alternatives; evaluations.json records the current planReviews[0] reviewer output, not stale \
         judge inputs \
         from a prior iteration; artifacts.json proves the unchanged retained incumbent hashes. \
         Update state, summary, and backlog once. Remove scratch only after finalization. Do not use Git.",
    )
}

fn staging_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    sequence(
        "staging_stage",
        state.clone(),
        vec![stage_parent()?, staging_review_stage(state)?],
        Vec::new(),
    )
}

fn stage_parent() -> Result<GraphNode, BuiltinTemplateError> {
    task_step(
        "stage_parent",
        "builtin.agent.research-stage@1",
        "Read the reviewed selection.json. Before changing candidate files, save the complete retained \
            incumbent mutable scope byte-for-byte under scratch/NNNNNN/incumbent and record its \
            inventory and hashes. If parentArtifactId is incumbent, leave it in place. Otherwise \
            restore the exact archived parent files from archive/<id>/files using its complete scope \
            and absence inventory; remove only mutable paths absent from that parent. Preserve \
            protected paths. Write scratch/NNNNNN/staging.json with selected and incumbent IDs, source \
            and staged hashes, changed paths, and restoration instructions. Do not edit the archive or \
            finalized records; do not use Git.",
    )
}

fn staging_review_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let route = GraphNode::Choice(ChoiceNode {
        name: node_name("staging_review_result")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: executable_error_guard("staging_review")?,
                node: recovery_stage(
                    state.clone(),
                    "abort_staging_review",
                    "Staging review failed. Restore the retained incumbent from scratch, prove its \
                        hashes, finalize an aborted iteration, and advance once. Do not use Git.",
                )?,
            },
            ChoiceBranch {
                when: signal_guard("staging_review", VERDICT_FIELD, &[REJECTED_LABEL])?,
                node: recovery_stage(
                    state.clone(),
                    "abort_staging_rejected",
                    "Staging review rejected the parent restoration. Restore the retained incumbent \
                        from scratch, prove its hashes, finalize an aborted iteration, and advance \
                        once. Do not use Git.",
                )?,
            },
            ChoiceBranch {
                when: signal_guard("staging_review", VERDICT_FIELD, &[ACCEPTED_LABEL])?,
                node: experiment_stage(state.clone())?,
            },
        ])?,
        otherwise: None,
        promoted_state_paths: Vec::new(),
    });
    sequence(
        "staging_review_stage",
        state,
        vec![staging_reviewer()?, route],
        Vec::new(),
    )
}

fn staging_reviewer() -> Result<GraphNode, BuiltinTemplateError> {
    task_reviewer(
        "staging_review",
        "builtin.agent.research-staging-review@1",
        review_signals()?,
        "Read only. Reject a failed or incomplete staging handoff. Before any experiment, independently \
            hash every selected parent file \
             against its immutable archive manifest, verify file type, mode, symlink target, complete \
             mutable path and absence inventory, verify the retained incumbent scratch backup and protected paths, and \
             check staging.json names the reviewed parentArtifactId. For incumbent parent, verify \
             no candidate path changed. Reject any mismatch or incomplete backup. Do not edit files \
             or use Git.",
    )
}

fn experiment_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    sequence(
        "experiment_stage",
        state.clone(),
        vec![experiment()?, review_stage(state)?],
        Vec::new(),
    )
}

fn planner() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name("plan_experiment")?,
        worker: worker_ref("builtin.agent.research-planner@1")?,
        instructions: Some(instructions(
            "Require three fresh scout proposals tagged with the current iteration; if any are missing, \
                stale, or failed, write no valid selection. Read the task, charter, state, summary, \
             backlog, immutable candidate archive, finalized records, and current incumbent. Consider \
             refinement of the incumbent, the strongest live archived branch, and fresh directions \
             when warranted; do not force a fixed schema of alternatives. \
             Compare their expected progress and information gain against cost, uncertainty, risk, and \
             diminishing returns. Choose one falsifiable experiment that fits this iteration; sustained \
             work on the best direction is valid when its expected value remains highest. Do not force a \
             quota of novel ideas or repeat a failed direction without new evidence. Choose both a \
             proposal and its starting artifact. Set parentArtifactId to incumbent or an immutable \
             restorable archive ID; never select a nonrestorable idea as a parent. Do not change workspace \
             artifacts, the backlog, or finalized records. Read the next iteration number from 'state.json' \
             and write only '.zeroshot/research/scratch/NNNNNN/selection.json'. Record action \
             experiment or stop; for experiment include parentArtifactId, selected proposal and \
             reason to use that parent. Record the three scouts, other considered alternatives, their \
             evidence references, expected value, costs, and why the selected direction wins now. \
             Stop only when no concrete affordable experiment remains and explain why the strongest \
             scout or archived alternative fails. Include the \
             chosen question, expected learning or artifact change, falsification condition, protected \
             paths or data, procedure, observations or sources to collect, comparison or reference checks \
             when useful, evaluation rules, resource limits, and disposition conditions. Predeclare \
             evidence collection and evaluation order when observations may be noisy or order-dependent; \
             use charter-defined controls, repetitions, and thresholds rather than choosing them after \
             seeing results. Do not let an optional progress threshold reject a verified repair when the \
             ledger already shows that the retained workspace violates a charter invariant. In that case, \
             prioritize a bounded repair or discriminating test, and judge repaired validity before \
             optional improvement. Do not use Git.",
        )?),
        input: selection_input_type()?,
        output: PayloadType::Null,
        input_bindings: selection_input_bindings()?,
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn experiment() -> Result<GraphNode, BuiltinTemplateError> {
    task_step(
        "experiment",
        "builtin.agent.research-experimenter@1",
        "Read the reviewed selection and verified staging.json. If either is absent or incomplete, \
         do not edit candidate files; leave a draft explaining why. Otherwise execute exactly that \
         experiment on the staged parent. Preserve the incumbent backup and record a manifest \
         covering modified, deleted, and new paths relative to the staged parent. Never use Git. Gather \
             evidence through the declared procedure. For an \
         executable artifact, compare prior and proposed states on the same inputs and environment; for \
         other research, retain source references, observations, and analysis steps that another judge \
         can inspect. Preserve all charter-protected material and respect its resource limits. Leave any \
         working changes in place for independent review. Write 'iterations/NNNNNN/draft.json' with the \
         question, plan, commands or queries, exit status, observations, timings when relevant, source \
         and environment facts, changed-path hashes, limitations, and short excerpts; keep bulky output \
         in ignored scratch. Do not judge your own experiment or update mutable summary state or a \
         finalized iteration.",
    )
}

fn review_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    judge_phase(state)
}

fn judge_phase(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("judge_phase")?,
        state: state.clone(),
        body: Box::new(judge_stage(state)?),
        over: DataSelector::State {
            path: field_path(WORK_ITEMS_FIELD)?,
        },
        max_items: positive(1)?,
        promoted_state_paths: Vec::new(),
    }))
}

fn judge_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    sequence(
        "judge_stage",
        state.clone(),
        vec![judge_map(state.clone())?, research_decision(state)?],
        Vec::new(),
    )
}

fn disposition_audit_phase(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("disposition_audit_phase")?,
        state: state.clone(),
        body: Box::new(decision_audit_stage(state)?),
        over: DataSelector::State {
            path: field_path(WORK_ITEMS_FIELD)?,
        },
        max_items: positive(1)?,
        promoted_state_paths: Vec::new(),
    }))
}

fn judge_map(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("independent_judges")?,
        state,
        body: Box::new(judge()?),
        over: DataSelector::State {
            path: field_path(JUDGE_ROLES_FIELD)?,
        },
        max_items: positive(3)?,
        promoted_state_paths: paths(&[REVIEWS_FIELD, VERDICTS_FIELD])?,
    }))
}

fn judge() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name("research_judge")?,
        worker: worker_ref("builtin.agent.research-judge@1")?,
        input: judge_input_type()?,
        output: review_output_type()?,
        input_bindings: vec![state_input(TASK_FIELD, TASK_FIELD)?, item_role_binding()?],
        write_bindings: vec![
            output_write("research_judge", "review", REVIEWS_FIELD)?,
            signal_write("research_judge", VERDICT_FIELD, VERDICTS_FIELD)?,
        ],
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: research_review_signals()?,
        diagnostic: diagnostic_type()?,
        instructions: Some(instructions(
            "Act only as the assigned judge. Reject missing or incomplete experiment output. Read the \
                current iteration's selection, draft, task, charter, \
             ledger, and workspace. Review independently and do not edit files. Evidence checks whether \
             observations support the main claims and repeats the decisive computation, source check, or \
             comparison when possible. Method audits design, controls, provenance, reproducibility, scope, \
             protected material, backups, and restoration. Progress compares the result with the charter \
             and ledger; a valid negative or inconclusive result can merit record_only when it removes a \
             live direction or reduces uncertainty. Adopt means the evidence permits retaining the working \
             changes. Record_only means the finding belongs in the ledger but workspace changes must be \
             restored. Abort means the evidence or method is invalid, incomplete, unsafe, or cannot support \
             a defensible finding. When prior evidence already proves that the retained workspace violates \
             a non-negotiable charter invariant, a verified repair merits adopt even if it does not improve \
             an optional measure. Do not choose record_only merely because that repair misses an optimization \
             threshold: restoring the known-invalid predecessor would violate the charter. Avoid \
             resource-heavy checks unless assigned evidence or the task cannot be judged otherwise; the \
             evidence role owns independent reproduction of the main measured claim. Return only the \
             assigned role's review and one of adopt, record_only, or abort.",
        )?),
    }))
}

#[derive(Clone, Copy)]
enum ResearchDisposition {
    Abort,
    Adopt,
    RecordOnly,
}

impl ResearchDisposition {
    fn label(self) -> &'static str {
        match self {
            Self::Abort => ABORT_LABEL,
            Self::Adopt => ADOPT_LABEL,
            Self::RecordOnly => RECORD_ONLY_LABEL,
        }
    }

    fn finalizer_name(self) -> &'static str {
        match self {
            Self::Abort => "finalize_aborted",
            Self::Adopt => "finalize_adopted",
            Self::RecordOnly => "finalize_record_only",
        }
    }

    fn finalizer_action(self) -> &'static str {
        match self {
            Self::Abort => {
                "The graph selected 'abort' because at least one judge emitted abort. Do not \
                 reinterpret the verdicts. Restore the retained incumbent byte-for-byte from \
                 scratch, including paths changed by parent staging, remove paths absent from it, \
                 prove restoration against incumbent hashes, and finalize the draft with disposition 'abort'."
            }
            Self::Adopt => {
                "The graph selected 'adopt' because all three judges emitted adopt. Do not \
                 reinterpret the verdicts. Keep the reviewed workspace changes and finalize the \
                 draft with disposition 'adopt'."
            }
            Self::RecordOnly => {
                "The graph selected 'record_only' because at least one judge emitted record_only \
                 and none emitted abort. Do not reinterpret the verdicts. Before restoration, archive \
                 the reviewed candidate with its complete mutable path inventory, file types, modes, \
                 symlink targets, \
                 file bytes, and hashes if it is safe and has a concrete revisit reason; otherwise mark \
                 it nonrestorable. Restore the retained incumbent byte-for-byte from scratch, including \
                 parent staging changes, and prove restoration against incumbent hashes, and finalize the draft \
                 with disposition 'record_only'."
            }
        }
    }
}

fn research_decision(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("research_decision")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: mapped_error_guard("research_judge")?,
                node: recovery_worker(
                    "abort_review",
                    "An independent judge failed. Restore the workspace from the current scratch \
                     manifest, remove newly created paths, and prove restoration against the prior \
                     state. Finalize the iteration as aborted with the five standard JSON records, \
                     record available reviews and the missing role, update the state, summary, and \
                     backlog, and advance the next iteration number. Remove this iteration's draft and \
                     backup only after restoration is proven. Keep prior finalized records unchanged \
                     and do not use Git. Return a Conventional Commit title and short checkpoint \
                     description.",
                )?,
            },
            ChoiceBranch {
                when: control_guard(
                    "independent_judges",
                    ControlSource::Group,
                    Some("overflow"),
                    &["overflow"],
                )?,
                node: fail("judge_activation_overflow", "invalid_research_topology")?,
            },
            ChoiceBranch {
                when: mapped_verdict_guard(1, &[ABORT_LABEL])?,
                node: decision_worker(ResearchDisposition::Abort)?,
            },
            ChoiceBranch {
                when: mapped_verdict_guard(3, &[ADOPT_LABEL])?,
                node: decision_worker(ResearchDisposition::Adopt)?,
            },
            ChoiceBranch {
                when: mapped_verdict_guard(1, &[RECORD_ONLY_LABEL])?,
                node: decision_worker(ResearchDisposition::RecordOnly)?,
            },
        ])?,
        otherwise: Some(Box::new(fail(
            "invalid_judge_consensus",
            "invalid_judge_consensus",
        )?)),
        promoted_state_paths: paths(&[TITLE_FIELD, DESCRIPTION_FIELD])?,
    }))
}

fn mapped_error_guard(node: &str) -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::KOfMap {
        count: positive(1)?,
        value: error_selector(node)?,
        labels: worker_error_labels()?,
    })
}

fn mapped_signal_guard(
    node: &str,
    count: u64,
    labels: &[&str],
) -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::KOfMap {
        count: positive(count)?,
        value: ControlSelector {
            name: node_name(node)?,
            source: ControlSource::Signal,
            field: Some(field_name(VERDICT_FIELD)?),
        },
        labels: enum_labels(labels)?,
    })
}

fn mapped_verdict_guard(count: u64, labels: &[&str]) -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::KOfMap {
        count: positive(count)?,
        value: ControlSelector {
            name: node_name("research_judge")?,
            source: ControlSource::Signal,
            field: Some(field_name(VERDICT_FIELD)?),
        },
        labels: enum_labels(labels)?,
    })
}

fn decision_worker(disposition: ResearchDisposition) -> Result<GraphNode, BuiltinTemplateError> {
    let name = disposition.finalizer_name();
    let authored_instructions = format!(
        "{} Store the three reviews and verdicts in 'evaluations.json' and write the exact '{}' \
         disposition plus its reason to 'decision.json'. Before removing scratch, preserve the \
         changed-path manifest, selected parent ID and hashes, pre-iteration incumbent, \
         reviewed-candidate, and final-current hashes in 'artifacts.json' so restoration or \
         retention remains independently auditable. For adopt, archive the displaced incumbent \
         if it remains a credible alternative; for record_only, retain a restorable reviewed \
         candidate only when its complete files and revisit reason are documented. Never overwrite \
         an archive entry. Put actual candidate files under archive/<id>/files and reference an \
         immutable manifest from the backlog; mark incomplete or oversize candidates nonrestorable. Reconcile \
         'state.json', 'summary.md', and 'backlog.json' so they separately identify the retained \
         workspace, its known invariant status and open violations, and the best supported historical \
         findings, including findings from restored artifacts. Archive the selected direction and credible \
         alternatives from the plan with their evidence, disposition, and useful next tests; preserve \
         promising branches even when another experiment was chosen. Never attribute a historical finding or \
         measure to the retained workspace unless hashes or provenance match. Update the retained \
         workspace identity, hashes, invariant status, open violations, and accepted measures only for \
         an adopted result. Advance the next iteration number and remove scratch only after any required \
         restoration is proven. Prior iteration directories are append-only. Keep large artifacts out \
         of Git and do not use Git commands. Return a Conventional Commit title and a short description \
         for this iteration checkpoint.",
        disposition.finalizer_action(),
        disposition.label(),
    );
    research_recorder(name, &authored_instructions)
}

fn research_recorder(
    name: &str,
    authored_instructions: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name(name)?,
        worker: worker_ref("builtin.agent.research-recorder@1")?,
        instructions: Some(instructions(authored_instructions)?),
        input: decision_input_type()?,
        output: change_manifest_type()?,
        input_bindings: decision_input_bindings()?,
        write_bindings: vec![
            output_write(name, TITLE_FIELD, TITLE_FIELD)?,
            output_write(name, DESCRIPTION_FIELD, DESCRIPTION_FIELD)?,
        ],
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn decision_audit_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let name = "audit_disposition";
    let result = GraphNode::Choice(ChoiceNode {
        name: node_name("audit_disposition_result")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: executable_error_guard(name)?,
                node: fail(
                    "audit_disposition_failed",
                    "iteration_finalization_audit_failed",
                )?,
            },
            ChoiceBranch {
                when: signal_guard(name, VERDICT_FIELD, &[REJECTED_LABEL])?,
                node: fail(
                    "audit_disposition_rejected",
                    "iteration_finalization_rejected",
                )?,
            },
            ChoiceBranch {
                when: signal_guard(name, VERDICT_FIELD, &[ACCEPTED_LABEL])?,
                node: empty_continuation("audit_disposition_accepted", state.clone())?,
            },
        ])?,
        otherwise: None,
        promoted_state_paths: Vec::new(),
    });
    sequence(
        "audit_disposition_stage",
        state,
        vec![decision_auditor()?, result],
        Vec::new(),
    )
}

fn decision_auditor() -> Result<GraphNode, BuiltinTemplateError> {
    task_reviewer(
        "audit_disposition",
        "builtin.agent.research-disposition-auditor@1",
        review_signals()?,
        "Read only. Recompute the required disposition from the finalized records according to \
             the path that actually ran. A reviewed stop selection requires stop, a current plan \
             review in evaluations.json, no experiment or judge claims, an unchanged incumbent, and \
             a concrete challenge of live affordable alternatives. A failed plan, staging, experiment, \
             or judge stage requires abort, its failed stage and available evidence, and no fabricated \
             evaluations. For a complete three-judge evaluation, any abort requires abort, three \
             adopts require adopt, and every other complete valid combination requires record_only. \
             Require evaluations.json to preserve the current planReviews[0] rationale on stop and \
             experiment paths. Reject duplicate, extra, unknown, stale, or inconsistent evaluations. Confirm \
             decision.json records that exact disposition; all five standard iteration records are \
             finalized; prior finalized iterations are unchanged; mutable state and summary advance \
             exactly once; and backlog retains the scouts, selected and credible alternatives with \
             accurate evidence references, parent identity, disposition, and archive IDs. Require no \
             draft or scratch backup to remain. Independently hash every restorable archive file \
             against its immutable manifest and verify complete mutable scope and provenance. For an \
             experiment, artifacts.json must contain the changed-path manifest, selected parent, \
             pre-iteration incumbent, reviewed candidate, and final-current hashes. For stop or \
             recovery before an experiment, require explicit absent candidate fields and hashes of the \
             unchanged or restored incumbent. For abort or record_only, require byte-for-byte \
             equality with the pre-iteration incumbent, restored deleted paths, and no candidate-created \
             paths. For adopt, require current hashes to equal the reviewed candidate and the retained \
             workspace identity to advance consistently. Return accepted only when the verdict rule, \
             ledger, archive, and current filesystem all agree; otherwise reject with an actionable \
             diagnostic. Do not edit files and do not use Git commands.",
    )
}

fn recovery_stage(
    _state: PayloadType,
    name: &str,
    authored_instructions: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    recovery_worker(name, authored_instructions)
}

fn recovery_worker(
    name: &str,
    authored_instructions: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name(name)?,
        worker: worker_ref("builtin.agent.research-recovery@1")?,
        instructions: Some(instructions(authored_instructions)?),
        input: task_type()?,
        output: change_manifest_type()?,
        input_bindings: vec![state_input(TASK_FIELD, TASK_FIELD)?],
        write_bindings: vec![
            output_write(name, TITLE_FIELD, TITLE_FIELD)?,
            output_write(name, DESCRIPTION_FIELD, DESCRIPTION_FIELD)?,
        ],
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn checkpoint(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let manifest_result = GraphNode::Choice(ChoiceNode {
        name: node_name("checkpoint_manifest_result")?,
        state: state.clone(),
        branches: non_empty(vec![ChoiceBranch {
            when: executable_error_guard("checkpoint_manifest")?,
            node: fail("checkpoint_manifest_failed", "checkpoint_manifest_failed")?,
        }])?,
        otherwise: Some(Box::new(checkpoint_delivery_stage(state.clone())?)),
        promoted_state_paths: checkpoint_paths()?,
    });
    sequence(
        "checkpoint",
        state,
        vec![checkpoint_manifest()?, manifest_result],
        checkpoint_paths()?,
    )
}

fn checkpoint_manifest() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name("checkpoint_manifest")?,
        worker: worker_ref("builtin.agent.research-checkpoint-manifest@1")?,
        instructions: Some(instructions(
            "Read the newest finalized iteration and mutable research summary without editing files. \
             Confirm that no experiment draft or scratch backup is pending. Return a concise \
             Conventional Commit title and checkpoint description that accurately state whether the \
             iteration was accepted, rejected, or aborted. State the retained workspace identity and \
             invariant status separately from the best supported historical finding, and never imply that \
             a restored artifact is current. Do not use Git.",
        )?),
        input: task_type()?,
        output: change_manifest_type()?,
        input_bindings: vec![state_input(TASK_FIELD, TASK_FIELD)?],
        write_bindings: vec![
            output_write("checkpoint_manifest", TITLE_FIELD, TITLE_FIELD)?,
            output_write("checkpoint_manifest", DESCRIPTION_FIELD, DESCRIPTION_FIELD)?,
        ],
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn checkpoint_delivery_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let result = GraphNode::Choice(ChoiceNode {
        name: node_name("checkpoint_delivery_result")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: executable_error_guard("checkpoint_delivery")?,
                node: fail("checkpoint_delivery_failed", "checkpoint_delivery_failed")?,
            },
            ChoiceBranch {
                when: named_delivery_signal_guard(
                    "checkpoint_delivery",
                    &[DELIVERY_REPAIR_REQUIRED_LABEL],
                )?,
                node: fail("checkpoint_repair_required", "checkpoint_repair_required")?,
            },
            ChoiceBranch {
                when: named_delivery_signal_guard("checkpoint_delivery", &[DELIVERY_PUSHED_LABEL])?,
                node: checkpoint_audit_stage(state.clone())?,
            },
        ])?,
        otherwise: None,
        promoted_state_paths: Vec::new(),
    });
    sequence(
        "checkpoint_delivery_stage",
        state,
        vec![
            named_delivery_node("checkpoint_delivery", DeliveryMode::Push)?,
            result,
        ],
        checkpoint_paths()?,
    )
}

fn checkpoint_audit_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let result = GraphNode::Choice(ChoiceNode {
        name: node_name("checkpoint_audit_result")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: executable_error_guard("checkpoint_audit")?,
                node: fail("checkpoint_audit_failed", "checkpoint_audit_failed")?,
            },
            ChoiceBranch {
                when: signal_guard("checkpoint_audit", VERDICT_FIELD, &[REJECTED_LABEL])?,
                node: fail("checkpoint_audit_rejected", "checkpoint_audit_rejected")?,
            },
            ChoiceBranch {
                when: signal_guard("checkpoint_audit", VERDICT_FIELD, &[ACCEPTED_LABEL])?,
                node: empty_continuation("checkpoint_accepted", state.clone())?,
            },
        ])?,
        otherwise: None,
        promoted_state_paths: Vec::new(),
    });
    sequence(
        "checkpoint_audit_stage",
        state,
        vec![checkpoint_auditor()?, result],
        Vec::new(),
    )
}

fn checkpoint_auditor() -> Result<GraphNode, BuiltinTemplateError> {
    let input = static_value(delivery_result_schema(DeliveryMode::Push))?;
    let input_bindings = output_fields(&input)?
        .into_iter()
        .map(|field| state_input(&field, &field))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name("checkpoint_audit")?,
        worker: worker_ref("builtin.agent.research-checkpoint@1")?,
        input,
        output: PayloadType::Null,
        input_bindings,
        write_bindings: vec![diagnostic_write(
            "checkpoint_audit",
            DELIVERY_FEEDBACK_FIELD,
        )?],
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: review_signals()?,
        diagnostic: diagnostic_type()?,
        instructions: Some(instructions(
            "Read only. Confirm that the push receipt names the exact finalized research checkpoint \
             currently present in the workspace and that no experiment draft or scratch backup was \
             published. Return accepted only when the receipt and workspace prove those conditions; \
             otherwise return rejected with an actionable diagnostic. Do not edit files and do not use \
             Git commands.",
        )?),
    }))
}

fn empty_continuation(name: &str, state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let invalid_name = format!("{name}_unexpected_item");
    Ok(GraphNode::Map(MapNode {
        name: node_name(name)?,
        state,
        body: Box::new(fail(&invalid_name, "invalid_continuation_state")?),
        over: DataSelector::State {
            path: field_path(CONTINUATION_ITEMS_FIELD)?,
        },
        max_items: positive(1)?,
        promoted_state_paths: Vec::new(),
    }))
}

fn auto_research_state(
    delivery: TemplateDelivery,
    initialized: bool,
) -> Result<PayloadType, BuiltinTemplateError> {
    let mut fields = auto_research_fields(initialized)?;
    if let Some(mode) = auto_research_delivery_mode(delivery)? {
        add_delivery_state_fields(&mut fields, mode)?;
    }
    Ok(PayloadType::Record { fields })
}

fn auto_research_fields(
    initialized: bool,
) -> Result<BTreeMap<FieldName, RecordField>, BuiltinTemplateError> {
    let mut fields = research_base_fields()?;
    fields.extend(research_collection_fields(initialized)?);
    Ok(fields)
}

fn research_collection_fields(
    initialized: bool,
) -> Result<BTreeMap<FieldName, RecordField>, BuiltinTemplateError> {
    [
        (SCOUT_ROLES_FIELD, role_array_type(&SCOUT_ROLE_LABELS)?),
        (JUDGE_ROLES_FIELD, role_array_type(&JUDGE_ROLE_LABELS)?),
        (WORK_ITEMS_FIELD, role_array_type(&WORK_ITEM_LABELS)?),
        (CONTINUATION_ITEMS_FIELD, array_type(PayloadType::Null)),
        (PROPOSALS_FIELD, array_type(PayloadType::String)),
        (PLAN_REVIEWS_FIELD, array_type(PayloadType::String)),
        (REVIEWS_FIELD, array_type(PayloadType::String)),
        (VERDICTS_FIELD, array_type(verdict_type()?)),
    ]
    .into_iter()
    .map(|(name, value_type)| research_collection_field(name, value_type, initialized))
    .collect()
}

fn research_collection_field(
    name: &str,
    value_type: PayloadType,
    initialized: bool,
) -> Result<(FieldName, RecordField), BuiltinTemplateError> {
    let field = if initialized {
        required(value_type)
    } else {
        optional(value_type)
    };
    Ok((field_name(name)?, field))
}

fn research_base_fields() -> Result<BTreeMap<FieldName, RecordField>, BuiltinTemplateError> {
    let mut fields = BTreeMap::new();
    fields.insert(
        field_name(OPTIONS_FIELD)?,
        required(research_options_type()?),
    );
    for name in [
        TASK_FIELD,
        DELIVERY_FEEDBACK_FIELD,
        TITLE_FIELD,
        DESCRIPTION_FIELD,
    ] {
        fields.insert(field_name(name)?, required(PayloadType::String));
    }
    Ok(fields)
}

fn array_type(items: PayloadType) -> PayloadType {
    PayloadType::Array {
        items: Box::new(items),
    }
}

type InputField = (&'static str, PayloadType, bool);

fn research_options_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![(ITERATIONS_FIELD, PayloadType::Integer, false)])
}

fn auto_research_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (OPTIONS_FIELD, research_options_type()?, false),
    ])
}

fn research_task_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (OPTIONS_FIELD, research_options_type()?, true),
    ])
}

fn research_task_input_bindings() -> Result<Vec<InputBinding>, BuiltinTemplateError> {
    [TASK_FIELD, OPTIONS_FIELD]
        .into_iter()
        .map(|field| state_input(field, field))
        .collect()
}

fn topology_input_fields() -> Result<Vec<InputField>, BuiltinTemplateError> {
    Ok(vec![
        (
            SCOUT_ROLES_FIELD,
            role_array_type(&SCOUT_ROLE_LABELS)?,
            true,
        ),
        (
            JUDGE_ROLES_FIELD,
            role_array_type(&JUDGE_ROLE_LABELS)?,
            true,
        ),
        (WORK_ITEMS_FIELD, role_array_type(&WORK_ITEM_LABELS)?, true),
    ])
}

fn bootstrap_output_type() -> Result<PayloadType, BuiltinTemplateError> {
    let mut fields = topology_input_fields()?;
    fields.extend([
        (
            CONTINUATION_ITEMS_FIELD,
            array_type(PayloadType::Null),
            true,
        ),
        (PROPOSALS_FIELD, array_type(PayloadType::String), true),
        (PLAN_REVIEWS_FIELD, array_type(PayloadType::String), true),
        (REVIEWS_FIELD, array_type(PayloadType::String), true),
        (VERDICTS_FIELD, array_type(verdict_type()?), true),
    ]);
    record_type(fields)
}

fn role_array_type(labels: &[&str]) -> Result<PayloadType, BuiltinTemplateError> {
    Ok(array_type(record_type(vec![(
        ROLE_FIELD,
        PayloadType::Enum {
            values: enum_labels(labels)?,
        },
        true,
    )])?))
}

fn proposal_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![("proposal", PayloadType::String, true)])
}

fn scout_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    role_input_type(&SCOUT_ROLE_LABELS)
}

fn selection_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (PROPOSALS_FIELD, array_type(PayloadType::String), true),
    ])
}

fn selection_input_bindings() -> Result<Vec<InputBinding>, BuiltinTemplateError> {
    [TASK_FIELD, PROPOSALS_FIELD]
        .into_iter()
        .map(|field| state_input(field, field))
        .collect()
}

fn verdict_signals(
    labels: &[&str],
) -> Result<BTreeMap<FieldName, NonEmptyEnumSet>, BuiltinTemplateError> {
    Ok(BTreeMap::from([(
        field_name(VERDICT_FIELD)?,
        enum_labels(labels)?,
    )]))
}

fn item_role_binding() -> Result<InputBinding, BuiltinTemplateError> {
    Ok(InputBinding {
        target: field_path(ROLE_FIELD)?,
        value: DataSelector::Item {
            path: field_path(ROLE_FIELD)?,
        },
    })
}

fn judge_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    role_input_type(&JUDGE_ROLE_LABELS)
}

fn role_input_type(labels: &[&str]) -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (
            ROLE_FIELD,
            PayloadType::Enum {
                values: enum_labels(labels)?,
            },
            true,
        ),
    ])
}

fn topology_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    let mut fields = topology_input_fields()?;
    fields.push((OPTIONS_FIELD, research_options_type()?, true));
    record_type(fields)
}

fn topology_input_bindings() -> Result<Vec<InputBinding>, BuiltinTemplateError> {
    [
        SCOUT_ROLES_FIELD,
        JUDGE_ROLES_FIELD,
        WORK_ITEMS_FIELD,
        OPTIONS_FIELD,
    ]
    .into_iter()
    .map(|field| state_input(field, field))
    .collect()
}

fn review_output_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![("review", PayloadType::String, true)])
}

fn research_review_signals() -> Result<BTreeMap<FieldName, NonEmptyEnumSet>, BuiltinTemplateError> {
    Ok(BTreeMap::from([(
        field_name(VERDICT_FIELD)?,
        research_verdict_labels()?,
    )]))
}

fn research_verdict_labels() -> Result<NonEmptyEnumSet, BuiltinTemplateError> {
    enum_labels(&RESEARCH_VERDICT_LABELS)
}

fn verdict_type() -> Result<PayloadType, BuiltinTemplateError> {
    Ok(PayloadType::Enum {
        values: research_verdict_labels()?,
    })
}

fn decision_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (REVIEWS_FIELD, array_type(PayloadType::String), true),
        (VERDICTS_FIELD, array_type(verdict_type()?), true),
        (PLAN_REVIEWS_FIELD, array_type(PayloadType::String), true),
    ])
}

fn decision_input_bindings() -> Result<Vec<InputBinding>, BuiltinTemplateError> {
    [
        TASK_FIELD,
        REVIEWS_FIELD,
        VERDICTS_FIELD,
        PLAN_REVIEWS_FIELD,
    ]
    .into_iter()
    .map(|field| state_input(field, field))
    .collect()
}

fn terminal_paths(delivery: TemplateDelivery) -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    auto_research_delivery_mode(delivery)?
        .map(delivery_output_paths)
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn checkpoint_paths() -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    let mut result = delivery_output_paths(DeliveryMode::Push)?;
    result.push(field_path(DELIVERY_FEEDBACK_FIELD)?);
    Ok(result)
}

fn delivery_output_paths(mode: DeliveryMode) -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    output_fields(&static_value(delivery_result_schema(mode))?)?
        .into_iter()
        .map(|field| field_path(&field))
        .collect()
}

fn paths(fields: &[&str]) -> Result<Vec<FieldPath>, BuiltinTemplateError> {
    fields.iter().map(|field| field_path(field)).collect()
}
