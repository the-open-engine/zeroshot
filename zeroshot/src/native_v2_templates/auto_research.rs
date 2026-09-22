//! Built-in bounded autoresearch graph.

use std::collections::BTreeMap;

use openengine_cluster_protocol::{DataSelector, InputBinding, RecordField};

use crate::native_v2_delivery::DELIVERY_PUSHED_LABEL;

use super::*;

pub(super) const AUTO_RESEARCH_ITERATIONS: u64 = 10;
const SCOUT_ROLES_FIELD: &str = "scoutRoles";
const JUDGE_ROLES_FIELD: &str = "judgeRoles";
const WORK_ITEMS_FIELD: &str = "workItems";
const ROLE_FIELD: &str = "role";
const PROPOSALS_FIELD: &str = "proposals";
const REVIEWS_FIELD: &str = "reviews";
const VERDICTS_FIELD: &str = "verdicts";
const ADOPT_LABEL: &str = "adopt";
const RECORD_ONLY_LABEL: &str = "record_only";
const ABORT_LABEL: &str = "abort";
const SCOUT_ROLE_LABELS: [&str; 3] = ["explorer", "synthesizer", "challenger"];
const JUDGE_ROLE_LABELS: [&str; 3] = ["evidence", "method", "progress"];
const RESEARCH_VERDICT_LABELS: [&str; 3] = [ADOPT_LABEL, RECORD_ONLY_LABEL, ABORT_LABEL];
const WORK_ITEM_LABELS: [&str; 1] = ["experiment"];

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
    Ok(GraphNode::Step(StepNode {
        name: node_name("bootstrap")?,
        worker: worker_ref("builtin.agent.research-bootstrap@1")?,
        instructions: Some(instructions(
            "Create or resume `.zeroshot/research` without changing candidate source files. The \
             directory is the durable memory for this task. Keep `charter.md`, `state.json`, \
             `summary.md`, and `backlog.json` at its root. Put finalized, append-only iteration records \
             under `iterations/NNNNNN/`; each record contains `proposal.json`, `experiment.json`, \
             `evaluations.json`, `decision.json`, and `artifacts.json`. Keep reversible backups under \
             `scratch/` and add that directory to the research `.gitignore`. Initialize `state.json` \
             with schema version 1, the next iteration number, the retained workspace identity, its \
             artifact hashes, invariant status, and open invariant violations, any task-specific adopted \
             measures, and the last finalized disposition. The charter defines the research \
             question, boundaries, protected material, evidence standard, resource limits, and stopping \
             rules from the caller's task. Distinguish non-negotiable charter invariants from optional \
             progress measures. Track any evidence that the retained workspace violates an invariant \
             in `state.json`, `summary.md`, and `backlog.json`; a verified repair of that violation takes \
             priority over optional improvement. Keep `summary.md` explicit about the retained workspace \
             identity and invariant status separately from the best supported historical findings; never \
             attribute a finding from a restored artifact to the retained workspace. Check that prior \
             finalized records agree with mutable state and summary. If an earlier process left a draft, \
             restore its backup and record an aborted \
             iteration before proceeding. Treat provider sessions as disposable; files are authoritative. \
             Do not use Git, do not rewrite a finalized iteration, and keep large logs or binaries out \
             of the committed research directory. Return exactly three ordered scout roles: explorer, \
             synthesizer, and challenger; three ordered judge roles: evidence, method, and progress; \
             one experiment work item; and empty proposals, reviews, and verdicts arrays using the \
             structured response fields.",
        )?),
        input: task_type()?,
        output: bootstrap_output_type()?,
        input_bindings: vec![state_input(TASK_FIELD, TASK_FIELD)?],
        write_bindings: vec![
            output_write("bootstrap", SCOUT_ROLES_FIELD, SCOUT_ROLES_FIELD)?,
            output_write("bootstrap", JUDGE_ROLES_FIELD, JUDGE_ROLES_FIELD)?,
            output_write("bootstrap", WORK_ITEMS_FIELD, WORK_ITEMS_FIELD)?,
            output_write("bootstrap", PROPOSALS_FIELD, PROPOSALS_FIELD)?,
            output_write("bootstrap", REVIEWS_FIELD, REVIEWS_FIELD)?,
            output_write("bootstrap", VERDICTS_FIELD, VERDICTS_FIELD)?,
        ],
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn after_bootstrap(
    route_state: PayloadType,
    research_state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let terminal = match auto_research_delivery_mode(delivery)? {
        Some(mode) => delivery_success("done", mode)?,
        None => succeed_null("done")?,
    };
    let research = sequence(
        "research",
        research_state.clone(),
        vec![research_loop(research_state, delivery)?, terminal],
        terminal_paths(delivery)?,
    )?;
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name("bootstrap_result")?,
        state: route_state,
        branches: non_empty(vec![ChoiceBranch {
            when: executable_error_guard("bootstrap")?,
            node: fail("bootstrap_failed", "bootstrap_failed")?,
        }])?,
        otherwise: Some(Box::new(research)),
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
    sequence(
        "research_iteration",
        state.clone(),
        vec![scout_stage(state, delivery)?],
        terminal_paths(delivery)?,
    )
}

fn scout_stage(
    state: PayloadType,
    delivery: TemplateDelivery,
) -> Result<GraphNode, BuiltinTemplateError> {
    let route = GraphNode::Choice(ChoiceNode {
        name: node_name("scout_result")?,
        state: state.clone(),
        branches: non_empty(vec![ChoiceBranch {
            when: mapped_error_guard("research_scout")?,
            node: recovery_worker(
                "abort_scouting",
                "At least one scout failed before the proposal set completed. Do not edit the \
                 workspace. Finalize the current iteration as aborted with the five standard JSON \
                 records, recording available evidence and missing roles. Update `state.json`, \
                 `summary.md`, and `backlog.json`, advance the next iteration number, and remove only \
                 scratch owned by this unfinished iteration. Keep prior finalized records unchanged, \
                 verify that the adopted workspace state is unchanged, and do not use Git. Return a \
                 Conventional Commit title and short checkpoint description.",
            )?,
        }])?,
        otherwise: Some(Box::new(after_scouts_phase(state.clone())?)),
        promoted_state_paths: Vec::new(),
    });
    let mut children = vec![scout_map(state.clone())?, route];
    if auto_research_delivery_mode(delivery)?.is_some() {
        children.push(checkpoint_manifest()?);
        children.push(checkpoint(state.clone(), "checkpoint")?);
    }
    sequence("scout_stage", state, children, terminal_paths(delivery)?)
}

fn scout_map(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
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

fn item_role_binding() -> Result<InputBinding, BuiltinTemplateError> {
    Ok(InputBinding {
        target: field_path(ROLE_FIELD)?,
        value: DataSelector::Item {
            path: field_path(ROLE_FIELD)?,
        },
    })
}

fn after_scouts_phase(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("research_phase")?,
        state: state.clone(),
        body: Box::new(after_scouts(state)?),
        over: DataSelector::State {
            path: field_path(WORK_ITEMS_FIELD)?,
        },
        max_items: positive(1)?,
        promoted_state_paths: Vec::new(),
    }))
}

fn after_scouts(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    sequence(
        "after_scouts",
        state.clone(),
        vec![experiment_phase(state.clone())?, judge_stage(state)?],
        Vec::new(),
    )
}

fn experiment_phase(state: PayloadType) -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Map(MapNode {
        name: node_name("experiment_phase")?,
        state: state.clone(),
        body: Box::new(sequence(
            "experiment_cycle",
            state,
            vec![selector()?, experiment()?],
            Vec::new(),
        )?),
        over: DataSelector::State {
            path: field_path(WORK_ITEMS_FIELD)?,
        },
        max_items: positive(1)?,
        promoted_state_paths: Vec::new(),
    }))
}

