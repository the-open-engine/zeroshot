fn assert_research_scouts(iteration_nodes: &[&GraphNode]) {
    let scouts = iteration_nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Map(node) if node.name.as_str() == "hypothesis_scouts" => Some(node),
            _ => None,
        })
        .assert_value_with("independent scouts");
    assert_eq!(scouts.max_items.get(), 3);
    assert_eq!(scouts.body.name().as_str(), "research_scout");
    assert_eq!(
        scouts.promoted_state_paths,
        vec![field_path("proposals").assert_value()]
    );
    let scout = assert_role_verifier(
        iteration_nodes,
        "research_scout",
        "builtin.agent.research-scout@1",
        &["explorer", "synthesizer", "challenger"],
    );
    assert!(scout.instructions.as_ref().is_some_and(|text| {
        text.as_str().contains("Act only in the assigned role")
            && text.as_str().contains("nextIterationNumber")
            && text.as_str().contains("Do not use Git commands")
    }));
}

fn assert_research_planner(iteration_nodes: &[&GraphNode]) {
    let planner = find_verifier(iteration_nodes, "plan_experiment");
    assert_eq!(planner.worker.as_str(), "builtin.agent.research-planner@1");
    assert!(planner.instructions.as_ref().is_some_and(|value| {
        value.as_str().contains("retained workspace violates a charter invariant")
            && value.as_str().contains("expected progress or information value")
            && value.as_str().contains("considered alternatives")
            && value.as_str().contains("parentArtifactId")
            && value.as_str().contains("one invalid scout does not invalidate")
            && value.as_str().contains("never infer or fabricate partial outputs")
            && value.as_str().contains("verified directions in the durable")
            && value.as_str().contains("Tie every adoption veto")
            && value.as_str().contains("continuing with the strongest concrete viable")
            && value.as_str().contains("nextIterationNumber")
    }));
    assert_eq!(
        planner.signals.get(&field_name(VERDICT_FIELD).assert_value()),
        Some(&enum_labels(&["work", "stop", "abort"]).assert_value())
    );
    assert!(iteration_nodes.iter().all(|node| node.name().as_str() != "plan_review"));
    let selection = find_choice(iteration_nodes, "research_route");
    assert_eq!(selection.branches.as_slice()[1].node.name().as_str(), "record_stop");
    assert_eq!(selection.branches.as_slice()[2].node.name().as_str(), "work_phase");
    let staging = find_choice(iteration_nodes, "staging_result");
    assert_eq!(staging.branches.as_slice().len(), 1);
    assert_eq!(staging.branches.as_slice()[0].node.name().as_str(), "abort_staging");
    assert_eq!(
        staging.otherwise.as_ref().map(|node| node.name().as_str()),
        Some("experiment_stage")
    );
    let experiment = find_choice(iteration_nodes, "experiment_result");
    assert_eq!(experiment.branches.as_slice()[0].node.name().as_str(), "abort_experiment");
    assert_eq!(
        experiment.otherwise.as_ref().map(|node| node.name().as_str()),
        Some("judge_phase")
    );
    assert!(iteration_nodes
        .iter()
        .all(|node| node.name().as_str() != "staging_review"));
    let exit_route = find_choice(iteration_nodes, "exit_route");
    assert_eq!(exit_route.branches.as_slice()[0].node.name().as_str(), "done_early");
    assert_eq!(
        exit_route.branches.as_slice()[1].node.name().as_str(),
        "iteration_continues"
    );
    assert_eq!(exit_route.branches.as_slice().len(), 2);
}

fn settle_audit_dispatch(
    reduction: &Reduction,
    position: u64,
    outcome: openengine_cluster_protocol::WorkerOutcome,
) -> DurableExecution {
    use crate::full_v1_reducer::{DurableExecutionState, HistoryPosition};
    let (node_instance, execution, occurrence, attempt, input) = reduction
        .decisions
        .iter()
        .find_map(|decision| match decision {
            Decision::Dispatch {
                node_instance,
                execution,
                occurrence,
                attempt,
                input,
                ..
            } => Some((
                *node_instance,
                *execution,
                occurrence.clone(),
                *attempt,
                input.clone(),
            )),
            _ => None,
        })
        .assert_value_with("next audit dispatch");
    DurableExecution {
        dispatch_position: HistoryPosition::new(position - 1).assert_value(),
        node_instance,
        execution,
        occurrence,
        attempt,
        input,
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(position).assert_value(),
            outcome,
        },
    }
}

