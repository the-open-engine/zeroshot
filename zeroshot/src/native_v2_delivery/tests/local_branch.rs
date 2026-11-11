use super::*;

#[tokio::test]
async fn timeout_and_cancellation_retain_the_confirmed_review_before_polling_completes() {
    for cancel in [false, true] {
        let repo = TempRepo::delivery();
        let branch = "feature/retained-review";
        git(&repo.workspace, &["checkout", "-b", branch]);
        let storage = repo.root.child("controller");
        fs::create_dir(&storage).assert_value();
        let lineage = RunId::new("retained-review");
        let mode = DeliveryMode::PullRequestV2;
        let recovery_path = storage.join("workspace-recovery.json");
        fs::write(
            &recovery_path,
            serde_json::to_vec(&json!({
                "submission": delivery_submission(&repo, mode, PullRequestFeedback::Consider),
                "workspace": repo.workspace,
                "localBranch": branch,
                "deliveryRunId": lineage,
            }))
            .assert_value(),
        )
        .assert_value();
        let recorder = crate::native_v2_cli::local::review_binding_recorder(
            &storage,
            &repo.workspace,
            Some(branch),
            &lineage,
        )
        .assert_value();
        let authority = Arc::new(FakeGitHub::new(
            repo.remote.clone(),
            Script::PendingReadiness,
        ));
        let config = NativeV2DeliveryConfig {
            local_branch: Some(branch.to_owned()),
            delivery_run_id: lineage,
            adopt_existing_delivery: false,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: repo.workspace.clone(),
            git_program: "/usr/bin/git".into(),
            target: target(&repo),
            poll: DeliveryPollPolicy::new(
                if cancel { 100 } else { 2 },
                if cancel {
                    Duration::from_secs(5)
                } else {
                    Duration::ZERO
                },
            )
            .assert_value(),
        };
        let adapter = Arc::new(
            NativeV2DeliveryAdapter::new(config.clone(), authority.clone())
                .with_review_binding_recorder(recorder.clone()),
        );
        let request = || DeliveryRunRequest {
            repo: &repo,
            attempts: 2,
            mode,
            run_id: "incomplete-review",
            refresh: None,
        };
        let mut handle = start_with_adapter_token(
            request(),
            adapter,
            PullRequestFeedback::Consider,
            Some("test-token"),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(10), async {
            while authority.inspections.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .assert_value();
        // This is durable before any successful verifier result exists.
        let recovery: Value =
            serde_json::from_slice(&fs::read(&recovery_path).assert_value()).assert_value();
        let receipt = &recovery["deliveryReceipt"];
        assert_eq!(receipt["pullRequestId"], "17");
        assert!(!is_matching_success_receipt(receipt, mode, &target(&repo)));
        if cancel {
            handle.cancel();
            assert!(matches!(
                handle.completion().await,
                Err(NodeRunnerError::Cancelled)
            ));
        } else {
            assert_eq!(
                handle.completion().await.assert_value().outcome,
                WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
            );
        }
        let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        let pushes = authority.head_updates.load(Ordering::SeqCst);
        authority.closed.store(true, Ordering::SeqCst);
        let recovered =
            contract::review_from_receipt(receipt, mode, &target(&repo), branch).assert_value();
        let resumed = NativeV2DeliveryAdapter::new(
            NativeV2DeliveryConfig {
                adopt_existing_delivery: true,
                ..config
            },
            authority.clone(),
        )
        .with_recovered_review(Some(recovered))
        .with_review_binding_recorder(recorder);
        let execution = run_with_adapter(request(), Arc::new(resumed)).await;
        assert_eq!(
            execution.outcome,
            WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
        );
        assert_eq!(
            authority.delivery_read_reviews.lock().assert_value().last(),
            Some(&Some("17".to_owned()))
        );
        assert_eq!(authority.review_requests().len(), 1);
        assert_eq!(authority.head_updates.load(Ordering::SeqCst), pushes);
        assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), head);
    }
}

