use super::*;

#[tokio::test]
async fn rejects_invalid_graph_visible_delivery_bindings_and_contracts() {
    let step_graph = graph(vec![
        null_step("deliver", GIT_DELIVERY_PR_WORKER_REF),
        succeed("done"),
    ]);
    let request = submission(
        step_graph,
        BTreeMap::from([(named("deliver"), delivery_binding())]),
    );
    assert!(matches!(
        NativeV2Admission.admit(request).await,
        Err(NativeV2AdmissionError::DeliveryMustBeVerifier { .. })
    ));

    let unsupported_graph = graph(vec![
        null_verifier("deliver", "builtin.git-delivery@1"),
        succeed("done"),
    ]);
    assert!(matches!(
        NativeV2Admission
            .admit(submission(
                unsupported_graph,
                BTreeMap::from([(named("deliver"), delivery_binding())]),
            ))
            .await,
        Err(NativeV2AdmissionError::UnsupportedDeliveryWorker { .. })
    ));

    let wrong_binding_graph = graph(vec![
        delivery_verifier("deliver", DeliveryMode::PullRequest),
        succeed("done"),
    ]);
    assert!(matches!(
        NativeV2Admission
            .admit(submission(
                wrong_binding_graph,
                BTreeMap::from([(named("deliver"), binding("claude-sonnet-5", None))]),
            ))
            .await,
        Err(NativeV2AdmissionError::DeliveryWorkerRequiresBinding { .. })
    ));

    let invalid_contract = graph(vec![
        null_verifier("deliver", GIT_DELIVERY_PR_WORKER_REF),
        succeed("done"),
    ]);
    assert!(matches!(
        NativeV2Admission
            .admit(submission(
                invalid_contract,
                BTreeMap::from([(named("deliver"), delivery_binding())]),
            ))
            .await,
        Err(NativeV2AdmissionError::InvalidDeliveryContract { .. })
    ));
}

#[tokio::test]
async fn enforces_graph_visible_delivery_policy_counts() {
    let no_delivery = submission(graph(vec![succeed("done")]), BTreeMap::new());
    assert_eq!(
        NativeV2Admission
            .admit_with_policy(no_delivery, DeliveryPolicy::Required)
            .await,
        Err(NativeV2AdmissionError::DeliveryNodeCount {
            policy: DeliveryPolicy::Required,
            found: 0,
        })
    );

    let delivery_graph = graph(vec![
        delivery_verifier("deliver", DeliveryMode::Merge),
        succeed("done"),
    ]);
    NativeV2Admission
        .admit_with_policy(
            submission(
                delivery_graph,
                BTreeMap::from([(named("deliver"), delivery_binding())]),
            ),
            DeliveryPolicy::Required,
        )
        .await
        .assert_value_with("required policy accepts one valid delivery node");

    let two_deliveries = graph(vec![
        delivery_verifier("open", DeliveryMode::PullRequest),
        delivery_verifier("merge", DeliveryMode::Merge),
        succeed("done"),
    ]);
    assert_eq!(
        NativeV2Admission
            .admit(submission(
                two_deliveries,
                BTreeMap::from([
                    (named("open"), delivery_binding()),
                    (named("merge"), delivery_binding()),
                ]),
            ))
            .await,
        Err(NativeV2AdmissionError::DeliveryNodeCount {
            policy: DeliveryPolicy::Optional,
            found: 2,
        })
    );
}

#[tokio::test]
async fn merge_profile_validation_rejects_pull_request_delivery() {
    let pull_request = submission(
        graph(vec![
            delivery_verifier("deliver", DeliveryMode::PullRequest),
            succeed("done"),
        ]),
        BTreeMap::from([(named("deliver"), delivery_binding())]),
    );
    assert_eq!(
        NativeV2Admission
            .validate_merge_profile(&pull_request.graph, &pull_request.runtime)
            .await,
        Err(NativeV2AdmissionError::MergeDeliveryRequired)
    );

    let merge = submission(
        graph(vec![
            delivery_verifier("deliver", DeliveryMode::Merge),
            succeed("done"),
        ]),
        BTreeMap::from([(named("deliver"), delivery_binding())]),
    );
    NativeV2Admission
        .validate_merge_profile(&merge.graph, &merge.runtime)
        .await
        .assert_value_with("merge plans accept the sole merge delivery worker");
}

#[tokio::test]
async fn legacy_merge_worker_remains_admissible_but_is_not_a_merge_plan_profile() {
    let legacy_graph = graph(vec![
        delivery_verifier("deliver", DeliveryMode::MergeV1),
        succeed("done"),
    ]);
    let legacy_nodes = BTreeMap::from([(named("deliver"), delivery_binding())]);

    NativeV2Admission
        .admit_with_policy(
            submission(legacy_graph.clone(), legacy_nodes.clone()),
            DeliveryPolicy::Required,
        )
        .await
        .assert_value_with("stored merge@1 graphs remain admissible");

    assert_eq!(
        NativeV2Admission
            .validate_merge_profile(
                &legacy_graph,
                &RuntimePlan::Claude {
                    provider: ClaudeProvider::Anthropic,
                    size: RunSize::Medium,
                    nodes: legacy_nodes,
                },
            )
            .await,
        Err(NativeV2AdmissionError::MergeDeliveryRequired)
    );
}
