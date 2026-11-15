use super::*;

#[tokio::test]
async fn mapped_error_outcome_does_not_expose_success_only_results() {
    let graph = mapped_error_read_graph();
    let error = assert_graph_rejected(&graph).await;
    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}
#[tokio::test]
async fn nested_maps_preserve_outer_index_definition_isolation() {
    let graph = nested_map_graph();
    assert_undefined_read_without_schema_error(&graph).await;
}

#[tokio::test]
async fn k_of_map_counts_group_controls_across_the_enclosing_map() {
    let graph = mapped_loop_group_aggregate_graph();
    assert_graph_accepted(&graph).await;
}

#[tokio::test]
async fn mapped_parallel_controls_allow_more_items_than_concurrency() {
    assert_graph_accepted(&mapped_parallel_control_correlation_graph(2, 1)).await;
}
#[tokio::test]
async fn mapped_parallel_controls_allow_jointly_possible_item_counts() {
    ProductionGraphVerifier::new(registry())
        .verify(&mapped_parallel_control_correlation_graph(1, 1))
        .await
        .assert_value();
}

#[tokio::test]
async fn mapped_parallel_controls_allow_counts_requiring_three_items() {
    assert_graph_accepted(&mapped_parallel_multicontrol_correlation_graph()).await;
}

#[tokio::test]
async fn filtering_only_some_map_errors_cannot_define_success_only_results() {
    for (labels, all_errors) in [
        (json!(["timeout"]), false),
        (json!(["crash"]), false),
        (json!(["timeout", "crash", "malformed"]), false),
        (json!(["timeout", "crash", "malformed", "refusal"]), true),
    ] {
        let mut value = serde_json::to_value(mapped_error_read_graph()).assert_value();
        let route = value
            .assert_at_mut("root")
            .assert_at_mut("children")
            .assert_at_mut(1);
        let read = route
            .assert_at_mut("branches")
            .assert_at_mut(0)
            .assert_at_mut("node")
            .take();
        let fallback = route.assert_at_mut("otherwise").take();
        let branch = route.assert_at_mut("branches").assert_at_mut(0);
        *branch.assert_at_mut("node") = fallback;
        *branch.assert_at_mut("when").assert_at_mut("labels") = labels;
        *route.assert_at_mut("otherwise") = read;
        let graph = serde_json::from_value(value).assert_value();
        if all_errors {
            assert_graph_accepted(&graph).await;
        } else {
            assert_undefined_read_without_schema_error(&graph).await;
        }
    }
}