#[tokio::test]
async fn local_resume_confirms_an_identical_published_starting_commit_with_product_git() {
    for worktree in [false, true] {
        let mut repo = TempRepo::delivery();
        git(&repo.workspace, &["checkout", "-b", "feature/committed"]);
        crate::native_v2_candidate::test_support::commit_all(&repo.workspace, "committed work");
        repo.base = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        let invoking = repo.workspace.clone();
        let branch = if worktree {
            let branch = delivery_head_branch(&RunId::new("committed-resume"));
            let workspace = repo.root.child("retained-worktree");
            git(
                &invoking,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    workspace.to_str().assert_value(),
                    &repo.base,
                ],
            );
            repo.workspace = workspace;
            branch
        } else {
            "feature/committed".to_owned()
        };
        let invoking_status = git_output(&invoking, &["status", "--porcelain=v1"]);
        let config = NativeV2DeliveryConfig {
            local_branch: Some(branch.clone()),
            delivery_run_id: RunId::new("committed-resume"),
            adopt_existing_delivery: false,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: repo.workspace.clone(),
            git_program: "/usr/bin/git".into(),
            target: target(&repo),
            poll: DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
        };
        for resumed in [false, true] {
            let mut config = config.clone();
            config.adopt_existing_delivery = resumed;
            let transport = transport::GitTransport::new(
                transport::GitTransportConfig {
                    git_program: "/usr/bin/git".into(),
                    workspace: repo.workspace.clone(),
                    home_directory: repo.root.path().to_owned(),
                    private_directory: repo.root.child("transport"),
                    identity: None,
                    local_environment: Some(Default::default()),
                    deadline: Duration::from_secs(10),
                },
                &repo.remote,
            );
            let authority = Arc::new(
                crate::source_code_provider::adapters::core::GitDeliveryAuthority::new(transport),
            );
            let adapter = Arc::new(NativeV2DeliveryAdapter::new(config, authority));
            let execution = run_with_adapter_token(
                DeliveryRunRequest {
                    repo: &repo,
                    attempts: 1,
                    mode: DeliveryMode::Push,
                    run_id: if resumed {
                        "committed-successor"
                    } else {
                        "committed-original"
                    },
                    refresh: None,
                },
                adapter,
                PullRequestFeedback::Consider,
                None,
            )
            .await;
            assert_eq!(
                assert_delivery_signal(&execution.outcome, DELIVERY_PUSHED_LABEL)["headRevision"],
                repo.base,
            );
            assert_eq!(
                git_output(&repo.workspace, &["rev-parse", "HEAD"]),
                repo.base
            );
            assert_eq!(git_output(&repo.remote, &["rev-parse", &branch]), repo.base);
            assert_eq!(
                git_output(&repo.workspace, &["symbolic-ref", "--short", "HEAD"]),
                branch
            );
            assert_eq!(git_output(&invoking, &["rev-parse", "HEAD"]), repo.base);
            assert_eq!(
                git_output(&invoking, &["status", "--porcelain=v1"]),
                invoking_status
            );
        }
    }
}

#[tokio::test]
async fn unbound_local_resume_ignores_history_and_retains_bound_terminal_review_safeguards() {
    for resumed in [false, true] {
        let repo = TempRepo::delivery();
        let branch = "feature/reused";
        git(&repo.workspace, &["checkout", "-b", branch]);
        git(&repo.workspace, &["push", "origin", branch]);
        let authority = Arc::new(FakeGitHub::new(
            repo.remote.clone(),
            Script::HistoricalClosed,
        ));
        let config = NativeV2DeliveryConfig {
            local_branch: Some(branch.to_owned()),
            delivery_run_id: RunId::new("unbound-resume"),
            adopt_existing_delivery: resumed,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: repo.workspace.clone(),
            git_program: "/usr/bin/git".into(),
            target: target(&repo),
            poll: DeliveryPollPolicy::new(2, Duration::ZERO).assert_value(),
        };
        let request = || DeliveryRunRequest {
            repo: &repo,
            attempts: 2,
            mode: DeliveryMode::PullRequest,
            run_id: "unbound-successor",
            refresh: None,
        };
        let opened = run_with_adapter(
            request(),
            Arc::new(NativeV2DeliveryAdapter::new(
                config.clone(),
                authority.clone(),
            )),
        )
        .await;
        assert_eq!(
            assert_delivery_signal(&opened.outcome, DELIVERY_OPENED_LABEL)["pullRequestId"],
            "17",
        );
        assert!(
            !authority
                .review_requests()
                .last()
                .assert_value()
                .include_terminal_reviews
        );
        let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        let pushes = authority.head_updates.load(Ordering::SeqCst);
        let bound = NativeV2DeliveryAdapter::new(config, authority.clone()).with_recovered_review(
            Some(SourceReviewReceipt {
                review_id: "1".to_owned(),
                repository: target(&repo).repository,
                target_branch: target(&repo).target_branch,
                head_branch: branch.to_owned(),
                head_revision: head.clone(),
            }),
        );
        let refused = run_with_adapter(request(), Arc::new(bound)).await;
        assert_eq!(
            refused.outcome,
            WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
        );
        assert_eq!(authority.head_updates.load(Ordering::SeqCst), pushes);
        assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), head);
    }
}

