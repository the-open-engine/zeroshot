use super::*;
use crate::source_code_provider::adapters::core::gitlab::GitLabDeliveryAuthority;

#[tokio::test]
async fn gitlab_normalizes_commit_messages_before_pushing_repairs_to_an_existing_review() {
    let repo = TempRepo::delivery();
    let mut authority = FakeGitHub::new(repo.remote.clone(), Script::NoCi);
    authority.commit_message_authority = Some(Arc::new(
        GitLabDeliveryAuthority::new(
            openengine_cluster_protocol::SourceForgeBaseUrl::new("https://gitlab.example")
                .assert_value(),
            None,
        )
        .assert_value(),
    ));
    let authority = Arc::new(authority);
    let adapter = retained_adapter(
        &repo,
        authority.clone(),
        DeliveryPollPolicy::new(2, Duration::ZERO).assert_value(),
        DeliveryLineage::original("gitlab-generated-commits"),
    );
    let admitted = admitted(&repo, DeliveryMode::PullRequest).await;
    let binding = admitted.runtime.nodes()[&NodeName::new("deliver").assert_value()].clone();
    let runner = NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value();
    let branch = delivery_head_branch(&RunId::new("gitlab-generated-commits"));
    let mut previous = None;
    for (index, title, expected) in [
        (1, "Initial report", "Initial report"),
        (2, "Draft: fix behavior", "fix behavior"),
        (3, "[Draft]", "Zeroshot change"),
    ] {
        fs::write(
            repo.workspace.join("result.txt"),
            format!("repair {index}\n"),
        )
        .assert_value();
        let request = NodeRunRequest {
            dispatch: Default::default(),
            invocation: NodeInvocation {
                reference: ExecutionRef {
                    run_id: RunId::new("gitlab-generated-commits"),
                    node: NodeName::new("deliver").assert_value(),
                    node_instance: native_v2_contract::NodeInstanceId::new(1).assert_value(),
                    execution: native_v2_contract::ExecutionId::new(index).assert_value(),
                },
                worker: WorkerRef::new(DeliveryMode::PullRequest.worker_ref()).assert_value(),
                instructions: None,
                input: json!({"title":title,"description":"Generated report."}),
                binding: binding.clone(),
            },
            environment: ResolvedEnvironment::exact(
                &binding,
                BTreeMap::from([(
                    EnvironmentVariableName::new(GITHUB_TOKEN_ENV).assert_value(),
                    "test-token".to_owned(),
                )]),
            )
            .assert_value(),
        };
        let mut handle = runner.start(request).await.assert_value();
        let _output = handle.take_initial_output().assert_value();
        let outcome = handle.completion().await.assert_value().outcome;
        assert_delivery_signal(&outcome, DELIVERY_OPENED_LABEL);
        let head = git_output(&repo.remote, &["rev-parse", &branch]);
        assert_ne!(previous.as_ref(), Some(&head));
        assert_eq!(
            git_output(&repo.remote, &["show", "--no-patch", "--format=%s", &head]),
            expected
        );
        // The original title still reaches the forge so its description can retain a shortened one.
        assert_eq!(
            authority.review_requests().last().assert_value().title,
            title
        );
        previous = Some(head);
    }
}