fn audit_review_outcome(verdict: &str) -> openengine_cluster_protocol::WorkerOutcome {
    use openengine_cluster_protocol::WorkerOutcome;
    WorkerOutcome::Verifier {
        output: Value::Null,
        signals: std::collections::BTreeMap::from([(
            field_name(VERDICT_FIELD).assert_value(),
            enum_label(verdict).assert_value(),
        )]),
        diagnostic: json!({"message":"repairable ledger issue"}),
        artifacts: Vec::new(),
    }
}

fn audit_probe_input() -> Value {
    json!({
        "task": "audit a research iteration",
        "workItems": [null],
        "continuationItems": []
    })
}

fn assert_research_dispatch(reduction: &Reduction, expected: &str) {
    assert!(reduction.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == expected
    )));
}

async fn verified_research_probe(
    authored: openengine_cluster_protocol::GraphSpec,
    input: &Value,
    title: &str,
    key: &str,
) -> VerifiedGraph {
    let runtime = runtime_for(
        BuiltinGraphTemplate::AutoResearch,
        TemplateDelivery::None,
        &executable_leaves(&authored.root),
    );
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            title: RunTitle::new(title).assert_value(),
            graph: authored,
            initial_input: input.clone(),
            runtime,
            environment: None,
            source: resolved_source(),
            submission_key: IdempotencyKey::new(key).assert_value(),
        })
        .await
        .unwrap_or_else(|error| panic!("research probe admission: {error:?}"));
    VerifiedGraph {
        compiled_ir: admitted.graph,
        diagnostics: Vec::new(),
    }
}

fn rejected_audit_recheck(
    verified: &VerifiedGraph,
    input: &Value,
    first: &Reduction,
) -> (Vec<DurableExecution>, Reduction) {
    let mut history = vec![settle_audit_dispatch(
        first,
        1,
        audit_review_outcome("rejected"),
    )];
    let repair = reduce(verified, input, &history);
    assert_research_dispatch(&repair, "audit_repair");
    history.push(settle_audit_dispatch(
        &repair,
        2,
        openengine_cluster_protocol::WorkerOutcome::Verified {
            output: Value::Null,
            artifacts: Vec::new(),
        },
    ));
    let recheck = reduce(verified, input, &history);
    assert_research_dispatch(&recheck, "audit_disposition_recheck");
    (history, recheck)
}

#[tokio::test]
async fn rejected_research_audit_repairs_then_rechecks_before_continuing() {
    let authored = super::auto_research::audit_probe_graph().assert_value();
    let input = audit_probe_input();
    let verified = verified_research_probe(authored, &input, "Audit repair routing", "audit-repair-routing").await;

    let first = reduce(&verified, &input, &[]);
    assert_research_dispatch(&first, "audit_disposition");
    let accepted_history = vec![settle_audit_dispatch(
        &first,
        1,
        audit_review_outcome("continue"),
    )];
    let accepted = reduce(&verified, &input, &accepted_history);
    assert!(matches!(
        accepted.terminal,
        Some(TerminalProjection::Succeeded { output }) if output.is_null()
    ));
    assert!(accepted.decisions.iter().all(|decision| !matches!(
        decision,
        Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "audit_repair"
    )));
    let (mut history, recheck) = rejected_audit_recheck(&verified, &input, &first);
    history.push(settle_audit_dispatch(
        &recheck,
        3,
        audit_review_outcome("continue"),
    ));

    let final_state = reduce(&verified, &input, &history);
    assert!(matches!(
        final_state.terminal,
        Some(TerminalProjection::Succeeded { output }) if output.is_null()
    ));

    let mut rejected_history = history[..2].to_vec();
    rejected_history.push(settle_audit_dispatch(
        &recheck,
        3,
        audit_review_outcome("rejected"),
    ));
    assert_eq!(
        reduce(&verified, &input, &rejected_history).terminal,
        Some(TerminalProjection::Failed {
            reason: "iteration_finalization_rejected".parse().assert_value()
        })
    );
}

