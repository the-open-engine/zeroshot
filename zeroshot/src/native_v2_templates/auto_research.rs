//! Built-in bounded autoresearch graph.

use std::collections::BTreeMap;

use openengine_cluster_protocol::{DataSelector, InputBinding, RecordField};

use crate::native_v2_delivery::DELIVERY_PUSHED_LABEL;

use super::*;

pub(super) const AUTO_RESEARCH_ITERATIONS: u64 = 10;
const SCOUT_ROLES_FIELD: &str = "scoutRoles";
const JUDGE_ROLES_FIELD: &str = "judgeRoles";
const WORK_ITEMS_FIELD: &str = "workItems";
const CONTINUATION_ITEMS_FIELD: &str = "continuationItems";
const ROLE_FIELD: &str = "role";
const PROPOSALS_FIELD: &str = "proposals";
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
        task_type()?,
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
        REVIEWS_FIELD,
        VERDICTS_FIELD,
    ]
    .into_iter()
    .map(|field| output_write("bootstrap", field, field))
    .collect::<Result<Vec<_>, _>>()?;
    task_step_with_contract(
        "bootstrap",
        "builtin.agent.research-bootstrap@1",
        "Create or resume '.zeroshot/research' without changing candidate source files. The \
         directory is the durable memory for this task. Keep 'charter.md', 'state.json', \
         'summary.md', and 'backlog.json' at its root. Put finalized, append-only iteration records \
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
         and keep large logs or binaries out of the committed research directory. Return exactly three \
         ordered scout roles (explorer, synthesizer, challenger), exactly three ordered judge roles \
         (evidence, method, progress), exactly one experiment work item, and empty continuationItems, \
         proposals, reviews, and verdicts arrays. The graph validates role and work-item coverage before \
         dependent work and rejects missing, duplicate, extra, or failed entries.",
        bootstrap_output_type()?,
        write_bindings,
    )
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
             diagnostic. This preflight gates the iteration loop. Do not edit files.",
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
        promoted_state_paths: terminal_paths(delivery)?,
    }))
}

fn research_iteration(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let mut children = vec![scout_stage(state.clone())?, scout_result(state.clone())?];
    if auto_research_delivery_mode(delivery)?.is_some() {
        children.push(checkpoint(state.clone())?);
    }
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
             available evidence. Do not edit anything. Return one bounded proposal with its question or \
             hypothesis, expected knowledge or artifact change, exact scope, procedure, evidence needed, \
             risks, falsification condition, and relation to prior attempts. It must fit in one iteration.",
        )?),
    }))
}

fn scout_result(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("scout_result")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: mapped_error_guard("research_scout")?,
                node: recovery_stage(
                    state.clone(),
                    "abort_scouting",
                    "At least one scout failed before the proposal set completed. Do not edit the \
                     workspace. Finalize the current iteration as aborted with the five standard JSON \
                     records, recording available evidence and missing roles. Update 'state.json', \
                     'summary.md', and 'backlog.json', advance the next iteration number, and remove only \
                     scratch owned by this unfinished iteration. Keep prior finalized records unchanged, \
                     verify that the retained workspace state is unchanged, and do not use Git. Return a \
                     Conventional Commit title and short checkpoint description.",
                )?,
            },
            ChoiceBranch {
                when: control_guard(
                    "hypothesis_scouts",
                    ControlSource::Group,
                    Some("overflow"),
                    &["overflow"],
                )?,
                node: fail("scout_activation_overflow", "invalid_research_topology")?,
            },
        ])?,
        otherwise: Some(Box::new(research_phase(state.clone())?)),
        promoted_state_paths: Vec::new(),
    }))
}

fn research_phase(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("research_phase")?,
        state: state.clone(),
        body: Box::new(execution_stage(state)?),
        over: DataSelector::State {
            path: field_path(WORK_ITEMS_FIELD)?,
        },
        max_items: positive(1)?,
        promoted_state_paths: Vec::new(),
    }))
}

fn execution_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    let result = GraphNode::Choice(ChoiceNode {
        name: node_name("execution_result")?,
        state: state.clone(),
        branches: non_empty(vec![ChoiceBranch {
            when: Guard::Any {
                guards: non_empty(vec![
                    executable_error_guard("select_hypothesis")?,
                    executable_error_guard("experiment")?,
                ])?,
            },
            node: recovery_stage(
                state.clone(),
                "abort_execution",
                "The selector or experiment worker failed before independent review. Restore every \
                 workspace path covered by the current scratch manifest byte-for-byte, remove paths \
                 created by the experiment, and prove restoration against the prior state. Remove an \
                 incomplete selection draft and finalize the iteration as aborted with the five standard \
                 JSON records, preserving available evidence and the failure reason. Update state, summary, \
                 and backlog, advance the next iteration number, and remove this iteration's scratch only \
                 after restoration is proven. Keep prior finalized records unchanged and do not use Git. \
                 Return a Conventional Commit title and short checkpoint description.",
            )?,
        }])?,
        otherwise: Some(Box::new(judge_phase(state.clone())?)),
        promoted_state_paths: Vec::new(),
    });
    sequence(
        "execution_stage",
        state,
        vec![selector()?, experiment()?, result],
        Vec::new(),
    )
}

