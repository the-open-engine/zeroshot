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
    let staging = find_choice(iteration_nodes, "staging_review_result");
    assert_eq!(staging.branches.as_slice()[2].node.name().as_str(), "experiment_stage");
    let stage_review = find_verifier(iteration_nodes, "staging_review");
    assert!(stage_review.instructions.as_ref().is_some_and(|text| {
        text.as_str().contains("incumbent scratch backup")
    }));
    let exit_route = find_choice(iteration_nodes, "exit_route");
    assert_eq!(exit_route.branches.as_slice()[0].node.name().as_str(), "done_early");
    assert!(matches!(exit_route.branches.as_slice()[0].node, GraphNode::Succeed(_)));
    assert_eq!(exit_route.branches.as_slice()[1].node.name().as_str(), "iteration_continues");
}