#[tokio::test]
async fn current_audit_stop_overrides_a_prior_iteration_recheck() {
    let authored = super::auto_research::audit_loop_probe_graph().assert_value();
    let input = audit_probe_input();
    let verified = verified_research_probe(authored, &input, "Audit loop routing", "audit-loop-routing").await;

    let first = reduce(&verified, &input, &[]);
    assert_research_dispatch(&first, "audit_disposition");
    let (mut history, recheck) = rejected_audit_recheck(&verified, &input, &first);
    history.push(settle_audit_dispatch(
        &recheck,
        3,
        audit_review_outcome("continue"),
    ));

    let second = reduce(&verified, &input, &history);
    assert_research_dispatch(&second, "audit_disposition");
    history.push(settle_audit_dispatch(
        &second,
        4,
        audit_review_outcome("stop"),
    ));

    let finished = reduce(&verified, &input, &history);
    assert!(matches!(
        finished.terminal,
        Some(TerminalProjection::Succeeded { output }) if output.is_null()
    ));
    assert!(finished.decisions.iter().all(|decision| !matches!(
        decision,
        Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "audit_disposition"
    )));
}

#[tokio::test]
async fn experiment_crash_routes_to_recovery_without_judging() {
    let authored = super::auto_research::experiment_failure_probe_graph().assert_value();
    let input = json!({
        "task": "research a bounded improvement",
        "workItems": [{"role":"experiment"}],
        "judgeRoles": [
            {"role":"evidence"},
            {"role":"method"},
            {"role":"progress"}
        ],
        "reviews": [],
        "verdicts": [],
        "title": "",
        "description": ""
    });
    let verified = verified_research_probe(
        authored,
        &input,
        "Experiment failure routing",
        "experiment-failure-routing",
    )
    .await;

    let first = reduce(&verified, &input, &[]);
    assert_research_dispatch(&first, "experiment");
    let history = vec![settle_audit_dispatch(
        &first,
        1,
        openengine_cluster_protocol::WorkerOutcome::declared_failure(
            openengine_cluster_protocol::WorkerErrorCode::Crash,
        ),
    )];
    let recovery = reduce(&verified, &input, &history);
    assert_research_dispatch(&recovery, "abort_experiment");
    assert!(recovery.decisions.iter().all(|decision| !matches!(
        decision,
        Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "research_judge"
    )));
}

#[tokio::test]
async fn recorder_error_reaches_independent_audit() {
    let authored = super::auto_research::recorder_audit_probe_graph().assert_value();
    let input = json!({
        "task": "audit durable research records",
        "workItems": [null],
        "continuationItems": [],
        "reviews": [],
        "verdicts": [],
        "title": "",
        "description": ""
    });
    let verified = verified_research_probe(authored, &input, "Recorder audit routing", "recorder-audit-routing").await;

    let first = reduce(&verified, &input, &[]);
    assert_research_dispatch(&first, "record_stop");
    let history = vec![settle_audit_dispatch(
        &first,
        1,
        openengine_cluster_protocol::WorkerOutcome::declared_failure(
            openengine_cluster_protocol::WorkerErrorCode::Malformed,
        ),
    )];
    let audit = reduce(&verified, &input, &history);
    assert_research_dispatch(&audit, "audit_disposition");
    assert!(audit.terminal.is_none());
}

#[tokio::test]
async fn auditor_crash_fails_without_retrying_a_written_audit() {
    let authored = super::auto_research::audit_probe_graph().assert_value();
    let input = audit_probe_input();
    let verified = verified_research_probe(authored, &input, "Auditor crash routing", "auditor-crash-routing").await;
    let first = reduce(&verified, &input, &[]);
    let history = vec![settle_audit_dispatch(
        &first,
        1,
        openengine_cluster_protocol::WorkerOutcome::declared_failure(
            openengine_cluster_protocol::WorkerErrorCode::Crash,
        ),
    )];

    let failed = reduce(&verified, &input, &history);
    assert_eq!(
        failed.terminal,
        Some(TerminalProjection::Failed {
            reason: "iteration_finalization_audit_failed".parse().assert_value()
        })
    );
    assert!(failed.decisions.iter().all(|decision| !matches!(
        decision,
        Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "audit_disposition"
    )));
}