fn selector() -> Result<GraphNode, BuiltinTemplateError> {
    Ok(GraphNode::Step(StepNode {
        name: node_name("select_hypothesis")?,
        worker: worker_ref("builtin.agent.research-selector@1")?,
        instructions: Some(instructions(
            "Compare the three proposals against the charter and durable research record. Select the \
             bounded experiment with the best expected progress or uncertainty reduction. Reject \
             disguised repeats, unfalsifiable plans, and work that cannot finish in one iteration. Do \
             not change workspace artifacts or finalized records. Read the next iteration number from \
             `state.json` and write the chosen question and plan only to \
             `.zeroshot/research/scratch/NNNNNN/selection.json`; include protected paths or data, the \
             procedure, observations or sources to collect, comparison or reference checks when useful, \
             evaluation rules, resource limits, and conditions for adopt, record_only, or abort. Predeclare \
             evidence collection and evaluation order when observations may be noisy or order-dependent; \
             use charter-defined controls, repetitions, and thresholds rather than choosing them after \
             seeing results. Do not let an optional progress threshold reject a verified repair when the \
             ledger already shows that the retained workspace violates a charter invariant. In that case, \
             prioritize a bounded \
             repair or discriminating test, and judge repaired validity before optional improvement. Do \
             not use Git.",
        )?),
        input: selection_input_type()?,
        output: PayloadType::Null,
        input_bindings: vec![
            state_input(TASK_FIELD, TASK_FIELD)?,
            state_input(PROPOSALS_FIELD, PROPOSALS_FIELD)?,
        ],
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(1)?,
    }))
}

