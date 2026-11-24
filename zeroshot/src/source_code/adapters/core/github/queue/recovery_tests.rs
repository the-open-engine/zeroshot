use super::*;

pub(super) fn recovery_policy(predecessor: bool) -> Value {
    let mut page = queued(false, false);
    let nodes = if predecessor {
        json!([{"id":"ENTRY_A","enqueuedAt":"2026-11-09T00:30:00Z","jump":false,
            "pullRequest":{"id":"PR_A","baseRefName":"main","repository":{"nameWithOwner":"acme/project"}}}])
    } else {
        json!([])
    };
    page["data"]["repository"]["mergeQueue"] = json!({"id":"MQ_MAIN","entries":{
        "totalCount":nodes.as_array().assert_value().len(),"nodes":nodes,
        "queuePageInfo":{"hasNextPage":false,"endCursor":null}
    }});
    page
}

fn set_policy(root: &std::path::Path, page: Value) {
    std::fs::write(root.join("policy-0.json"), json!([page]).to_string()).assert_value();
}

fn set_conflict_removal(root: &std::path::Path, id: &str) {
    let mut outcome = removed_queue_candidate("2026-11-09T01:30:00Z", "merge_conflict", vec![]);
    let removed = &mut outcome["data"]["repository"]["pullRequest"]["timelineItems"]["nodes"][0];
    removed["id"] = json!(id);
    removed["beforeCommit"] = Value::Null;
    std::fs::write(
        root.join("queue-outcome.json"),
        json!([outcome]).to_string(),
    )
    .assert_value();
}

async fn inspect(provider: &GhCliDeliveryAuthority) -> SourceReviewState {
    provider
        .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_value()
        .state
}

#[tokio::test]
async fn admitted_pr_retains_failed_checks_and_logs_without_removal_evidence() {
    for in_queue in [true, false] {
        let root = tempfile::tempdir().assert_value();
        let provider = queue_authority(
            root.path(),
            &[queued(false, false)],
            &[
                ("enqueuePullRequest", &response(&enqueue_response())),
                (
                    "dequeuePullRequest",
                    "/bin/cp \"$HOME/withdrawn-policy.json\" \"$HOME/policy-0.json\"; printf '{}'",
                ),
            ],
        );
        enqueue_accepted(&provider).await;
        let mut policy = queued(in_queue, false);
        policy["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("BLOCKED");
        policy["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] = json!({"contexts":{
            "nodes":[check_run("required", "COMPLETED", Some("FAILURE"), true)],
            "pageInfo":{"hasNextPage":false,"endCursor":null}
        }});
        set_policy(root.path(), policy.clone());
        policy["data"]["repository"]["pullRequest"]["isInMergeQueue"] = json!(false);
        std::fs::write(
            root.path().join("withdrawn-policy.json"),
            json!([policy]).to_string(),
        )
        .assert_value();
        std::fs::write(
            root.path().join("job-log"),
            "##[error]required check failed",
        )
        .assert_value();
        for _ in 0..2 {
            let SourceReviewState::Open {
                checks: SourceChecks::Failed { diagnostic },
            } = inspect(&provider).await
            else {
                panic!("an admission must preserve exact-head required-check failure feedback");
            };
            assert!(
                diagnostic.contains("required concluded FAILURE"),
                "{diagnostic}"
            );
            assert!(diagnostic.contains("required check failed"), "{diagnostic}");
            assert_eq!(
                provider
                    .request_merge(
                        &review(),
                        SourceDeliveryCredential::token("test-token"),
                        &TestReviewCheckpoint,
                    )
                    .await
                    .assert_value(),
                SourceMergeRequestOutcome::Pending
            );
            assert_eq!(enqueue_count(root.path()), 1);
        }
        assert_withdrawn(&provider).await;
        let mutations = std::fs::read_to_string(root.path().join("mutations")).assert_value();
        assert_eq!(
            mutations.matches("dequeuePullRequest").count(),
            usize::from(in_queue)
        );
        assert_eq!(enqueue_count(root.path()), 1);
    }
}

