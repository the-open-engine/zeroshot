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

fn stack_profile(mode: DeliveryMode) -> RunSubmission {
    submission(
        graph(vec![delivery_verifier("deliver", mode), succeed("done")]),
        BTreeMap::from([(named("deliver"), delivery_binding())]),
    )
}

async fn validate_stack(
    profile: &RunSubmission,
    delivery: StackDelivery,
) -> Result<(), NativeV2AdmissionError> {
    NativeV2Admission
        .validate_stack_profile(&profile.graph, &profile.runtime, delivery)
        .await
}

#[tokio::test]
async fn stack_profiles_must_match_the_stack_delivery() {
    for mode in [DeliveryMode::Merge, DeliveryMode::MergeV3] {
        validate_stack(&stack_profile(mode), StackDelivery::Merge)
            .await
            .assert_value_with("merge stacks accept the sole merge delivery worker");
        assert_eq!(
            validate_stack(&stack_profile(mode), StackDelivery::StackedPullRequests).await,
            Err(NativeV2AdmissionError::StackDeliveryRequired {
                delivery: StackDelivery::StackedPullRequests,
            })
        );
    }

    let pull_request = stack_profile(DeliveryMode::PullRequestV2);
    validate_stack(&pull_request, StackDelivery::StackedPullRequests)
        .await
        .assert_value_with("stacked pull requests accept the sole pr@2 delivery worker");
    assert_eq!(
        validate_stack(&pull_request, StackDelivery::Merge).await,
        Err(NativeV2AdmissionError::StackDeliveryRequired {
            delivery: StackDelivery::Merge,
        })
    );

    for (mode, delivery) in [
        (
            DeliveryMode::PullRequest,
            StackDelivery::StackedPullRequests,
        ),
        (DeliveryMode::Push, StackDelivery::StackedPullRequests),
        (DeliveryMode::Push, StackDelivery::Merge),
    ] {
        assert_eq!(
            validate_stack(&stack_profile(mode), delivery).await,
            Err(NativeV2AdmissionError::StackDeliveryRequired { delivery }),
            "{mode:?} cannot deliver {delivery:?} stacks"
        );
    }
}

#[test]
fn stack_delivery_errors_name_the_required_worker() {
    assert_eq!(
        NativeV2AdmissionError::StackDeliveryRequired {
            delivery: StackDelivery::Merge,
        }
        .to_string(),
        "stack delivery \"merge\" requires exactly one graph-visible Git merge delivery node \
         (merge@2 or merge@3)"
    );
    assert_eq!(
        NativeV2AdmissionError::StackDeliveryRequired {
            delivery: StackDelivery::StackedPullRequests,
        }
        .to_string(),
        "stack delivery \"stacked_pull_requests\" requires exactly one graph-visible Git \
         pull-request delivery node (pr@2)"
    );
}

#[tokio::test]
async fn stack_profiles_keep_github_credentials_on_delivery_nodes() {
    for (mode, delivery) in [
        (DeliveryMode::MergeV3, StackDelivery::Merge),
        (
            DeliveryMode::PullRequestV2,
            StackDelivery::StackedPullRequests,
        ),
    ] {
        let stack_graph = graph(vec![
            null_step("worker", "worker.change@1"),
            delivery_verifier("deliver", mode),
            succeed("done"),
        ]);
        let agent_token = submission(
            stack_graph.clone(),
            BTreeMap::from([
                (
                    named("worker"),
                    binding_with_environment([GITHUB_TOKEN_ENV.to_owned()]),
                ),
                (named("deliver"), delivery_binding()),
            ]),
        );

        NativeV2Admission
            .validate_profile(
                &agent_token.graph,
                &agent_token.runtime,
                DeliveryPolicy::Required,
            )
            .await
            .assert_value_with("ordinary hosted runs may explicitly bind GitHub to an agent");
        assert_eq!(
            validate_stack(&agent_token, delivery).await,
            Err(NativeV2AdmissionError::StackAgentGitHubToken {
                node: named("worker"),
            })
        );

        let delivery_token = submission(
            stack_graph,
            BTreeMap::from([
                (named("worker"), binding("claude-sonnet-5", None)),
                (
                    named("deliver"),
                    NodeRuntimeBinding::GitDelivery {
                        connections: DeclaredConnections::single(
                            "github-alias",
                            DeclaredEnvironment::new([EnvironmentVariableName::new(
                                GITHUB_TOKEN_ENV,
                            )
                            .assert_value()])
                            .assert_value(),
                        )
                        .assert_value(),
                        pull_request_feedback: Default::default(),
                    },
                ),
            ]),
        );
        validate_stack(&delivery_token, delivery)
            .await
            .assert_value_with("stacks keep GitHub credentials on Git delivery");
    }
}

#[tokio::test]
async fn legacy_merge_worker_remains_admissible_but_is_not_a_stack_profile() {
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
            .validate_stack_profile(
                &legacy_graph,
                &RuntimePlan::new(
                    openengine_cluster_protocol::RuntimeLane::new(
                        "claude",
                        (ClaudeProvider::Anthropic).name()
                    )
                    .assert_value(),
                    RunSize::Medium,
                    legacy_nodes
                ),
                StackDelivery::Merge,
            )
            .await,
        Err(NativeV2AdmissionError::StackDeliveryRequired {
            delivery: StackDelivery::Merge,
        })
    );
}
