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

#[cfg(test)]
include!("auto_research_probes.rs");

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
         parents; immutable archive entries are append-only. Put the current provisional \
         iteration under 'iterations/NNNNNN/'; its five records contain 'proposal.json', 'experiment.json', \
         'evaluations.json', 'decision.json', and 'artifacts.json'. A successful auditor \
         appends 'audit.json' with continue or stop and any stop counterproposal. Keep reversible backups under \
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
         records and their audit outcomes agree with mutable state and summary. Use each \
         iteration's audit.json as the sole authority for its audit status; do not infer \
         acceptance from provisional records or summary prose. Never treat an \
         unaudited stop proposal as an approved stop. If an earlier process left any unaudited \
         iteration, draft, or unfinished backup, preserve its files and mutable state without \
         starting new work. The preflight will reject a fresh graph in that state; resume from a \
         checkpoint before the unfinished work or audit instead. Treat provider sessions as \
         disposable; files are authoritative. Do not use Git or rewrite an audited iteration; \
         and keep large logs or binaries out of the committed research directory. Read \
         'options.iterations' from input, using ten when absent. Admission has already checked \
         that an authored value is a positive safe integer and set the loop to that exact count. \
         Record the resolved value as 'iterationLimit' in 'state.json' and reject a resumed state \
         with a different value. Return exactly three ordered scout roles (explorer, synthesizer, \
         challenger), exactly three ordered judge roles (evidence, method, progress), exactly one \
         experiment work item, and empty continuationItems, proposals, reviews, and \
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
             iteration loop. Reject any prior iteration without audit.json, draft, or unfinished \
             scratch backup. Explain in the diagnostic that the operator should list the failed \
             run's checkpoints and resume from one before the unfinished work or audit; a fresh \
             graph must not scout past it. Do not edit files.",
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

fn early_terminal(
    name: &str,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    match auto_research_delivery_mode(delivery)? {
        Some(mode) => delivery_success(name, mode),
        None => succeed_null(name),
    }
}

fn exit_route(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("exit_route")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: Guard::Any {
                    guards: non_empty(vec![
                        mapped_signal_guard("audit_disposition", 1, &["stop"])?,
                        mapped_signal_guard("audit_disposition_recheck", 1, &["stop"])?,
                    ])?,
                },
                node: early_terminal("done_early", delivery)?,
            },
            ChoiceBranch {
                when: Guard::Any {
                    guards: non_empty(vec![
                        mapped_signal_guard("audit_disposition", 1, &["continue"])?,
                        mapped_signal_guard("audit_disposition_recheck", 1, &["continue"])?,
                    ])?,
                },
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
        vec![planner()?, research_route(state)?],
        Vec::new(),
    )
}

fn research_route(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("research_route")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: Guard::Any {
                    guards: non_empty(vec![
                        executable_error_guard("plan_experiment")?,
                        signal_guard("plan_experiment", VERDICT_FIELD, &["abort"])?,
                    ])?,
                },
                node: recovery_stage(
                    state.clone(),
                    "abort_plan",
                    "The planner failed or could not make a valid selection. Leave the incumbent \
                     intact, finalize an aborted iteration with the five standard records and the \
                     actual error or diagnostic, and advance once. Do not invent a selection, \
                     experiment, or judge verdict. Do not use Git.",
                )?,
            },
            ChoiceBranch {
                when: signal_guard("plan_experiment", VERDICT_FIELD, &["stop"])?,
                node: stop_stage(state.clone())?,
            },
            ChoiceBranch {
                when: signal_guard("plan_experiment", VERDICT_FIELD, &["work"])?,
                node: work_phase(state.clone())?,
            },
        ])?,
        otherwise: None,
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

fn stop_stage(_state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    research_recorder(
        "record_stop",
        "The planner proposed stop. Do not change candidate files. Finalize this iteration \
         with the five standard records. decision.json records disposition stop as a proposal, \
         not an approved campaign termination. Copy the selection and its challenged alternatives \
         into proposal.json before removing scratch; evaluations.json contains no judge claims. \
         artifacts.json proves the unchanged retained incumbent hashes. Update state, summary, \
         and backlog once. The independent disposition auditor decides whether to stop or continue \
         and writes its own audit.json. Remove scratch only after finalization. Do not use Git.",
    )
}