fn scout_handoff_history(first: &Reduction, malformed_middle_scout: bool) -> Vec<DurableExecution> {
    use crate::full_v1_reducer::{DurableExecutionState, HistoryPosition};
    use openengine_cluster_protocol::{WorkerErrorCode, WorkerOutcome};

    let scouts = first
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch {
                node_instance,
                execution,
                occurrence,
                attempt,
                input,
                ..
            } if occurrence.node.as_str() == "research_scout" => {
                Some((*node_instance, *execution, occurrence, *attempt, input))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(scouts.len(), 3);
    scouts
        .into_iter()
        .enumerate()
        .map(|(index, (node_instance, execution, occurrence, attempt, input))| {
            let outcome = if index == 1 && malformed_middle_scout {
                WorkerOutcome::declared_failure(WorkerErrorCode::Malformed)
            } else {
                let proposal = if index == 1 {
                    "iteration 1 synthesizer: self-reported invalid procedure"
                } else {
                    "iteration 1: bounded valid direction"
                };
                WorkerOutcome::Verifier {
                    output: json!({"proposal": proposal}),
                    signals: Default::default(),
                    diagnostic: Value::Null,
                    artifacts: Vec::new(),
                }
            };
            DurableExecution {
                dispatch_position: HistoryPosition::new(index as u64 * 2).assert_value(),
                node_instance,
                execution,
                occurrence: occurrence.clone(),
                attempt,
                input: input.clone(),
                state: DurableExecutionState::Settled {
                    position: HistoryPosition::new(index as u64 * 2 + 1).assert_value(),
                    outcome,
                },
            }
        })
        .collect()
}

fn dispatched_planner_input(reduction: &Reduction) -> &Value {
    reduction
        .decisions
        .iter()
        .find_map(|decision| match decision {
            Decision::Dispatch { occurrence, input, .. }
                if occurrence.node.as_str() == "plan_experiment" => Some(input),
            _ => None,
        })
        .assert_value_with("planner dispatch")
}

#[tokio::test]
async fn failed_scout_map_preserves_no_partial_proposals_and_dispatches_planner() {
    let authored = super::auto_research::scout_handoff_probe_graph().assert_value();
    let input = json!({
        "task": "research a bounded improvement",
        "scoutRoles": [
            {"role": "explorer"},
            {"role": "synthesizer"},
            {"role": "challenger"}
        ],
        "proposals": []
    });
    let verified = verified_research_probe(
        authored,
        &input,
        "Scout handoff routing",
        "scout-handoff-routing",
    )
    .await;
    let first = reduce(&verified, &input, &[]);

    let semantic = reduce(&verified, &input, &scout_handoff_history(&first, false));
    let semantic_input = dispatched_planner_input(&semantic);
    assert_eq!(semantic_input["proposals"].as_array().unwrap().len(), 3);
    assert!(semantic_input["proposals"][1]
        .as_str()
        .unwrap()
        .contains("self-reported invalid procedure"));

    let mut failed_history = scout_handoff_history(&first, true);
    let failed = reduce(&verified, &input, &failed_history);
    let failed_input = dispatched_planner_input(&failed);
    assert_eq!(failed_input["proposals"], json!([]));
    assert!(failed.terminal.is_none());

    failed_history.push(settle_audit_dispatch(
        &failed,
        7,
        openengine_cluster_protocol::WorkerOutcome::Verifier {
            output: Value::Null,
            signals: std::collections::BTreeMap::from([(
                field_name(VERDICT_FIELD).assert_value(),
                enum_label("work").assert_value(),
            )]),
            diagnostic: json!({"message": "archived direction independently verified"}),
            artifacts: Vec::new(),
        },
    ));
    let finished = reduce(&verified, &input, &failed_history);
    assert!(matches!(
        finished.terminal,
        Some(TerminalProjection::Succeeded { output }) if output.is_null()
    ));
}