fn experiment() -> Result<GraphNode, BuiltinTemplateError> {
    task_step(
        "experiment",
        "builtin.agent.research-experimenter@1",
        "Read the durable state and current iteration's `scratch/NNNNNN/selection.json`. If it is absent \
         or incomplete, do not edit workspace artifacts; leave a draft explaining why the iteration \
         could not run. Otherwise execute exactly that experiment. Before changing the workspace, save \
         byte-for-byte originals under `scratch/NNNNNN/` and write a manifest covering modified, \
         deleted, and new paths. Never use Git. Gather evidence through the declared procedure. For an \
         executable artifact, compare prior and proposed states on the same inputs and environment; for \
         other research, retain source references, observations, and analysis steps that another judge \
         can inspect. Preserve all charter-protected material and respect its resource limits. Leave any \
         working changes in place for independent review. Write `iterations/NNNNNN/draft.json` with the \
         question, plan, commands or queries, exit status, observations, timings when relevant, source \
         and environment facts, changed-path hashes, limitations, and short excerpts; keep bulky output \
         in ignored scratch. Do not judge your own experiment or update mutable summary state or a \
         finalized iteration.",
    )
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
            "Read the current iteration's selection, draft, task, charter, ledger, and workspace. Review \
             independently and do not edit files. Evidence checks whether observations support the main \
             claims and repeats the decisive computation, source check, or comparison when possible. \
             Method audits design, controls, provenance, reproducibility, scope, protected material, \
             backups, and restoration. Progress compares the result with the charter and ledger; a valid \
             negative or inconclusive result can merit record_only when it removes a live direction or \
             reduces uncertainty. Adopt means the evidence permits retaining the working changes. \
             Record_only means the finding belongs in the ledger but workspace changes must be restored. \
             Abort means the evidence or method is invalid, incomplete, unsafe, or cannot support a \
             defensible finding. When prior evidence already proves that the retained workspace violates \
             a non-negotiable charter invariant, a verified repair merits adopt even if it does not improve \
             an optional measure. Do not choose record_only merely because that repair misses an optimization \
             threshold: restoring the known-invalid predecessor would violate the charter. Avoid \
             resource-heavy checks unless assigned evidence or the task cannot \
             be judged otherwise; the evidence role owns independent reproduction of the main measured \
             claim. Return only the assigned role's review and one of adopt, record_only, or abort.",
        )?),
    }))
}

#[derive(Clone, Copy)]
enum ResearchDisposition {
    Adopt,
    RecordOnly,
    Abort,
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
                when: research_judges_guard(1, ABORT_LABEL)?,
                node: decision_worker("abort_iteration", ResearchDisposition::Abort)?,
            },
            ChoiceBranch {
                when: research_judges_guard(3, ADOPT_LABEL)?,
                node: decision_worker("adopt_iteration", ResearchDisposition::Adopt)?,
            },
        ])?,
        otherwise: Some(Box::new(decision_worker(
            "record_iteration",
            ResearchDisposition::RecordOnly,
        )?)),
        promoted_state_paths: paths(&[TITLE_FIELD, DESCRIPTION_FIELD])?,
    }))
}

fn research_judges_guard(count: u64, label: &str) -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::KOfMap {
        count: positive(count)?,
        value: ControlSelector {
            name: node_name("research_judge")?,
            source: ControlSource::Signal,
            field: Some(field_name(VERDICT_FIELD)?),
        },
        labels: enum_labels(&[label])?,
    })
}