fn selector() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name("select_hypothesis")?,
        worker: worker_ref("builtin.agent.research-selector@1")?,
        instructions: Some(instructions(
            "Require exactly three proposals. If the collection is incomplete, do not write a selection; \
             return normally so graph-owned recovery can finalize the iteration. Otherwise compare the \
             three proposals against the charter and durable research record. Select the \
             bounded experiment with the best expected progress or uncertainty reduction. Reject \
             disguised repeats, unfalsifiable plans, and work that cannot finish in one iteration. Do \
             not change workspace artifacts or finalized records. Read the next iteration number from \
             'state.json' and write the chosen question and plan only to \
             '.zeroshot/research/scratch/NNNNNN/selection.json'; include protected paths or data, the \
             procedure, observations or sources to collect, comparison or reference checks when useful, \
             evaluation rules, resource limits, and conditions for adopt, record_only, or abort. Predeclare \
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
        "Read the durable state and current iteration's 'scratch/NNNNNN/selection.json'. If it is absent \
         or incomplete, do not edit workspace artifacts; leave a draft explaining why the iteration \
         could not run. Otherwise execute exactly that experiment. Before changing the workspace, save \
         byte-for-byte originals under 'scratch/NNNNNN/' and write a manifest covering modified, \
         deleted, and new paths. Never use Git. Gather evidence through the declared procedure. For an \
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
            "Act only as the assigned judge. Read the current iteration's selection, draft, task, charter, \
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

fn research_decision(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("research_decision")?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: mapped_error_guard("research_judge")?,
                node: recovery_stage(
                    state.clone(),
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
        ])?,
        otherwise: Some(Box::new(decision_stage(state.clone())?)),
        promoted_state_paths: paths(&[TITLE_FIELD, DESCRIPTION_FIELD])?,
    }))
}

fn decision_stage(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    finalizer_stage(state, "finalize_iteration", decision_worker()?)
}

fn mapped_error_guard(node: &str) -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::KOfMap {
        count: positive(1)?,
        value: error_selector(node)?,
        labels: worker_error_labels()?,
    })
}

fn decision_worker() -> Result<GraphNode, BuiltinTemplateError> {
    let name = "finalize_iteration";
    Ok(GraphNode::Step(StepNode {
        name: node_name(name)?,
        worker: worker_ref("builtin.agent.research-recorder@1")?,
        instructions: Some(instructions(
            "Apply the consensus rule mechanically to exactly three complete judge verdicts. If any \
             verdict is abort, restore every changed workspace path byte-for-byte from the scratch \
             manifest, remove paths created by the experiment, prove restoration, and finalize the draft \
             as aborted. If all three verdicts are adopt, keep the working changes and finalize it as \
             adopted. Otherwise, when at least one verdict is record_only and none is abort, restore and \
             prove the prior workspace, then finalize it as record-only. Never reinterpret a verdict. If \
             the reviews and verdicts are incomplete or inconsistent, restore and finalize as aborted. \
             Store the three reviews and verdicts in 'evaluations.json'; write the resulting disposition \
             and reason to 'decision.json'. Reconcile 'state.json', 'summary.md', and 'backlog.json' so they \
             separately identify the retained workspace, its known invariant status and open violations, \
             and the best supported historical findings, including findings from restored artifacts. \
             Never attribute a historical finding or measure to the retained workspace unless hashes or \
             provenance match. Update the retained workspace identity, hashes, invariant status, open \
             violations, and accepted measures only for an adopted result. Advance the next iteration \
             number and remove scratch only after any required restoration is proven. Prior iteration \
             directories are append-only. Keep large artifacts out of Git and do not use Git commands. \
             Return a Conventional Commit title and a short description for this iteration checkpoint.",
        )?),
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

fn recovery_stage(
    state: PayloadType,
    name: &str,
    authored_instructions: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    let worker = recovery_worker(name, authored_instructions)?;
    finalizer_stage(state, name, worker)
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

fn finalizer_stage(
    state: PayloadType,
    name: &str,
    worker: GraphNode,
) -> Result<GraphNode, BuiltinTemplateError> {
    let result_name = format!("{name}_result");
    let stage_name = format!("{name}_stage");
    let failed_name = format!("{name}_failed");
    let complete_name = format!("{name}_complete");
    let result = choice(
        &result_name,
        state.clone(),
        vec![ChoiceBranch {
            when: executable_error_guard(name)?,
            node: fail(&failed_name, "iteration_finalization_failed")?,
        }],
        Some(empty_continuation(&complete_name, state.clone())?),
    )?;
    sequence(&stage_name, state, vec![worker, result], Vec::new())
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
    let mut fields = research_string_fields()?;
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

fn research_string_fields() -> Result<BTreeMap<FieldName, RecordField>, BuiltinTemplateError> {
    let mut fields = BTreeMap::new();
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

fn item_role_binding() -> Result<InputBinding, BuiltinTemplateError> {
    Ok(InputBinding {
        target: field_path(ROLE_FIELD)?,
        value: DataSelector::Item {
            path: field_path(ROLE_FIELD)?,
        },
    })
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
    record_type(topology_input_fields()?)
}

fn topology_input_bindings() -> Result<Vec<InputBinding>, BuiltinTemplateError> {
    [SCOUT_ROLES_FIELD, JUDGE_ROLES_FIELD, WORK_ITEMS_FIELD]
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