fn staging_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let route = GraphNode::Choice(ChoiceNode {
        name: node_name("staging_result")?,
        state: state.clone(),
        branches: non_empty(vec![ChoiceBranch {
            when: executable_error_guard("stage_parent")?,
            node: recovery_stage(
                state.clone(),
                "abort_staging",
                "Staging failed before a usable experiment. Restore the retained incumbent from \
                 scratch/NNNNNN/incumbent when its verified backup exists; otherwise prove the \
                 original workspace unchanged against retained hashes and absence inventory. If \
                 neither can be proven, report failure. Finalize an aborted iteration with the five \
                 standard records and available staging evidence, then advance once. Do not use Git.",
            )?,
        }])?,
        otherwise: Some(Box::new(experiment_stage(state.clone())?)),
        promoted_state_paths: Vec::new(),
    });
    sequence(
        "staging_stage",
        state,
        vec![stage_parent()?, route],
        Vec::new(),
    )
}

fn stage_parent() -> Result<GraphNode, BuiltinTemplateError> {
    task_step(
        "stage_parent",
        "builtin.agent.research-stage@1",
        "Read selection.json and check the selected parent before changing candidate files. \
         The manifest's declared candidate paths and files are the restorable source; generated \
         files recorded as outside candidate scope are evidence, not required archive bytes. \
         Check every declared file's hash, type, mode, and symlink target, and the complete \
         candidate path and absence inventory. Remove only clearly incidental generated files \
         outside that inventory, such as bytecode created by an archive read, and record the \
         cleanup. Refuse any other unlisted archive file. Never rewrite a declared archive \
         file or manifest; fail if its identity cannot \
         be established. Save the complete retained incumbent mutable scope byte-for-byte under \
         scratch/NNNNNN/incumbent and record its inventory and hashes before switching parents. \
         If parentArtifactId is incumbent, leave it in place. Otherwise restore the exact \
         archived parent files from archive/<id>/files using its scope and absence inventory. \
         Verify staged bytes and protected paths, then write scratch/NNNNNN/staging.json with \
         selected and incumbent IDs, hashes, changed paths, and restoration instructions. \
         Resolve routine staging housekeeping yourself; do not use Git.",
    )
}

fn experiment_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let result = GraphNode::Choice(ChoiceNode {
        name: node_name("experiment_result")?,
        state: state.clone(),
        branches: non_empty(vec![ChoiceBranch {
            when: executable_error_guard("experiment")?,
            node: recovery_stage(
                state.clone(),
                "abort_experiment",
                "The experiment execution failed, so do not judge its draft. Restore the \
                 pre-iteration incumbent byte-for-byte from scratch/NNNNNN/incumbent whenever \
                 that verified backup exists, including changes made when staging an archived \
                 parent. If staging failed before backup, prove the original workspace unchanged. \
                 Remove paths absent from the incumbent inventory and prove its hashes. Finalize \
                 an aborted iteration with the five standard records using available planner and \
                 experiment evidence. \
                 Record that no judges ran; never fabricate verdicts or a reviewed candidate. \
                 Advance state, summary, and backlog exactly once and remove scratch only after \
                 restoration and finalization are proven. If restoration cannot be proven, \
                 report failure. Do not use Git.",
            )?,
        }])?,
        otherwise: Some(Box::new(review_stage(state.clone())?)),
        promoted_state_paths: Vec::new(),
    });
    sequence(
        "experiment_stage",
        state,
        vec![experiment()?, result],
        Vec::new(),
    )
}