#[tokio::test]
async fn queue_conflict_removal_preserves_queue_commit_failures_before_recovery() {
    for predecessor in [true, false] {
        for conflicting in [true, false] {
            let root = tempfile::tempdir().assert_value();
            let provider = admission(root.path(), &response(&enqueue_response()));
            enqueue_accepted(&provider).await;
            let mut policy = recovery_policy(predecessor);
            if conflicting {
                policy["data"]["repository"]["pullRequest"]["mergeable"] = json!("CONFLICTING");
                policy["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("DIRTY");
            }
            set_policy(root.path(), policy);
            let outcome = removed_queue_candidate(
                "2026-11-09T01:30:00Z",
                "merge_conflict",
                vec![check_run(
                    "queue-required",
                    "COMPLETED",
                    Some("FAILURE"),
                    true,
                )],
            );
            std::fs::write(
                root.path().join("queue-outcome.json"),
                json!([outcome]).to_string(),
            )
            .assert_value();
            std::fs::write(
                root.path().join("job-log"),
                "##[error]queue candidate required check failed",
            )
            .assert_value();
            for _ in 0..2 {
                let SourceReviewState::Open {
                    checks: SourceChecks::Failed { diagnostic },
                } = inspect(&provider).await
                else {
                    panic!("queue candidate failures must precede conflict recovery");
                };
                for expected in [
                    "merge_conflict",
                    &"c".repeat(40),
                    "queue-required concluded FAILURE",
                    "queue candidate required check failed",
                ] {
                    assert!(diagnostic.contains(expected), "{diagnostic}");
                }
                assert_eq!(
                    provider
                        .request_merge(
                            &review(),
                            SourceDeliveryCredential::token("test-token"),
                            &TestReviewCheckpoint,
                        )
                        .await
                        .assert_value(),
                    SourceMergeRequestOutcome::Pending
                );
                assert_eq!(enqueue_count(root.path()), 1);
            }
            assert_withdrawn(&provider).await;
            assert_eq!(enqueue_count(root.path()), 1);
        }
    }
}

#[tokio::test]
async fn queue_conflict_waits_then_readmits_the_same_authority_and_observes_merge() {
    let root = tempfile::tempdir().assert_value();
    let provider = admission(root.path(), &response(&enqueue_response()));
    enqueue_accepted(&provider).await;
    set_conflict_removal(root.path(), "REMOVAL_B");
    set_policy(root.path(), recovery_policy(true));
    for _ in 0..2 {
        assert_eq!(
            inspect(&provider).await,
            SourceReviewState::Open {
                checks: SourceChecks::Pending
            }
        );
        assert_eq!(
            provider
                .request_merge(
                    &review(),
                    SourceDeliveryCredential::token("test-token"),
                    &TestReviewCheckpoint
                )
                .await
                .assert_value(),
            SourceMergeRequestOutcome::Pending
        );
        assert_eq!(enqueue_count(root.path()), 1);
    }
    set_policy(root.path(), recovery_policy(false));
    enqueue_accepted(&provider).await;
    assert_eq!(
        provider
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    assert_eq!(enqueue_count(root.path()), 2);
    let mut merged = recovery_policy(false);
    let pull = &mut merged["data"]["repository"]["pullRequest"];
    pull["merged"] = json!(true);
    pull["state"] = json!("MERGED");
    pull["mergeCommit"] = json!({"oid":"f".repeat(40)});
    set_policy(root.path(), merged);
    assert_eq!(
        inspect(&provider).await,
        SourceReviewState::Merged {
            merge_revision: "f".repeat(40)
        }
    );
    assert_eq!(enqueue_count(root.path()), 2);
}

#[tokio::test]
async fn queue_conflict_waits_before_exposing_a_predecessors_base_conflict() {
    let root = tempfile::tempdir().assert_value();
    let provider = admission(root.path(), &response(&enqueue_response()));
    enqueue_accepted(&provider).await;
    set_conflict_removal(root.path(), "REMOVAL_B");
    let mut page = recovery_policy(true);
    page["data"]["repository"]["pullRequest"]["mergeable"] = json!("CONFLICTING");
    page["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("DIRTY");
    set_policy(root.path(), page.clone());
    assert_eq!(
        inspect(&provider).await,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    page["data"]["repository"]["mergeQueue"]["entries"]["nodes"] = json!([]);
    page["data"]["repository"]["mergeQueue"]["entries"]["totalCount"] = json!(0);
    set_policy(root.path(), page);
    assert_eq!(inspect(&provider).await, SourceReviewState::Conflict);
    assert_eq!(enqueue_count(root.path()), 1);
    assert_withdrawn(&provider).await;
}

#[tokio::test]
async fn queue_conflict_missing_or_stale_removal_keeps_conflicting_pr_pending() {
    let root = tempfile::tempdir().assert_value();
    let provider = admission(root.path(), &response(&enqueue_response()));
    enqueue_accepted(&provider).await;
    set_conflict_removal(root.path(), "REMOVAL_B");
    let path = root.path().join("queue-outcome.json");
    let complete: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).assert_value()).assert_value();
    for predecessor in [true, false] {
        let mut policy = recovery_policy(predecessor);
        policy["data"]["repository"]["pullRequest"]["mergeable"] = json!("CONFLICTING");
        policy["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("DIRTY");
        policy["data"]["repository"]["pullRequest"]["isInMergeQueue"] = json!(true);
        set_policy(root.path(), policy.clone());
        assert_eq!(
            inspect(&provider).await,
            SourceReviewState::Open {
                checks: SourceChecks::Pending
            }
        );
        policy["data"]["repository"]["pullRequest"]["isInMergeQueue"] = json!(false);
        set_policy(root.path(), policy);
        for unavailable in [None, Some("2026-11-09T00:59:59Z")] {
            let mut outcome = complete.clone();
            let events =
                &mut outcome[0]["data"]["repository"]["pullRequest"]["timelineItems"]["nodes"];
            if let Some(timestamp) = unavailable {
                events[0]["createdAt"] = json!(timestamp);
            } else {
                *events = json!([]);
            }
            std::fs::write(&path, outcome.to_string()).assert_value();
            assert_eq!(
                inspect(&provider).await,
                SourceReviewState::Open {
                    checks: SourceChecks::Pending
                }
            );
            assert_eq!(
                provider
                    .request_merge(
                        &review(),
                        SourceDeliveryCredential::token("test-token"),
                        &TestReviewCheckpoint,
                    )
                    .await
                    .assert_value(),
                SourceMergeRequestOutcome::Pending
            );
            assert_eq!(enqueue_count(root.path()), 1);
        }
        std::fs::write(&path, complete.to_string()).assert_value();
        assert_eq!(
            inspect(&provider).await,
            if predecessor {
                SourceReviewState::Open {
                    checks: SourceChecks::Pending,
                }
            } else {
                SourceReviewState::Conflict
            }
        );
    }
    assert_withdrawn(&provider).await;
}

#[tokio::test]
async fn queue_conflict_recovery_exhausts_without_ordinary_reads_renewing_it() {
    let root = tempfile::tempdir().assert_value();
    let provider = admission(root.path(), &response(&enqueue_response()));
    enqueue_accepted(&provider).await;
    set_policy(root.path(), recovery_policy(false));
    for index in 0..3 {
        set_conflict_removal(root.path(), &format!("REMOVAL_{index}"));
        for _ in 0..2 {
            inspect(&provider).await;
        }
        enqueue_accepted(&provider).await;
        assert_eq!(enqueue_count(root.path()), index + 2);
    }
    set_conflict_removal(root.path(), "REMOVAL_EXHAUSTED");
    let error = provider
        .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_error();
    assert!(
        error.to_string().contains("recovery allowance exhausted"),
        "{error}"
    );
    assert!(!error.retryable_operation());
    assert_eq!(enqueue_count(root.path()), 4);
    assert_withdrawn(&provider).await;
}

#[tokio::test]
async fn queue_conflict_readmission_requires_complete_current_policy() {
    for change in [
        "unknown",
        "missing_queue",
        "partial_queue",
        "wrong_base",
        "wrong_repository",
        "changed_head",
        "required_check",
        "changing_pages",
    ] {
        let root = tempfile::tempdir().assert_value();
        let provider = admission(root.path(), &response(&enqueue_response()));
        enqueue_accepted(&provider).await;
        set_conflict_removal(root.path(), "REMOVAL_B");
        let mut page = recovery_policy(false);
        let repository = &mut page["data"]["repository"];
        match change {
            "unknown" => repository["pullRequest"]["mergeable"] = json!("UNKNOWN"),
            "missing_queue" => repository["mergeQueue"] = Value::Null,
            "partial_queue" => {
                repository["mergeQueue"]["entries"]["queuePageInfo"] =
                    json!({"hasNextPage":true,"endCursor":"next"})
            }
            "wrong_base" | "wrong_repository" => {
                *repository = recovery_policy(true)["data"]["repository"].clone();
                let pull = &mut repository["mergeQueue"]["entries"]["nodes"][0]["pullRequest"];
                if change == "wrong_base" {
                    pull["baseRefName"] = json!("release");
                } else {
                    pull["repository"]["nameWithOwner"] = json!("other/repo");
                }
            }
            "changed_head" => repository["pullRequest"]["headRefOid"] = json!("d".repeat(40)),
            "required_check" => {
                std::fs::write(root.path().join("branch-rules.json"), json!([[{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"new-policy","integration_id":null}]}}]]).to_string()).assert_value();
            }
            _ => {}
        }
        if change == "changing_pages" {
            page["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] = json!({"contexts":{
                "nodes":[check_run("required", "COMPLETED", Some("SUCCESS"), true)],
                "pageInfo":{"hasNextPage":false,"endCursor":null}
            }});
            let contexts =
                "/data/repository/pullRequest/commits/nodes/0/commit/statusCheckRollup/contexts";
            let mut second = page.clone();
            page.pointer_mut(contexts).assert_value()["pageInfo"] =
                json!({"hasNextPage":true,"endCursor":"first"});
            second["data"]["repository"]["mergeQueue"]["id"] = json!("CHANGED_QUEUE");
            std::fs::write(
                root.path().join("policy-0.json"),
                json!([page, second]).to_string(),
            )
            .assert_value();
        } else {
            set_policy(root.path(), page);
        }
        let result = provider
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await;
        assert!(
            matches!(result, Ok(SourceMergeRequestOutcome::Pending) | Err(_)),
            "{change}: {result:?}"
        );
        assert_eq!(enqueue_count(root.path()), 1, "{change}");
    }
}

