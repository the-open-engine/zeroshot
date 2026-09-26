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
            && text.as_str().contains("next iteration number")
    }));
}

fn assert_research_planner(iteration_nodes: &[&GraphNode]) {
    let planner = find_verifier(iteration_nodes, "plan_experiment");
    assert_eq!(planner.worker.as_str(), "builtin.agent.research-planner@1");
    assert!(planner.instructions.as_ref().is_some_and(|value| {
        value.as_str().contains("retained workspace violates a charter invariant")
            && value.as_str().contains("Compare expected progress and information")
            && value.as_str().contains("considered alternatives")
            && value.as_str().contains("parentArtifactId")
    }));
    assert_eq!(
        planner.signals.get(&field_name(VERDICT_FIELD).assert_value()),
        Some(&enum_labels(&["work", "stop", "abort"]).assert_value())
    );
    assert!(iteration_nodes.iter().all(|node| node.name().as_str() != "plan_review"));
    let selection = find_choice(iteration_nodes, "research_route");
    assert_eq!(selection.branches.as_slice()[2].node.name().as_str(), "record_stop");
    assert_eq!(selection.branches.as_slice()[3].node.name().as_str(), "work_phase");
    let staging = find_choice(iteration_nodes, "staging_result");
    assert_eq!(staging.branches.as_slice().len(), 1);
    assert_eq!(staging.branches.as_slice()[0].node.name().as_str(), "abort_staging");
    assert_eq!(
        staging.otherwise.as_ref().map(|node| node.name().as_str()),
        Some("experiment_stage")
    );
    assert!(iteration_nodes
        .iter()
        .all(|node| node.name().as_str() != "staging_review"));
    let exit_route = find_choice(iteration_nodes, "exit_route");
    assert_eq!(exit_route.branches.as_slice()[0].node.name().as_str(), "done_early_after_recheck");
    assert_eq!(
        exit_route.branches.as_slice()[1].node.name().as_str(),
        "iteration_continues_after_recheck"
    );
    assert_eq!(exit_route.branches.as_slice()[2].node.name().as_str(), "done_early");
    assert_eq!(exit_route.branches.as_slice()[3].node.name().as_str(), "iteration_continues");
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

fn assert_research_dispatch(reduction: &Reduction, expected: &str) {
    assert!(reduction.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == expected
    )));
}

#[tokio::test]
async fn rejected_research_audit_repairs_then_rechecks_before_continuing() {
    use openengine_cluster_protocol::WorkerOutcome;

    let authored = super::auto_research::audit_probe_graph().assert_value();
    let runtime = runtime_for(
        BuiltinGraphTemplate::AutoResearch,
        TemplateDelivery::None,
        &executable_leaves(&authored.root),
    );
    let input = json!({
        "task": "audit a research iteration",
        "workItems": [null],
        "continuationItems": []
    });
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            title: RunTitle::new("Audit repair routing").assert_value(),
            graph: authored,
            initial_input: input.clone(),
            runtime,
            source: resolved_source(),
            submission_key: IdempotencyKey::new("audit-repair-routing").assert_value(),
        })
        .await
        .unwrap_or_else(|error| panic!("audit probe admission: {error:?}"));
    let verified = VerifiedGraph {
        compiled_ir: admitted.graph,
        diagnostics: Vec::new(),
    };

    let mut history = Vec::new();
    let first = reduce(&verified, &input, &history);
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
    history.push(settle_audit_dispatch(
        &first,
        1,
        audit_review_outcome("rejected"),
    ));

    let repair = reduce(&verified, &input, &history);
    assert_research_dispatch(&repair, "audit_repair");
    history.push(settle_audit_dispatch(
        &repair,
        2,
        WorkerOutcome::Verified {
            output: Value::Null,
            artifacts: Vec::new(),
        },
    ));

    let recheck = reduce(&verified, &input, &history);
    assert_research_dispatch(&recheck, "audit_disposition_recheck");
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