fn planner() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name("plan_experiment")?,
        worker: worker_ref("builtin.agent.research-planner@1")?,
        instructions: Some(instructions(
            "Require three fresh scout proposals tagged with the current iteration; if any are \
             missing, stale, or failed, write no valid selection and signal abort. Read the task, \
             charter, state, summary, backlog, immutable candidate archive, finalized records, \
             including the latest audit.json, and current incumbent. Consider refinement of the \
             incumbent, the strongest live archived branch, and fresh directions when warranted; \
             do not force a fixed schema of alternatives. Compare expected progress and information \
             gain against cost, uncertainty, risk, and diminishing returns. When the two most \
             recent audited iterations did not adopt a candidate, explicitly compare a new \
             mechanism, a restorable archived branch, and a bounded measurement or discriminating \
             test. Weigh expected gain and information against cost, risk, and observed noise; \
             explain when a category has no affordable concrete option. Record this comparison in \
             the selection. This trigger does not require a parent switch or impose an exploration \
             quota. Choose one falsifiable experiment that fits this iteration; sustained work on \
             the best direction is valid when its expected value remains highest. Do not impose a \
             novelty quota or repeat a failed direction without new evidence. Choose both a proposal \
             and its starting artifact. \
             Set parentArtifactId to incumbent or an immutable restorable archive ID; never select \
             a nonrestorable idea as a parent. Do not change workspace artifacts, the backlog, or \
             finalized records. Read the next iteration number from state.json and write only \
             .zeroshot/research/scratch/NNNNNN/selection.json. For work, name parentArtifactId, \
             selected proposal, and reason to use that parent; signal work only after writing a \
             complete selection. For stop, explain why the strongest concrete affordable scout or \
             archived alternative fails charter cost and risk limits; signal stop only after writing \
             the stop selection. For an incomplete or invalid handoff, signal abort with a diagnostic. \
             Record the three scouts, other considered alternatives, evidence references, expected \
             value, costs, and why the selected direction wins now. Include the chosen question, \
             expected learning or artifact change, falsification condition, protected paths or data, \
             procedure, observations or sources to collect, comparison or reference checks when \
             useful, evaluation rules, resource limits, and disposition conditions. Predeclare \
             evidence collection and evaluation order when observations may be noisy or order-dependent; \
             use charter-defined controls, repetitions, and thresholds rather than choosing them after \
             seeing results. Before work, state what evidence would make the candidate a better \
             retained default than the incumbent under the charter, and what would instead warrant \
             record_only. Ground those conditions in the task's intended use and priorities, considering \
             benefit magnitude and coverage, costs, regressions, complexity, and uncertainty where \
             relevant. A valid finding can merit archiving without displacing the incumbent. Do not \
             invent a universal score or fixed adoption threshold. Do not let an optional progress \
             threshold reject a verified repair when \
             the ledger already shows that the retained workspace violates a charter invariant. In \
             that case prioritize a bounded repair or discriminating test, and judge repaired validity \
             before optional improvement. Do not use Git.",
        )?),
        input: selection_input_type()?,
        output: PayloadType::Null,
        input_bindings: selection_input_bindings()?,
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(1)?,
        signals: verdict_signals(&["work", "stop", "abort"])?,
        diagnostic: diagnostic_type()?,
    }))
}

