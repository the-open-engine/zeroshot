//! Local delivery composition and publication state survive independent controller attempts.
use super::*;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use crate::native_v2_comment_delivery::{PublicationRecord, PublicationStatus};

const URL: &str = "https://github.com/acme/project/issues/17";

async fn comment_run(source: Option<ResolvedSource>) -> (RunSubmission, AdmittedRun) {
    let graph = BuiltinGraphTemplate::IssueReview
        .materialize(TemplateDelivery::Comment)
        .assert_value();
    let submission: RunSubmission = serde_json::from_value(json!({
        "title":"Local comment delivery", "graph":graph,
        "initialInput":{"url":URL,"context":{"title":"Proposal","body":"Review this","reviewScope":{}}},
        "runtime":{"harness":"codex","provider":"openai","size":"small",
            "activeMemory":{"repository":false,"graph":false},"globalAgents":{"postmortem":{"enabled":false}},
            "nodes":{"reviewer":{"kind":"agent","model":"caller-model"},
                "deliver":{"kind":"comment_delivery","connections":{"github":["GH_TOKEN"]}}}},
        "source":source, "submissionKey":"local-comment"
    })).assert_value();
    let admitted = NativeV2Admission
        .admit_with_policy(
            submission.clone(),
            crate::native_v2_admission::DeliveryPolicy::Optional,
        )
        .await
        .assert_value();
    (submission, admitted)
}

#[cfg(unix)]
#[tokio::test]
async fn source_less_comment_composition_requires_gh_but_no_git_program_or_provider_login() {
    let root = TestDirectory::new("local-comment-composition");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let storage = root.child("runs/attempt");
    let (bin, environment) = search_directory(&root);
    let (submission, admitted) = comment_run(None).await;
    assert!(matches!(
        check_delivery_executables(&submission, &environment, &workspace),
        Err(LocalCompositionError::MissingDeliveryExecutable { executable: "gh" })
    ));
    install_executable(&bin, "gh");
    install_executable(&bin, "codex");
    check_delivery_executables(&submission, &environment, &workspace).assert_value();
    for owner in [false, true] {
        let request = LocalProcessCandidateRequest {
            admitted: &admitted,
            delivery_run_id: RunId::new("root-comment"),
            adopt_existing_delivery: false,
            workspace: &workspace,
            storage: &storage,
            memory_storage: None,
            github_token: None,
            github_token_file: None,
            delivery_validator: None,
            git_credentials: None,
            native_environment: &environment,
        };
        if owner {
            build_local_owner_process_candidate(request).assert_value();
        } else {
            build_local_process_candidate(request).assert_value();
        }
    }
    assert!(root.child("comment-publications").is_dir());
    assert!(!bin.join("git").exists());
}