#[tokio::test]
async fn merged_local_committed_source_is_confirmed_on_resume_without_republication() {
    let mut repo = TempRepo::delivery();
    git(&repo.workspace, &["checkout", "-b", "feature/committed"]);
    crate::native_v2_candidate::test_support::commit_all(&repo.workspace, "already committed work");
    repo.base = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
    let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
    let adapter = |adopting| {
        Arc::new(
            NativeV2DeliveryAdapter::new(
                NativeV2DeliveryConfig {
                    local_branch: Some("feature/committed".to_owned()),
                    delivery_run_id: RunId::new("committed-merge"),
                    adopt_existing_delivery: adopting,
                    git_identity: None,
                    commit_identity: None,
                    commit_co_author: None,
                    workspace: repo.workspace.clone(),
                    git_program: "/usr/bin/git".into(),
                    target: target(&repo),
                    poll: DeliveryPollPolicy::new(2, Duration::ZERO).assert_value(),
                },
                authority.clone(),
            )
            .with_recovered_review(adopting.then(|| SourceReviewReceipt {
                review_id: "17".to_owned(),
                repository: target(&repo).repository,
                target_branch: target(&repo).target_branch,
                head_branch: "feature/committed".to_owned(),
                head_revision: repo.base.clone(),
            })),
        )
    };
    let request = || DeliveryRunRequest {
        repo: &repo,
        attempts: 2,
        mode: DeliveryMode::Merge,
        run_id: "committed-merge",
        refresh: None,
    };
    let original = adapter(false);
    let review = run_with_adapter(request(), original.clone()).await;
    assert_delivery_signal(&review.outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
    let published = run_with_adapter(request(), original).await;
    assert_eq!(
        assert_delivery_signal(&published.outcome, DELIVERY_MERGED_LABEL)["headRevision"],
        repo.base
    );
    let pushes = authority.head_updates.load(Ordering::SeqCst);
    let resumed = run_with_adapter(request(), adapter(true)).await;
    assert_eq!(
        assert_delivery_signal(&resumed.outcome, DELIVERY_MERGED_LABEL)["headRevision"],
        repo.base
    );
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), pushes);
    assert_eq!(
        authority.delivery_read_reviews.lock().assert_value().last(),
        Some(&Some("17".to_owned()))
    );
    assert_eq!(
        git_output(&repo.workspace, &["symbolic-ref", "--short", "HEAD"]),
        "feature/committed"
    );

    fs::write(repo.workspace.join("new-work.txt"), "preserve this work\n").assert_value();
    let refused = run_with_adapter(request(), adapter(true)).await;
    assert_eq!(
        refused.outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
    );
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), pushes);
    assert_eq!(
        git_output(&repo.workspace, &["rev-parse", "HEAD"]),
        repo.base
    );
}

#[tokio::test]
async fn in_place_delivery_publishes_the_attached_branch_and_its_unpushed_commits() {
    for mode in [
        DeliveryMode::Push,
        DeliveryMode::PullRequest,
        DeliveryMode::Merge,
    ] {
        let repo = TempRepo::delivery();
        git(&repo.workspace, &["checkout", "-b", "feature/publish"]);
        fs::write(repo.workspace.join("prior.txt"), "earlier work\n").assert_value();
        git(&repo.workspace, &["add", "prior.txt"]);
        git(
            &repo.workspace,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "prior unpushed",
            ],
        );
        let prior = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        git(&repo.workspace, &["push", "origin", "feature/publish"]);
        fs::write(
            repo.workspace.join("extra.txt"),
            "another unpushed commit\n",
        )
        .assert_value();
        git(&repo.workspace, &["add", "extra.txt"]);
        git(
            &repo.workspace,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "unpushed",
            ],
        );
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
        let adapter = Arc::new(NativeV2DeliveryAdapter::new(
            NativeV2DeliveryConfig {
                local_branch: Some("feature/publish".to_owned()),
                delivery_run_id: RunId::new("in-place"),
                adopt_existing_delivery: false,
                git_identity: None,
                commit_identity: None,
                commit_co_author: None,
                workspace: repo.workspace.clone(),
                git_program: "/usr/bin/git".into(),
                target: target(&repo),
                poll: DeliveryPollPolicy::new(2, Duration::ZERO).assert_value(),
            },
            authority.clone(),
        ));
        let execution = run_with_adapter(
            DeliveryRunRequest {
                repo: &repo,
                attempts: 2,
                mode,
                run_id: "in-place",
                refresh: None,
            },
            adapter,
        )
        .await;
        let receipt = assert_delivery_signal(&execution.outcome, mode.success_outcome());
        if mode.creates_review() {
            assert!(
                !authority
                    .review_requests()
                    .last()
                    .assert_value()
                    .include_terminal_reviews
            );
        }
        if mode == DeliveryMode::Push {
            assert_eq!(receipt["headBranch"], "feature/publish");
        }
        assert!(
            authority
                .delivery_read_branches
                .lock()
                .assert_value()
                .iter()
                .all(|branch| branch == "feature/publish")
        );
        let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        assert_eq!(
            git_output(&repo.workspace, &["symbolic-ref", "--short", "HEAD"]),
            "feature/publish"
        );
        assert_eq!(
            git_output(&repo.remote, &["rev-parse", "feature/publish"]),
            head
        );
        git(
            &repo.workspace,
            &["merge-base", "--is-ancestor", &prior, &head],
        );
        for path in [&repo.workspace, &repo.remote] {
            assert!(
                git_output(
                    path,
                    &["for-each-ref", "--format=%(refname)", "refs/heads/zeroshot"]
                )
                .is_empty()
            );
        }
        assert_eq!(
            git_output(&repo.workspace, &["show", "HEAD:extra.txt"]),
            "another unpushed commit"
        );
    }
}

