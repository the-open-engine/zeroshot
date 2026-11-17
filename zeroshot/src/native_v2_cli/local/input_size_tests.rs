use super::*;

#[tokio::test]
async fn oversized_submission_is_rejected_before_local_run_storage_or_bootstrap() {
    for (size, worktree) in [
        (3_000_000, None),
        (9_700_000, None),
        (
            3_000_000,
            Some(crate::native_v2_cli::LocalWorktreeBase::Local),
        ),
    ] {
        let root = TestDirectory::new("submission-size");
        let workspace = root.child("workspace");
        std::fs::create_dir(&workspace).assert_value();
        if worktree.is_some() {
            initialize_local_repository(&workspace);
        }
        let backend = contract_backend(&root.child("state"));
        let run_id = RunId::new("01900000-0000-7000-8000-000000000162");
        let mut submission = contract_submission("oversized-submission");
        let mut graph = serde_json::to_value(&submission.graph).assert_value();
        let schema =
            json!({"kind":"record","fields":{"task":{"required":true,"type":{"kind":"string"}}}});
        graph["initialInput"] = schema.clone();
        graph["root"]["state"] = schema;
        submission.graph = serde_json::from_value(graph).assert_value();
        submission.initial_input = json!({"task":"x".repeat(size)});
        let input_bytes = serde_json::to_vec(&submission.initial_input)
            .assert_value()
            .len();
        let admitted = NativeV2Admission
            .admit(submission.clone())
            .await
            .assert_value();
        let submission_bytes = serde_json::to_vec(&admitted).assert_value().len();
        let error = backend
            .submit_prepared(
                prepared_local_run(run_id.clone(), submission, workspace),
                worktree,
            )
            .await
            .expect_err("oversized submission");
        let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
        assert_eq!(diagnostic["kind"], "invalid_request");
        assert_eq!(diagnostic["code"], "submission.too_large");
        for detail in [
            submission_bytes.to_string(),
            input_bytes.to_string(),
            "2097152".into(),
            "2 MiB".into(),
        ] {
            assert!(
                error.to_string().contains(&detail),
                "missing {detail}: {error}"
            );
        }
        assert!(!backend.run_storage(&run_id).assert_value().exists());
        assert!(!root.child("state-worktrees").exists());
    }
}

#[tokio::test]
async fn invalid_submission_input_is_rejected_before_local_run_storage_or_worktree() {
    for worktree in [None, Some(crate::native_v2_cli::LocalWorktreeBase::Local)] {
        let root = TestDirectory::new("submission-input-type");
        let workspace = root.child("workspace");
        std::fs::create_dir(&workspace).assert_value();
        if worktree.is_some() {
            initialize_local_repository(&workspace);
        }
        let backend = contract_backend(&root.child("state"));
        let run_id = RunId::new("01900000-0000-7000-8000-000000000163");
        let mut submission = contract_submission("invalid-submission-input");
        submission.initial_input = json!(true);
        let error = backend
            .submit_prepared(
                prepared_local_run(run_id.clone(), submission, workspace),
                worktree,
            )
            .await
            .expect_err("invalid initial input");
        assert!(matches!(
            error,
            NativeV2CliError::InvalidRun(
                crate::native_v2_admission::NativeV2AdmissionError::InitialInput(_)
            )
        ));
        let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
        assert_eq!(diagnostic["kind"], "invalid_request");
        assert_eq!(diagnostic["code"], "input.type_mismatch");
        assert!(!backend.run_storage(&run_id).assert_value().exists());
        assert!(!root.child("state-worktrees").exists());
    }
}

#[tokio::test]
async fn large_valid_submission_reaches_worktree_preparation_before_controller_launch() {
    let root = TestDirectory::new("submission-worktree");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    initialize_local_repository(&workspace);
    let backend = contract_backend(&root.child("state"));
    let run_id = RunId::new("01900000-0000-7000-8000-000000000164");
    let branch = crate::native_v2_delivery::delivery_head_branch(&run_id);
    crate::native_v2_candidate::test_support::git(&workspace, &["branch", &branch]);

    let mut submission = contract_submission("large-valid-submission");
    let mut graph = serde_json::to_value(&submission.graph).assert_value();
    let schema =
        json!({"kind":"record","fields":{"task":{"required":true,"type":{"kind":"string"}}}});
    graph["initialInput"] = schema.clone();
    graph["root"]["state"] = schema;
    submission.graph = serde_json::from_value(graph).assert_value();
    submission.initial_input = json!({"task":"x".repeat(1_400_000)});

    let error = backend
        .submit_prepared(
            prepared_local_run(run_id.clone(), submission, workspace),
            Some(crate::native_v2_cli::LocalWorktreeBase::Local),
        )
        .await
        .expect_err("existing worktree branch");
    assert!(matches!(error, NativeV2CliError::Usage(_)));
    assert!(error.to_string().contains(&branch), "{error}");
    assert!(!backend.run_storage(&run_id).assert_value().exists());
    assert!(!root.child("state-worktrees").join(run_id.as_str()).exists());
}