fn decision_worker(
    name: &str,
    disposition: ResearchDisposition,
) -> Result<GraphNode, BuiltinTemplateError> {
    let action = disposition_action(disposition);
    Ok(GraphNode::Step(StepNode {
        name: node_name(name)?,
        worker: worker_ref("builtin.agent.research-recorder@1")?,
        instructions: Some(instructions(&format!(
            "{action} Read the current iteration's scratch selection and experiment draft. Store the three \
             reviews and their verdicts in `evaluations.json`; write the graph-owned disposition and \
             reason to `decision.json`. Reconcile `state.json`, `summary.md`, and `backlog.json` so they \
             separately identify the retained workspace, its known invariant status and open violations, \
             and the best supported historical findings, including findings from restored artifacts. \
             Never attribute a historical finding or measure to the retained workspace unless hashes or \
             provenance match. Prior iteration directories are append-only. Keep large artifacts out of \
             Git and do not use Git commands. Return a Conventional Commit title and \
             a short description for this iteration checkpoint."
        ))?),
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

fn disposition_action(disposition: ResearchDisposition) -> &'static str {
    match disposition {
        ResearchDisposition::Adopt => {
            "All three judges chose adopt. Keep the working changes, finalize the draft as an adopted \
             iteration, remove its scratch backup, update the retained workspace identity, artifact hashes, \
             invariant status, open violations, and any task-specific accepted measures in `state.json`, \
             refresh the summary and backlog, and advance the next \
             iteration number."
        }
        ResearchDisposition::RecordOnly => {
            "No judge chose abort and at least one chose record_only. Restore every changed workspace \
             path byte-for-byte from the scratch manifest, remove paths created by the experiment, and \
             check the restored state. Preserve the retained workspace identity; update its known invariant \
             status and open violations when the supported finding concerns it. Finalize the draft as \
             record-only, retain the supported negative or inconclusive finding in the summary and \
             backlog, advance the next iteration number, and \
             remove scratch only after restoration is proven."
        }
        ResearchDisposition::Abort => {
            "At least one judge chose abort. Restore every changed workspace path byte-for-byte from \
             the scratch manifest, remove paths created by the experiment, and check the restored state. \
             Finalize the draft as aborted because its evidence or method was invalid or incomplete. Keep the \
             retained workspace identity and status based only on previously supported findings, preserve \
             the failure reason in the summary and backlog, advance the next iteration number, \
             and remove scratch only after restoration is proven."
        }
    }
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

fn checkpoint(state: PayloadType, prefix: &str) -> Result<GraphNode, BuiltinTemplateError> {
    let retry_name = format!("{prefix}_retry");
    let delivery_name = format!("{prefix}_delivery");
    let retry = GraphNode::Loop(LoopNode {
        name: node_name(&retry_name)?,
        state: state.clone(),
        body: Box::new(checkpoint_attempt(state.clone(), prefix)?),
        until: Some(named_delivery_signal_guard(
            &delivery_name,
            &[DELIVERY_PUSHED_LABEL],
        )?),
        max_iterations: positive(3)?,
        promoted_state_paths: checkpoint_paths()?,
    });
    sequence(
        prefix,
        state.clone(),
        vec![retry, checkpoint_completion(state, prefix)?],
        checkpoint_paths()?,
    )
}

fn checkpoint_completion(
    state: PayloadType,
    prefix: &str,
) -> Result<GraphNode, BuiltinTemplateError> {
    let retry_name = format!("{prefix}_retry");
    let completion_name = format!("{prefix}_completion");
    let exhausted_name = format!("{prefix}_attempts_exhausted");
    let complete_name = format!("{prefix}_complete");
    Ok(GraphNode::Choice(ChoiceNode {
        name: node_name(&completion_name)?,
        state,
        branches: non_empty(vec![ChoiceBranch {
            when: group_guard(&retry_name, "terminated", &["exhausted"])?,
            node: fail(&exhausted_name, "delivery_failed")?,
        }])?,
        otherwise: Some(Box::new(checkpoint_auditor(&complete_name)?)),
        promoted_state_paths: Vec::new(),
    }))
}

fn checkpoint_attempt(state: PayloadType, prefix: &str) -> Result<GraphNode, BuiltinTemplateError> {
    let delivery_name = format!("{prefix}_delivery");
    let result_name = format!("{prefix}_result");
    let failed_name = format!("{prefix}_failed");
    let repair_name = format!("{prefix}_repair");
    let observer_name = format!("{prefix}_observer");
    let attempt_name = format!("{prefix}_attempt");
    let route = GraphNode::Choice(ChoiceNode {
        name: node_name(&result_name)?,
        state: state.clone(),
        branches: non_empty(vec![
            ChoiceBranch {
                when: executable_error_guard(&delivery_name)?,
                node: fail(&failed_name, "delivery_failed")?,
            },
            ChoiceBranch {
                when: named_delivery_signal_guard(
                    &delivery_name,
                    &[DELIVERY_REPAIR_REQUIRED_LABEL],
                )?,
                node: named_delivery_repair(&repair_name, DeliveryMode::Push)?,
            },
        ])?,
        otherwise: Some(Box::new(checkpoint_auditor(&observer_name)?)),
        promoted_state_paths: Vec::new(),
    });
    sequence(
        &attempt_name,
        state,
        vec![
            named_delivery_node(&delivery_name, DeliveryMode::Push)?,
            route,
        ],
        checkpoint_paths()?,
    )
}

fn checkpoint_auditor(name: &str) -> Result<GraphNode, BuiltinTemplateError> {
    let input = static_value(delivery_result_schema(DeliveryMode::Push))?;
    let input_bindings = output_fields(&input)?
        .into_iter()
        .map(|field| state_input(&field, &field))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GraphNode::Verifier(VerifierNode {
        name: node_name(name)?,
        worker: worker_ref("builtin.agent.research-checkpoint@1")?,
        input,
        output: PayloadType::Null,
        input_bindings,
        write_bindings: Vec::new(),
        timeout_ms: None,
        attempts: positive(MAX_AGENT_VERIFIER_ATTEMPTS)?,
        signals: BTreeMap::new(),
        diagnostic: PayloadType::Null,
        instructions: Some(instructions(
            "Read only. Confirm that the push receipt names the exact finalized research checkpoint \
             currently present in the workspace and that no experiment draft or scratch backup is \
             being published. Do not edit files and do not use Git commands.",
        )?),
    }))
}

fn mapped_error_guard(node: &str) -> Result<Guard, BuiltinTemplateError> {
    Ok(Guard::KOfMap {
        count: positive(1)?,
        value: error_selector(node)?,
        labels: worker_error_labels()?,
    })
}

fn group_guard(node: &str, field: &str, labels: &[&str]) -> Result<Guard, BuiltinTemplateError> {
    control_guard(node, ControlSource::Group, Some(field), labels)
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
    add_role_fields(&mut fields, initialized)?;
    add_result_fields(&mut fields, initialized)?;
    Ok(fields)
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

fn add_role_fields(
    fields: &mut BTreeMap<FieldName, RecordField>,
    initialized: bool,
) -> Result<(), BuiltinTemplateError> {
    fields.insert(
        field_name(SCOUT_ROLES_FIELD)?,
        collection_field(role_array_type(&SCOUT_ROLE_LABELS)?, initialized),
    );
    fields.insert(
        field_name(JUDGE_ROLES_FIELD)?,
        collection_field(role_array_type(&JUDGE_ROLE_LABELS)?, initialized),
    );
    fields.insert(
        field_name(WORK_ITEMS_FIELD)?,
        collection_field(role_array_type(&WORK_ITEM_LABELS)?, initialized),
    );
    Ok(())
}

fn add_result_fields(
    fields: &mut BTreeMap<FieldName, RecordField>,
    initialized: bool,
) -> Result<(), BuiltinTemplateError> {
    fields.insert(
        field_name(PROPOSALS_FIELD)?,
        collection_field(array_type(PayloadType::String), initialized),
    );
    fields.insert(
        field_name(REVIEWS_FIELD)?,
        collection_field(array_type(PayloadType::String), initialized),
    );
    fields.insert(
        field_name(VERDICTS_FIELD)?,
        collection_field(array_type(verdict_type()?), initialized),
    );
    Ok(())
}

fn collection_field(value_type: PayloadType, initialized: bool) -> RecordField {
    if initialized {
        required(value_type)
    } else {
        optional(value_type)
    }
}

fn bootstrap_output_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
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
        (PROPOSALS_FIELD, array_type(PayloadType::String), true),
        (REVIEWS_FIELD, array_type(PayloadType::String), true),
        (VERDICTS_FIELD, array_type(verdict_type()?), true),
    ])
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

fn array_type(items: PayloadType) -> PayloadType {
    PayloadType::Array {
        items: Box::new(items),
    }
}

fn proposal_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![("proposal", PayloadType::String, true)])
}

fn scout_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (
            ROLE_FIELD,
            PayloadType::Enum {
                values: enum_labels(&SCOUT_ROLE_LABELS)?,
            },
            true,
        ),
    ])
}

fn selection_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (PROPOSALS_FIELD, array_type(PayloadType::String), true),
    ])
}

fn judge_input_type() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (
            ROLE_FIELD,
            PayloadType::Enum {
                values: enum_labels(&JUDGE_ROLE_LABELS)?,
            },
            true,
        ),
    ])
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