#[tokio::test]
async fn confirmed_comment_facts_survive_attempt_disposal_and_remain_lineage_scoped() {
    let root = TestDirectory::new("local-comment-durability");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let (bin, environment) = search_directory(&root);
    install_executable(&bin, "gh");
    let (_, admitted) = comment_run(Some(github_source())).await;
    let compose = |storage: &Path, identity: &RunId| {
        local_comment_delivery(LocalCommentRequest {
            admitted: &admitted,
            delivery_run_id: identity,
            storage,
            environment: &environment,
            workspace: &workspace,
            runtime_home: &storage.join("runtime"),
        })
        .assert_value()
        .assert_value()
    };
    let identity = RunId::new("root-comment");
    let first_storage = root.child("runs/first");
    std::fs::create_dir_all(first_storage.join("runtime")).assert_value();
    let first = compose(&first_storage, &identity);
    assert_eq!(first.config.expected_url, URL);
    assert_eq!(first.config.expected_review_scope, Some(json!({})));
    assert_eq!(
        first.config.source_revision.as_deref(),
        admitted
            .source
            .as_ref()
            .map(|source| source.revision.as_str())
    );
    let record = PublicationRecord {
        version: 1,
        url: URL.to_owned(),
        marker: format!("<!-- zeroshot-comment:v1:{} -->", "b".repeat(64)),
        body_hash: "a".repeat(64),
        author: "123".to_owned(),
        status: PublicationStatus::Confirmed {
            comment_id: "456".to_owned(),
            comment_url: format!("{URL}#issuecomment-456"),
        },
    };
    first
        .store
        .compare_and_set(None, &record)
        .await
        .assert_value();
    std::fs::remove_dir_all(first_storage).assert_value();
    std::fs::remove_dir_all(&workspace).assert_value();
    std::fs::create_dir(&workspace).assert_value();
    let resumed = compose(&root.child("runs/resumed"), &identity);
    assert_eq!(resumed.store.load().await.assert_value(), Some(record));
    let unrelated = compose(
        &root.child("runs/unrelated"),
        &RunId::new("unrelated-comment"),
    );
    assert!(unrelated.store.load().await.assert_value().is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn software_change_pr_composition_checks_source_and_programs_without_dispatch() {
    let root = TestDirectory::new("local-software-change-composition");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let storage = root.child("runs/attempt");
    let (bin, environment) = search_directory(&root);
    let marker = root.child("unexpected-execution");
    let script = format!(
        "#!/bin/sh\nprintf invoked >> '{}'\nexit 93\n",
        marker.to_string_lossy().replace('\'', "'\\''")
    );
    let install = |name: &str| {
        openengine_cluster_testkit::fixture::write_executable(&bin.join(name), &script, 0o700)
            .assert_value()
    };
    install("codex");
    let template = BuiltinGraphTemplate::SoftwareChange;
    let graph = template
        .materialize(TemplateDelivery::PullRequest)
        .assert_value();
    let mut nodes: BTreeMap<_, _> =
        crate::native_v2_admission::executable_runtime_roles(&graph.root)
            .into_iter()
            .filter(|(_, delivery)| !delivery)
            .map(|(name, _)| {
                (
                    name,
                    crate::native_v2_runner::test_support::binding(
                        openengine_cluster_protocol::SessionScope::Execution,
                    ),
                )
            })
            .collect();
    let (name, binding) = template
        .delivery_runtime_binding(TemplateDelivery::PullRequest)
        .assert_value()
        .assert_value();
    nodes.insert(name, binding);
    let mut runtime = codex_plan(json!(nodes));
    runtime.active_memory_mut().repository = false;
    runtime.active_memory_mut().graph = false;
    runtime.global_agents_mut().postmortem.enabled = false;
    let submission = RunSubmission {
        title: crate::native_v2_contract::RunTitle::new(
            "Compose the default software-change delivery",
        )
        .assert_value(),
        graph,
        initial_input: json!({"task":"Review the implementation and publish the accepted change"}),
        runtime,
        environment: None,
        source: Some(github_source()),
        submission_key: openengine_cluster_protocol::IdempotencyKey::new("local-pr-composition")
            .assert_value(),
    };
    let admitted = NativeV2Admission.admit(submission).await.assert_value();
    let request = || LocalProcessCandidateRequest {
        admitted: &admitted,
        delivery_run_id: RunId::new("root-pr"),
        adopt_existing_delivery: false,
        workspace: &workspace,
        storage: &storage,
        memory_storage: None,
        github_token: None,
        github_token_file: None,
        delivery_validator: None,
        git_credentials: None,
        native_environment: &environment,
    };
    let mut missing_source = admitted.clone();
    missing_source.source = None;
    assert!(matches!(
        build_local_process_candidate(LocalProcessCandidateRequest {
            admitted: &missing_source,
            ..request()
        }),
        Err(LocalCompositionError::DeliverySourceRequired)
    ));
    assert!(matches!(
        build_local_process_candidate(request()),
        Err(LocalCompositionError::MissingDeliveryExecutable { executable: "gh" })
    ));
    install("gh");
    assert!(matches!(
        build_local_process_candidate(request()),
        Err(LocalCompositionError::MissingDeliveryExecutable { executable: "git" })
    ));
    install("git");
    build_local_process_candidate(request()).assert_value();
    build_local_owner_process_candidate(request()).assert_value();
    assert!(
        !marker.exists(),
        "composition must not execute providers or Git/forge operations"
    );
    assert_eq!(std::fs::read_dir(&workspace).assert_value().count(), 0);
}