#[tokio::test]
async fn explicit_push_can_publish_the_default_branch() {
    let repo = TempRepo::delivery();
    let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
    let adapter = Arc::new(NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            local_branch: Some("main".to_owned()),
            delivery_run_id: RunId::new("push-main"),
            adopt_existing_delivery: false,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: repo.workspace.clone(),
            git_program: "/usr/bin/git".into(),
            target: target(&repo),
            poll: DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
        },
        authority,
    ));
    let execution = run_with_adapter(
        DeliveryRunRequest {
            repo: &repo,
            attempts: 1,
            mode: DeliveryMode::Push,
            run_id: "push-main",
            refresh: None,
        },
        adapter,
    )
    .await;
    assert_delivery_signal(&execution.outcome, DELIVERY_PUSHED_LABEL);
    assert_eq!(
        git_output(&repo.workspace, &["rev-parse", "main"]),
        git_output(&repo.remote, &["rev-parse", "main"])
    );
}

#[tokio::test]
async fn resumed_local_delivery_reuses_an_identical_published_candidate() {
    let repo = TempRepo::delivery();
    let config = NativeV2DeliveryConfig {
        local_branch: Some("main".to_owned()),
        delivery_run_id: RunId::new("local-resume"),
        adopt_existing_delivery: false,
        git_identity: None,
        commit_identity: None,
        commit_co_author: None,
        workspace: repo.workspace.clone(),
        git_program: "/usr/bin/git".into(),
        target: target(&repo),
        poll: DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
    };
    let mut published = None;
    for resumed in [false, true] {
        let mut config = config.clone();
        config.adopt_existing_delivery = resumed;
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
        let adapter = Arc::new(NativeV2DeliveryAdapter::new(config, authority));
        let execution = run_with_adapter(
            DeliveryRunRequest {
                repo: &repo,
                attempts: 1,
                mode: DeliveryMode::Push,
                run_id: if resumed {
                    "local-resumed"
                } else {
                    "local-original"
                },
                refresh: None,
            },
            adapter,
        )
        .await;
        assert_delivery_signal(&execution.outcome, DELIVERY_PUSHED_LABEL);
        let current = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        if let Some(published) = &published {
            assert_eq!(&current, published);
        }
        published = Some(current);
        assert_eq!(
            git_output(&repo.workspace, &["symbolic-ref", "--short", "HEAD"]),
            "main"
        );
    }
}

#[tokio::test]
async fn a_non_fast_forward_local_push_is_a_refusal_without_repair() {
    let repo = TempRepo::delivery();
    git(&repo.workspace, &["checkout", "-b", "feature/refused"]);
    let authority = Arc::new(FakeGitHub::new(
        repo.remote.clone(),
        Script::PushNonFastForward,
    ));
    let adapter = Arc::new(NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            local_branch: Some("feature/refused".to_owned()),
            delivery_run_id: RunId::new("refused-local"),
            adopt_existing_delivery: false,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: repo.workspace.clone(),
            git_program: "/usr/bin/git".into(),
            target: target(&repo),
            poll: DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
        },
        authority,
    ));
    let execution = run_with_adapter(
        DeliveryRunRequest {
            repo: &repo,
            attempts: 1,
            mode: DeliveryMode::Push,
            run_id: "refused-local",
            refresh: None,
        },
        adapter,
    )
    .await;
    assert_eq!(
        execution.outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
    );
    assert!(
        execution
            .output
            .iter()
            .any(|record| record.text.contains("feature/refused")
                && record.text.contains("fast-forward"))
    );
}