fn experiment() -> Result<GraphNode, BuiltinTemplateError> {
    task_step(
        "experiment",
        "builtin.agent.research-experimenter@1",
        "Read the planner selection and staging.json. Independently verify staged source \
         hashes, candidate scope, incumbent backup, and protected paths. If the handoff is invalid, \
         do not edit candidate files; leave a draft that marks the staging failure and names the \
         missing or mismatched evidence so judges can abort. Otherwise execute exactly that \
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
            "Act only as the assigned judge. Return abort for invalid staging or missing or \
             incomplete experiment output. Read the \
                current iteration's selection, draft, task, charter, \
             ledger, and workspace. Review independently and do not edit files. Evidence checks whether \
             observations support the main claims and repeats the decisive computation, source check, or \
             comparison when possible. Method audits design, controls, provenance, reproducibility, scope, \
             protected material, backups, and restoration. Progress decides whether the candidate should \
             replace the retained incumbent as the default for the next iteration under the charter. \
             Compare benefit magnitude and coverage of intended use with costs, regressions, complexity, \
             and confidence from relevant controls and counterexamples, as applicable to the task. Check \
             the planner's predeclared disposition conditions against the results, allowing a different \
             conclusion when new evidence supports it; do not invent a universal score or fixed adoption \
             threshold. Evidence and method return adopt when their own checks support retaining the \
             candidate. Progress returns adopt only when the candidate is a defensibly better default. \
             Record_only preserves a valid negative, inconclusive, or promising finding while restoring \
             the incumbent; a narrow gain alone does not require replacing it. Abort means the evidence \
             or method is invalid, incomplete, unsafe, or cannot support a defensible finding. When prior \
             evidence already proves that the retained workspace violates \
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
                    "An independent judge failed. Restore the pre-iteration incumbent byte-for-byte \
                     from scratch/NNNNNN/incumbent, including changes made when staging an archived \
                     parent. Remove paths absent from the incumbent inventory and prove its hashes. \
                     Finalize the iteration as aborted with the five standard JSON records, \
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
         disposition plus its reason to 'decision.json'. Before removing scratch, copy the \
         planner selection and considered alternatives into proposal.json. Preserve the \
         changed-path manifest, selected parent ID and hashes, pre-iteration incumbent, \
         reviewed-candidate, and final-current hashes in 'artifacts.json' so restoration or \
         retention remains independently auditable. For adopt, archive the displaced incumbent \
         if it remains a credible alternative; for record_only, retain a restorable reviewed \
         candidate only when its complete files and revisit reason are documented. Never overwrite \
         an archive entry. Put actual candidate files under archive/<id>/files and reference an \
         immutable manifest from the backlog; mark incomplete or oversize candidates \
         nonrestorable. Only declared candidate files belong in the restorable archive. Keep \
         generated files outside it and remove incidental files left by inspection. Reconcile \
         'state.json', 'summary.md', and 'backlog.json' so they separately identify the retained \
         workspace, its known invariant status and open violations, and the best supported historical \
         findings, including findings from restored artifacts. Describe the finalized disposition \
         in summary.md without asserting the current audit outcome or leaving a pending-audit \
         claim that will become stale. An iteration's audit.json alone determines its audit status. \
         Keep all scouts and considered alternatives in proposal.json; index actionable unresolved \
         directions in the backlog with evidence and useful next tests. Preserve \
         promising branches even when another experiment was chosen. Never attribute a historical finding or \
         measure to the retained workspace unless hashes or provenance match. Update the retained \
         workspace identity, hashes, invariant status, open violations, and accepted measures only for \
         an adopted result. Advance the next iteration number and remove scratch only after any required \
         restoration is proven. Before returning, resolve every evidence reference newly written \
         in the five current records and mutable state, backlog, and summary, including file paths \
         and JSON fragment anchors, against the post-cleanup ledger. Replace links to removed drafts \
         with finalized evidence; preserve immutable archive provenance. Earlier audited iteration \
         directories are append-only. Current \
         records are provisional until audit acceptance; correct only bookkeeping and \
         provenance supported by existing evidence after rejection. Keep large artifacts out \
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
                when: signal_guard(name, VERDICT_FIELD, &["rejected"])?,
                node: sequence(
                    "audit_repair_stage",
                    state.clone(),
                    vec![
                        audit_repair()?,
                        decision_auditor("audit_disposition_recheck")?,
                        audit_recheck_result(state.clone())?,
                    ],
                    Vec::new(),
                )?,
            },
            ChoiceBranch {
                when: signal_guard(name, VERDICT_FIELD, &["continue", "stop"])?,
                node: empty_continuation("audit_disposition_accepted", state.clone())?,
            },
        ])?,
        otherwise: None,
        promoted_state_paths: Vec::new(),
    });
    sequence(
        "audit_disposition_stage",
        state,
        vec![decision_auditor(name)?, result],
        Vec::new(),
    )
}

fn audit_recheck_result(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("audit_disposition_recheck_result")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: executable_error_guard("audit_repair")?,
                node: fail(
                    "audit_repair_failed",
                    "iteration_finalization_repair_failed",
                )?,
            },
            ChoiceBranch {
                when: executable_error_guard("audit_disposition_recheck")?,
                node: fail(
                    "audit_disposition_recheck_failed",
                    "iteration_finalization_audit_failed",
                )?,
            },
            ChoiceBranch {
                when: signal_guard("audit_disposition_recheck", VERDICT_FIELD, &["rejected"])?,
                node: fail(
                    "audit_disposition_recheck_rejected",
                    "iteration_finalization_rejected",
                )?,
            },
            ChoiceBranch {
                when: signal_guard(
                    "audit_disposition_recheck",
                    VERDICT_FIELD,
                    &["continue", "stop"],
                )?,
                node: empty_continuation("audit_disposition_recheck_accepted", state)?,
            },
        ])?,
        otherwise: None,
        promoted_state_paths: Vec::new(),
    }))
}