#[tokio::test]
async fn other_removals_without_failed_checks_are_errors_with_removal_evidence() {
    for reason in ["manual", "checks_timed_out", "failed_checks"] {
        let root = tempfile::tempdir().assert_value();
        let provider = admission(root.path(), &response(&enqueue_response()));
        let mut outcome = removed_queue_candidate("2026-11-09T01:30:00Z", reason, vec![]);
        outcome["data"]["repository"]["pullRequest"]["timelineItems"]["nodes"][0]["beforeCommit"] =
            Value::Null;
        let error = inspect_outcome(root.path(), &provider, json!([outcome]))
            .await
            .assert_error();
        assert!(error.to_string().contains(reason), "{error}");
        assert!(
            !error.to_string().contains("Required CI checks failed:"),
            "{error}"
        );
        assert!(!error.retryable_operation());
        assert_eq!(enqueue_count(root.path()), 1);
        assert_withdrawn(&provider).await;
    }
}

#[tokio::test]
async fn queue_conflict_recovery_does_not_reuse_missing_or_ambiguous_removal_evidence() {
    for change in ["missing", "same_second", "stale"] {
        let root = tempfile::tempdir().assert_value();
        let provider = admission(root.path(), &response(&enqueue_response()));
        enqueue_accepted(&provider).await;
        set_policy(root.path(), recovery_policy(false));
        set_conflict_removal(root.path(), "REMOVAL_B");
        inspect(&provider).await;
        let path = root.path().join("queue-outcome.json");
        let mut outcome: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).assert_value()).assert_value();
        let pull = &mut outcome[0]["data"]["repository"]["pullRequest"];
        match change {
            "missing" => pull["timelineItems"]["nodes"] = json!([]),
            "same_second" => {
                pull["timelineItems"]["nodes"][0]["createdAt"] = json!("2026-11-09T01:00:00Z")
            }
            _ => pull["timelineItems"]["nodes"][0]["createdAt"] = json!("2026-11-09T00:59:59Z"),
        }
        std::fs::write(path, outcome.to_string()).assert_value();
        let result = provider
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await;
        if change == "same_second" {
            result.assert_error();
        } else {
            result.assert_value();
        }
        assert_eq!(enqueue_count(root.path()), 1, "{change}");
    }
}