fn audit_repair() -> Result<GraphNode, BuiltinTemplateError> {
    task_step(
        "audit_repair",
        "builtin.agent.research-audit-repair@1",
        "Read the latest auditor rejection and scratch/NNNNNN/audit-feedback.json. Fix the \
         complete set of repairable issues in the current unaudited iteration, mutable state, \
         summary, backlog, and incidental generated files. Recheck source hashes, archive \
         manifests, protected paths, and the graph-selected verdict after each correction. \
         Correct only transcription, references, inventories, and metadata supported by \
         existing evidence. Never change measurements, experiment observations, planner selection, \
         an earlier audited record, declared archived source bytes, immutable manifest, judge \
         verdict, or graph-selected disposition. A scout already recorded in proposal.json \
         need not be duplicated in the backlog unless it is an actionable unresolved lead. \
         If a declared artifact or retained workspace cannot be proven, leave the evidence and \
         report failure rather than inventing it. Remove audit-feedback.json after a completed \
         repair. Do not use Git.",
    )
}

fn decision_auditor(name: &str) -> Result<GraphNode, BuiltinTemplateError> {
    task_reviewer(
        name,
        "builtin.agent.research-disposition-auditor@1",
        verdict_signals(&["continue", "stop", "rejected"])?,
        "Inspect exactly the current unaudited iteration, not an older audited directory. \
         Derive its number as the successor of the latest audited iteration, starting at one. \
         Require that new directory to contain all five current records, have no prior audit.json, \
         and match state.json nextIteration minus one; older records cannot substitute for it. \
         A settled recorder or recovery worker error does not invalidate complete, independently \
         proven files; incomplete or inconsistent files require rejection. Inspect read-only \
         before writing an audit or rejection feedback. Recompute the required disposition from \
         the path that actually ran. A planner stop \
         requires disposition stop, no experiment or judge claims, an unchanged incumbent, \
         and a concrete challenge of affordable alternatives. A failed plan, staging, experiment, \
         or judge requires abort and no fabricated evaluations. For three valid judge verdicts, \
         any abort requires abort, three adopts require adopt, and every other combination requires \
         record_only. Reject duplicate, extra, stale, or inconsistent evaluations. Confirm the \
         five current iteration records agree with decision.json, state, and summary; confirm \
         state advances exactly once and earlier audited records are unchanged. Treat existing \
         audit.json files as the sole authority for audit status, and reject summary wording that \
         conflicts with them or would falsely remain pending after this audit is appended. The current \
         records are provisional until audit acceptance; any repair must preserve original \
         observations, selection, and judge verdicts. Proposal.json preserves the full scout \
         handoff; backlog indexes actionable unresolved \
         directions without copying every scout. Check evidence references, parent identity, \
         disposition, archive IDs, retained workspace hashes, and protected paths. Require no \
         draft or incumbent \
         scratch backup to remain. The manifest's declared candidate files are the restorable \
         archive; generated files recorded outside candidate scope are not required archive bytes. \
         Verify complete mutable scope, absence inventory, provenance, file types, modes, \
         symlink targets, and hashes of every declared archive file. Inspect for undeclared \
         source files; treat incidental generated files as repairable contamination. For a \
         completed experiment, artifacts.json \
         must contain changed paths, selected parent, pre-iteration incumbent, reviewed candidate, \
         and final-current hashes. For a failed experiment, require selected parent, \
         pre-iteration incumbent, known changed paths, explicit absence of a reviewed candidate, \
         and final-current hashes. For stop or pre-experiment recovery, require explicit absent \
         candidate fields and hashes of the unchanged or restored incumbent. For abort or \
         record_only, require byte-for-byte equality with the pre-iteration incumbent, \
         restored deleted paths, and no candidate-created paths. For adopt, require current \
         hashes to match the reviewed candidate and retained identity to advance consistently. \
         If valid, challenge a proposed stop against the strongest affordable alternative and \
         append iterations/NNNNNN/audit.json with continue or stop, rationale, and any counterproposal. \
         If invalid, return rejected with an actionable diagnostic and write \
         scratch/NNNNNN/audit-feedback.json naming the exact issues and safe repair scope. \
         Do not edit reviewed records, candidate files, or earlier audit files. Do not use Git.",
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
                node: empty_continuation("checkpoint_accepted", state.clone())?,
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
    ])
}

fn decision_input_bindings() -> Result<Vec<InputBinding>, BuiltinTemplateError> {
    [TASK_FIELD, REVIEWS_FIELD, VERDICTS_FIELD]
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
    delivery_output_paths(DeliveryMode::Push)
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