#[tokio::test]
async fn queue_conflict_recovery_preserves_earlier_submission_uncertainty() {
    let root = tempfile::tempdir().assert_value();
    let action = format!(
        "if [ ! -f \"$HOME/attempted\" ]; then /usr/bin/touch \"$HOME/attempted\"; printf 'HTTP 503: unavailable\\n' >&2; exit 1; fi; {}",
        response(&enqueue_response())
    );
    let provider = admission(root.path(), &action);
    provider
        .request_merge(
            &review(),
            SourceDeliveryCredential::token("test-token"),
            &TestReviewCheckpoint,
        )
        .await
        .assert_error();
    enqueue_accepted(&provider).await;
    set_policy(root.path(), recovery_policy(false));
    set_conflict_removal(root.path(), "REMOVAL_B");
    let error = provider
        .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_error();
    assert!(
        error.to_string().contains("unconfirmed submission"),
        "{error}"
    );
    assert_eq!(enqueue_count(root.path()), 2);
    provider
        .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_error();
}

#[tokio::test]
async fn cancelled_enqueue_observation_settles_its_finished_admission_before_conflict() {
    let root = tempfile::tempdir().assert_value();
    let action = format!(
        "printf started > \"$HOME/started\"; while [ ! -f \"$HOME/allow-completion\" ]; do /bin/sleep 0.005; done; {}",
        response(&enqueue_response())
    );
    let provider = Arc::new(admission(root.path(), &action));
    let owner = provider.clone();
    let enqueue = tokio::spawn(async move { enqueue_accepted(&owner).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !root.path().join("started").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .assert_value();
    enqueue.abort();
    enqueue.await.assert_error();
    let mut conflict = recovery_policy(false);
    conflict["data"]["repository"]["pullRequest"]["mergeable"] = json!("CONFLICTING");
    conflict["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("DIRTY");
    set_policy(root.path(), conflict);
    set_conflict_removal(root.path(), "REMOVAL_B");
    assert_eq!(
        inspect(&provider).await,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    std::fs::write(root.path().join("allow-completion"), "").assert_value();
    tokio::time::timeout(Duration::from_secs(5), async {
        while inspect(&provider).await != SourceReviewState::Conflict {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .assert_value();
    assert_eq!(enqueue_count(root.path()), 1);
    assert_withdrawn(&provider).await;
}

#[tokio::test]
async fn uncertain_recovery_cannot_expose_a_conflict_before_withdrawal() {
    for (head_update, failed_check) in [(false, false), (true, false), (true, true)] {
        let root = tempfile::tempdir().assert_value();
        let action = format!(
            "if [ ! -f \"$HOME/attempted\" ]; then /usr/bin/touch \"$HOME/attempted\"; {}; else /bin/cp \"$HOME/blocked-policy.json\" \"$HOME/policy-0.json\"; /bin/cp \"$HOME/missing-outcome.json\" \"$HOME/queue-outcome.json\"; printf 'HTTP 503: unavailable\\n' >&2; exit 1; fi",
            response(&enqueue_response())
        );
        let provider = admission(root.path(), &action);
        enqueue_accepted(&provider).await;
        set_policy(root.path(), recovery_policy(false));
        set_conflict_removal(root.path(), "REMOVAL_B");
        let mut policy = recovery_policy(true);
        policy["data"]["repository"]["mergeQueue"]["entries"]["nodes"][0]["enqueuedAt"] =
            json!("2026-11-09T02:00:00Z");
        policy["data"]["repository"]["mergeQueue"]["entries"]["nodes"][0]["jump"] = json!(true);
        let pull = &mut policy["data"]["repository"]["pullRequest"];
        if head_update {
            pull["mergeable"] = json!("MERGEABLE");
            pull["mergeStateStatus"] = json!("BEHIND");
            pull["isMergeQueueEnabled"] = json!(false);
            pull["baseRef"]["compare"]["behindBy"] = json!(2);
        } else {
            pull["mergeable"] = json!("CONFLICTING");
            pull["mergeStateStatus"] = json!("DIRTY");
        }
        if failed_check {
            pull["commits"]["nodes"][0]["commit"]["statusCheckRollup"] = json!({"contexts":{
                "nodes":[check_run("required", "COMPLETED", Some("FAILURE"), true)],
                "pageInfo":{"hasNextPage":false,"endCursor":null}
            }});
            std::fs::write(
                root.path().join("job-log"),
                "##[error]required check failed",
            )
            .assert_value();
        }
        std::fs::write(
            root.path().join("blocked-policy.json"),
            json!([policy]).to_string(),
        )
        .assert_value();
        let path = root.path().join("queue-outcome.json");
        let mut outcome: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).assert_value()).assert_value();
        outcome[0]["data"]["repository"]["pullRequest"]["timelineItems"]["nodes"] = json!([]);
        std::fs::write(
            root.path().join("missing-outcome.json"),
            outcome.to_string(),
        )
        .assert_value();
        assert!(
            provider
                .request_merge(
                    &review(),
                    SourceDeliveryCredential::token("test-token"),
                    &TestReviewCheckpoint,
                )
                .await
                .assert_error()
                .retryable_operation()
        );
        let state = inspect(&provider).await;
        if failed_check {
            let SourceReviewState::Open {
                checks: SourceChecks::Failed { diagnostic },
            } = state
            else {
                panic!("an uncertain admission must retain genuine CI failure feedback");
            };
            assert!(diagnostic.contains("required check failed"), "{diagnostic}");
        } else {
            assert_eq!(
                state,
                SourceReviewState::Open {
                    checks: SourceChecks::Pending
                }
            );
        }
        assert_eq!(
            provider
                .request_merge(
                    &review(),
                    SourceDeliveryCredential::token("test-token"),
                    &TestReviewCheckpoint,
                )
                .await
                .assert_value(),
            SourceMergeRequestOutcome::Pending
        );
        assert_eq!(enqueue_count(root.path()), 2);
        assert!(
            provider
                .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
                .await
                .assert_error()
                .to_string()
                .contains("submission remains unconfirmed")
        );
    }
}
#[tokio::test]
async fn superseded_admission_cannot_expose_an_older_conflict_observation() {
    let root = tempfile::tempdir().assert_value();
    let receipt_path = root.path().join("enqueue-dynamic.json");
    std::fs::write(&receipt_path, enqueue_response().to_string()).assert_value();
    let provider = Arc::new(admission(
        root.path(),
        "/bin/cat \"$HOME/enqueue-dynamic.json\"",
    ));
    let program = root.path().join("gh-queue-fixture");
    let source = std::fs::read_to_string(&program).assert_value();
    write_executable(
        &program,
        source.replace(
            "*'query QueueOutcome('*) /bin/cat",
            "*'query QueueOutcome('*) if [ -f \"$HOME/block-next\" ]; then /bin/rm \"$HOME/block-next\"; printf blocked > \"$HOME/blocked\"; while [ ! -f \"$HOME/release\" ]; do /bin/sleep 0.005; done; fi; /bin/cat",
        ),
    );
    enqueue_accepted(&provider).await;
    set_conflict_removal(root.path(), "REMOVAL_B");
    let mut conflict = recovery_policy(false);
    conflict["data"]["repository"]["pullRequest"]["mergeable"] = json!("CONFLICTING");
    conflict["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("DIRTY");
    set_policy(root.path(), conflict);
    std::fs::write(root.path().join("block-next"), "").assert_value();
    let observer = provider.clone();
    let stalled = tokio::spawn(async move { inspect(&observer).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !root.path().join("blocked").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .assert_value();
    set_policy(root.path(), recovery_policy(false));
    let mut receipt = enqueue_response();
    receipt["data"]["enqueuePullRequest"]["mergeQueueEntry"]["id"] = json!("NEW_ENTRY");
    receipt["data"]["enqueuePullRequest"]["mergeQueueEntry"]["enqueuedAt"] =
        json!("2026-11-09T02:00:00Z");
    std::fs::write(receipt_path, receipt.to_string()).assert_value();
    enqueue_accepted(&provider).await;
    std::fs::write(root.path().join("release"), "").assert_value();
    assert_eq!(
        stalled.await.assert_value(),
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    assert_eq!(enqueue_count(root.path()), 2);
    assert_withdrawn(&provider).await;
}

#[tokio::test]
async fn recovered_removal_precedes_same_second_ambiguity_on_the_new_admission() {
    let root = tempfile::tempdir().assert_value();
    let receipt_path = root.path().join("enqueue-dynamic.json");
    std::fs::write(&receipt_path, enqueue_response().to_string()).assert_value();
    let provider = admission(root.path(), "/bin/cat \"$HOME/enqueue-dynamic.json\"");
    enqueue_accepted(&provider).await;
    set_policy(root.path(), recovery_policy(false));
    let mut outcome = removed_queue_candidate(
        "2026-11-09T01:30:00Z",
        "merge_conflict",
        vec![check_run("old-queue-check", "IN_PROGRESS", None, true)],
    );
    outcome["data"]["repository"]["pullRequest"]["timelineItems"]["nodes"][0]["id"] =
        json!("REMOVAL_B");
    let outcome_path = root.path().join("queue-outcome.json");
    std::fs::write(&outcome_path, json!([outcome.clone()]).to_string()).assert_value();
    let mut receipt = enqueue_response();
    receipt["data"]["enqueuePullRequest"]["mergeQueueEntry"]["id"] = json!("ENTRY_B_NEW");
    receipt["data"]["enqueuePullRequest"]["mergeQueueEntry"]["enqueuedAt"] =
        json!("2026-11-09T01:30:00Z");
    std::fs::write(receipt_path, receipt.to_string()).assert_value();
    enqueue_accepted(&provider).await;
    // Queue membership can lag the receipt: the old removal is already reconciled,
    // even when it shares the new admission's second-precision timestamp.
    let mut conflict = recovery_policy(false);
    conflict["data"]["repository"]["pullRequest"]["mergeable"] = json!("CONFLICTING");
    conflict["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("DIRTY");
    set_policy(root.path(), conflict);
    assert_eq!(
        inspect(&provider).await,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    // The old queue commit may finish after readmission, without becoming current evidence.
    outcome["data"]["repository"]["pullRequest"]["timelineItems"]["nodes"][0]["beforeCommit"]["statusCheckRollup"]
        ["contexts"]["nodes"][0] = check_run("old-queue-check", "COMPLETED", Some("FAILURE"), true);
    std::fs::write(outcome_path, json!([outcome]).to_string()).assert_value();
    assert_eq!(
        inspect(&provider).await,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    assert_eq!(
        provider
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    assert_eq!(enqueue_count(root.path()), 2);
    set_conflict_removal(root.path(), "UNCONFIRMED_NEW_REMOVAL");
    provider
        .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_error();
    assert_eq!(enqueue_count(root.path()), 2);
}
