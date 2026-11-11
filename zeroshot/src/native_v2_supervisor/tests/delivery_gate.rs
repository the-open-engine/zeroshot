use openengine_cluster_testkit::fixture::run_size;
use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    DeclaredConnections, EnumLabel, FieldName, IdempotencyKey, NodeName, PositiveInteger, RunId,
    RunTitle, SessionScope, SourceBranchId, SourceRepositoryId, SourceRevisionId, ResolvedSource,
    TerminalResult, WorkerOutcome,
};
use serde_json::{Value, json};

fn enforce_delivery_terminal(
    policy: DeliveryPolicy,
    admitted: &AdmittedRun,
    snapshot: &RunSnapshot,
    terminal: TerminalResult,
) -> Result<TerminalResult, super::super::NativeV2SupervisorError> {
    super::super::enforce_delivery_terminal(policy, admitted, snapshot, terminal)
        .map(|checked| checked.result)
}
use crate::full_v1_reducer::StructuralOccurrence;
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_candidate::test_support::{full_graph, git_delivery_node, success_node};
use crate::native_v2_contract::{
    AdmittedRun, ExecutionId, ExecutionRef, NodeInstanceId, NodeRuntimeBinding, ReasoningEffort,
    RunSubmission,
};
use crate::native_v2_delivery::{DELIVERY_MERGED_LABEL, DELIVERY_SIGNAL_FIELD};
use crate::v2_run_ledger::{NodeSnapshot, NodeState, RunPhase, RunSnapshot, cursor_for};
use openengine_cluster_testkit::assertions::AssertValue;

fn agent_binding() -> NodeRuntimeBinding {
    NodeRuntimeBinding::Agent {
        lane: None,
        model: crate::worker_catalog::ModelId::new("gpt-5.6-sol").assert_value(),
        effort: Some(ReasoningEffort::Max),
        harness_config: None,
        context_window: None,
        max_tokens: None,
        session_scope: SessionScope::Execution,
        connections: DeclaredConnections::empty(),
    }
}

fn source() -> ResolvedSource {
    ResolvedSource {
        remote: None,

        repository: SourceRepositoryId::new("acme/project").assert_value(),
        branch: SourceBranchId::new("main").assert_value(),
        revision: SourceRevisionId::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").assert_value(),
    }
}

async fn admitted_with_delivery() -> AdmittedRun {
    admitted_with_git_mode(crate::native_v2_delivery::DeliveryMode::Merge).await
}

async fn admitted_with_git_mode(mode: crate::native_v2_delivery::DeliveryMode) -> AdmittedRun {
    let mut git_node = git_delivery_node();
    git_node["worker"] = json!(mode.worker_ref());
    git_node["output"] = serde_json::to_value(
        crate::native_v2_delivery::delivery_result_schema(mode).assert_value(),
    )
    .assert_value();
    git_node["signals"] =
        json!({"delivery": crate::native_v2_delivery::delivery_signal_labels(mode).assert_value()});
    let graph = full_graph(vec![
        json!({
            "kind":"step","name":"worker","worker":"agent.worker@1",
            "instructions":"Implement the requested change.",
            "input":{"kind":"null"},"output":{"kind":"null"},
            "inputBindings":[],"writeBindings":[],"timeoutMs":1000,"attempts":1
        }),
        json!({
            "kind":"verifier","name":"verify","worker":"agent.verify@1",
            "instructions":"Verify the completed delivery.",
            "input":{"kind":"null"},"output":{"kind":"null"},
            "inputBindings":[],"writeBindings":[],"timeoutMs":1000,"attempts":1,
            "signals":{"verdict":["accepted"]},"diagnostic":{"kind":"null"}
        }),
        git_node,
        success_node(),
    ]);
    let runtime = crate::native_v2_candidate::test_support::codex_runtime(
        run_size("medium"),
        BTreeMap::from([
            (NodeName::new("worker").assert_value(), agent_binding()),
            (
                NodeName::new("deliver").assert_value(),
                NodeRuntimeBinding::GitDelivery {
                    connections: DeclaredConnections::empty(),
                    pull_request_feedback: Default::default(),
                },
            ),
            (NodeName::new("verify").assert_value(), agent_binding()),
        ]),
    );
    NativeV2Admission
        .admit_with_policy(
            RunSubmission {
                environment: None,
                title: RunTitle::new("Required delivery gate").assert_value(),
                graph,
                initial_input: Value::Null,
                runtime,
                source: Some(source()),
                submission_key: IdempotencyKey::new("required-delivery-gate").assert_value(),
            },
            DeliveryPolicy::Required,
        )
        .await
        .assert_value()
}

async fn admitted_without_delivery() -> AdmittedRun {
    NativeV2Admission
        .admit(RunSubmission {
            environment: None,
            title: RunTitle::new("Local optional run").assert_value(),
            graph: full_graph(vec![success_node()]),
            initial_input: Value::Null,
            runtime: crate::native_v2_candidate::test_support::codex_runtime(
                run_size("medium"),
                BTreeMap::new(),
            ),
            source: Some(source()),
            submission_key: IdempotencyKey::new("optional-delivery-gate").assert_value(),
        })
        .await
        .assert_value()
}

fn delivery_receipt() -> Value {
    json!({
        "version": "v2",
        "mode": "merge",
        "outcome": "merged",
        "repository": "acme/project",
        "targetBranch": "main",
        "headRevision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "mergeRevision": "cccccccccccccccccccccccccccccccccccccccc",
        "pullRequestId": "17"
    })
}

fn worker_outcome() -> WorkerOutcome {
    WorkerOutcome::Verified {
        output: Value::Null,
        artifacts: Vec::new(),
    }
}

fn verifier_outcome() -> WorkerOutcome {
    WorkerOutcome::Verifier {
        output: Value::Null,
        signals: BTreeMap::from([(
            FieldName::new("verdict").assert_value(),
            EnumLabel::new("accepted").assert_value(),
        )]),
        diagnostic: Value::Null,
        artifacts: Vec::new(),
    }
}

fn delivery_outcome(receipt: Value) -> WorkerOutcome {
    let outcome = receipt
        .get("outcome")
        .or_else(|| receipt.pointer("/receipt/outcome"))
        .and_then(Value::as_str)
        .unwrap_or(DELIVERY_MERGED_LABEL)
        .to_owned();
    WorkerOutcome::Verifier {
        output: receipt,
        signals: BTreeMap::from([(
            FieldName::new(DELIVERY_SIGNAL_FIELD).assert_value(),
            EnumLabel::new(outcome).assert_value(),
        )]),
        diagnostic: json!({"message":"authoritatively observed merge"}),
        artifacts: Vec::new(),
    }
}

fn snapshot(
    admitted: &AdmittedRun,
    completions: impl IntoIterator<Item = (&'static str, WorkerOutcome)>,
) -> RunSnapshot {
    let run_id = RunId::new("delivery-gate-run");
    let mut snapshot = RunSnapshot::admitted(run_id.clone(), admitted).assert_value();
    snapshot.phase = RunPhase::Running;
    for (index, (node, outcome)) in completions.into_iter().enumerate() {
        let identity = u64::try_from(index + 1).assert_value();
        let reference = ExecutionRef {
            run_id: run_id.clone(),
            node: NodeName::new(node).assert_value(),
            node_instance: NodeInstanceId::new(identity).assert_value(),
            execution: ExecutionId::new(identity).assert_value(),
        };
        snapshot.executions.insert(
            reference.execution,
            NodeSnapshot {
                failure_diagnostic: false,
                reference: reference.clone(),
                occurrence: StructuralOccurrence {
                    node: reference.node.clone(),
                    map_indices: Vec::new(),
                },
                attempt: PositiveInteger::new(1).assert_value(),
                input: Value::Null,
                started_at: cursor_for(identity * 2 - 1),
                state: NodeState::Completed {
                    at: cursor_for(identity * 2),
                    outcome,
                },
            },
        );
    }
    snapshot
}

fn delivery_unconfirmed() -> TerminalResult {
    TerminalResult::Failed {
        context: None,
        reason: EnumLabel::new("delivery_unconfirmed").assert_value(),
    }
}

#[tokio::test]
async fn required_success_needs_native_confirmation_from_last_completed_writer() {
    let admitted = admitted_with_delivery().await;
    let receipt = delivery_receipt();
    let accepted = TerminalResult::Succeeded {
        output: json!({"delivery": receipt.clone()}),
    };
    let reviewed_delivery = snapshot(
        &admitted,
        [
            ("worker", worker_outcome()),
            ("verify", verifier_outcome()),
            ("deliver", delivery_outcome(receipt.clone())),
        ],
    );
    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &reviewed_delivery,
            accepted.clone(),
        )
        .assert_value(),
        accepted
    );

    let reviewer_after_delivery = snapshot(
        &admitted,
        [
            ("worker", worker_outcome()),
            ("deliver", delivery_outcome(receipt.clone())),
            ("verify", verifier_outcome()),
        ],
    );
    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &reviewer_after_delivery,
            accepted.clone(),
        )
        .assert_value(),
        delivery_unconfirmed()
    );

    let missing_inline_receipt = TerminalResult::Succeeded {
        output: Value::Null,
    };
    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &reviewed_delivery,
            missing_inline_receipt.clone(),
        )
        .assert_value(),
        missing_inline_receipt
    );

    let stale_after_later_write = snapshot(
        &admitted,
        [
            ("deliver", delivery_outcome(receipt.clone())),
            ("worker", worker_outcome()),
        ],
    );
    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &stale_after_later_write,
            TerminalResult::Succeeded {
                output: receipt.clone(),
            },
        )
        .assert_value(),
        delivery_unconfirmed()
    );

    let no_durable_delivery = snapshot(&admitted, [("worker", worker_outcome())]);
    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &no_durable_delivery,
            TerminalResult::Succeeded { output: receipt },
        )
        .assert_value(),
        delivery_unconfirmed()
    );
}

#[tokio::test]
async fn optional_policy_still_requires_a_receipt_when_delivery_is_present() {
    let admitted = admitted_with_delivery().await;
    let receipt = delivery_receipt();
    let snapshot = snapshot(
        &admitted,
        [
            ("worker", worker_outcome()),
            ("verify", verifier_outcome()),
            ("deliver", delivery_outcome(receipt.clone())),
        ],
    );
    let accepted = TerminalResult::Succeeded {
        output: json!({"delivery": receipt}),
    };

    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Optional,
            &admitted,
            &snapshot,
            accepted.clone(),
        )
        .assert_value(),
        accepted
    );
    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Optional,
            &admitted,
            &snapshot,
            TerminalResult::Succeeded {
                output: Value::Null,
            },
        )
        .assert_value(),
        TerminalResult::Succeeded {
            output: Value::Null
        }
    );
}

#[tokio::test]
async fn optional_local_success_still_allows_no_delivery() {
    let admitted = admitted_without_delivery().await;
    let terminal = TerminalResult::Succeeded {
        output: Value::Null,
    };
    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Optional,
            &admitted,
            &RunSnapshot::admitted(RunId::new("local-run"), &admitted).assert_value(),
            terminal.clone(),
        )
        .assert_value(),
        terminal
    );
}

#[tokio::test]
async fn delivery_receipt_requires_all_other_writers_to_settle_before_it_starts() {
    let admitted = admitted_with_delivery().await;
    let receipt = delivery_receipt();
    let accepted = TerminalResult::Succeeded {
        output: receipt.clone(),
    };
    let baseline = snapshot(
        &admitted,
        [
            ("worker", worker_outcome()),
            ("deliver", delivery_outcome(receipt)),
        ],
    );
    for state in [
        NodeState::Completed {
            at: cursor_for(4),
            outcome: worker_outcome(),
        },
        NodeState::Voided {
            at: cursor_for(4),
            reason: crate::full_v1_reducer::ExecutionVoidReason::ParallelJoin,
        },
        NodeState::Active,
    ] {
        let mut overlapping = baseline.clone();
        // Delivery starts at cursor 3 and completes at 5; the writer is still active at 3.
        overlapping
            .executions
            .get_mut(&ExecutionId::new(1).assert_value())
            .assert_value()
            .state = state;
        overlapping
            .executions
            .get_mut(&ExecutionId::new(2).assert_value())
            .assert_value()
            .state = NodeState::Completed {
            at: cursor_for(5),
            outcome: delivery_outcome(delivery_receipt()),
        };
        assert_eq!(
            enforce_delivery_terminal(
                DeliveryPolicy::Required,
                &admitted,
                &overlapping,
                accepted.clone()
            )
            .assert_value(),
            delivery_unconfirmed()
        );
    }
    let mut previously_voided = baseline;
    previously_voided
        .executions
        .get_mut(&ExecutionId::new(1).assert_value())
        .assert_value()
        .state = NodeState::Voided {
        at: cursor_for(2),
        reason: crate::full_v1_reducer::ExecutionVoidReason::ParallelJoin,
    };
    assert_eq!(
        enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &previously_voided,
            accepted.clone()
        )
        .assert_value(),
        accepted
    );
}

#[tokio::test]
async fn cancelled_writer_cleanup_failure_prevents_delivery_after_any_join() {
    use super::RunLedger;
    for remote in [false, true] {
        for failed_cleanup in [false, true] {
            let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let driver = super::FakeDriver::scripted([
                ("fast", vec![super::Behavior::Together(barrier.clone())]),
                (
                    "worker",
                    vec![super::Behavior::CancelAfterStart(
                        barrier,
                        if failed_cleanup {
                            super::NodeRunnerError::CleanupUnconfirmed
                        } else {
                            super::NodeRunnerError::Cancelled
                        },
                    )],
                ),
                (
                    "deliver",
                    vec![super::Behavior::Complete {
                        delay: std::time::Duration::ZERO,
                        outcome: delivery_outcome(delivery_receipt()),
                    }],
                ),
            ]);
            let (supervisor, driver, ledger) =
                cancellation_delivery_supervisor(driver, remote).await;
            let result = supervisor.drive().await;
            if failed_cleanup {
                assert!(matches!(
                    result,
                    Err(super::NativeV2SupervisorError::CleanupUnconfirmed)
                ));
                assert_eq!(driver.starts("deliver"), 0);
            } else {
                result.assert_value();
                assert_eq!(driver.starts("deliver"), 1);
            }
            assert_eq!(driver.cancellations("worker"), 1);
            assert_eq!(driver.state().active, 0);
            let snapshot = ledger
                .get(&RunId::new("cancelled-writer-delivery"))
                .await
                .assert_value()
                .assert_value()
                .snapshot;
            let writer = snapshot
                .executions
                .values()
                .find(|node| node.reference.node.as_str() == "worker")
                .assert_value();
            assert_eq!(
                matches!(writer.state, NodeState::Voided { .. }),
                !failed_cleanup
            );
        }
    }
}

async fn cancellation_delivery_supervisor(
    driver: super::FakeDriver,
    remote: bool,
) -> (
    super::NativeV2Supervisor,
    std::sync::Arc<super::FakeDriver>,
    std::sync::Arc<super::FakeRunLedger>,
) {
    use super::{RunLedger, Sha256Digest};
    let admitted = admitted_with_delivery().await;
    let graph = full_graph(vec![
        json!({
            "kind":"par", "name":"writers", "state":{"kind":"null"},
            "branches":[super::verifier("fast", 1000), super::step("worker", 1000)],
            "promotedStatePaths":[], "join":{"kind":"any"}
        }),
        git_delivery_node(),
        super::verifier("verify", 1000),
        success_node(),
    ]);
    let mut runtime = admitted.runtime;
    assert_eq!(runtime.harness.as_str(), "codex");
    let nodes = runtime.nodes_mut();
    nodes.insert(NodeName::new("fast").assert_value(), agent_binding());
    let submission_key = IdempotencyKey::new("cancelled-writer-delivery").assert_value();
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            environment: None,
            title: RunTitle::new("Cancelled writer delivery").assert_value(),
            graph,
            initial_input: Value::Null,
            runtime,
            source: Some(source()),
            submission_key: submission_key.clone(),
        })
        .await
        .assert_value();
    let run_id = RunId::new("cancelled-writer-delivery");
    let ledger = std::sync::Arc::new(super::FakeRunLedger::new());
    ledger
        .create_or_get(super::CreateRun {
            run_id: run_id.clone(),
            submission_key,
            submission_digest: Sha256Digest::new("0".repeat(64)).assert_value(),
            admitted: admitted.clone(),
        })
        .await
        .assert_value();
    let driver = std::sync::Arc::new(driver);
    let local = std::sync::Arc::new(
        super::NativeNodeRunner::new(
            &admitted,
            driver.clone(),
            std::sync::Arc::new(super::FakeSessionFactory::default()),
        )
        .assert_value(),
    );
    let runner: std::sync::Arc<dyn super::NodeRunner> = if remote {
        std::sync::Arc::new(crate::native_v2_capsule::RemoteCapsuleNodeRunner::new(
            std::sync::Arc::new(crate::native_v2_capsule::NativeCapsuleNodeEndpoint::new(
                local,
            )),
        ))
    } else {
        local
    };
    let environment = super::RunEnvironment::exact(
        &admitted.runtime,
        admitted.environment.as_ref(),
        BTreeMap::new(),
    )
    .assert_value();
    (
        super::NativeV2Supervisor::new(
            run_id,
            ledger.clone(),
            runner,
            std::sync::Arc::new(environment),
        ),
        driver,
        ledger,
    )
}

#[tokio::test]
async fn restored_delivery_receipt_remains_valid_until_a_new_writer_runs() {
    let admitted = admitted_with_delivery().await;
    let receipt = delivery_receipt();
    let mut restored = snapshot(
        &admitted,
        [
            ("worker", worker_outcome()),
            ("deliver", delivery_outcome(receipt.clone())),
        ],
    );
    restored.execution_seed = super::super::durable_history(&restored).assert_value();
    restored.executions.clear();
    assert!(super::super::confirmed_delivery_execution(&admitted, &restored).is_some());
    let new_writer = snapshot(&admitted, [("worker", worker_outcome())]);
    restored.executions = new_writer.executions;
    assert!(super::super::confirmed_delivery_execution(&admitted, &restored).is_none());
}

#[tokio::test]
async fn checked_delivery_evidence_does_not_depend_on_graph_output() {
    let admitted = admitted_with_delivery().await;
    let native = delivery_receipt();
    let mut recorded = snapshot(&admitted, [("deliver", delivery_outcome(native.clone()))]);
    for output in [
        Value::Null,
        json!("report"),
        json!({"summary":"done"}),
        json!({"delivery":{"mode":"merge","outcome":"merged","mergeRevision":"forged"}}),
    ] {
        let terminal = TerminalResult::Succeeded { output };
        let checked = super::super::enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &recorded,
            terminal.clone(),
        )
        .assert_value();
        assert_eq!(checked.result, terminal);
        recorded.primary_graph = Some(checked);
        assert_eq!(recorded.delivery_receipt(), Some(native.clone()));
    }
    let failed = TerminalResult::Failed {
        reason: EnumLabel::new("review_rejected").assert_value(),
        context: None,
    };
    let checked = super::super::enforce_delivery_terminal(
        DeliveryPolicy::Required,
        &admitted,
        &recorded,
        failed.clone(),
    )
    .assert_value();
    assert_eq!(checked.result, failed);
    assert!(checked.delivery_execution.is_some());
}

#[tokio::test]
async fn forged_or_unsuccessful_native_results_never_supply_delivery_evidence() {
    let admitted = admitted_with_delivery().await;
    for (field, value) in [
        ("repository", "wrong/project"),
        ("targetBranch", "wrong"),
        ("outcome", "conflict"),
        ("headRevision", "invalid"),
        ("mergeRevision", ""),
    ] {
        let mut native = delivery_receipt();
        native[field] = json!(value);
        let recorded = snapshot(&admitted, [("deliver", delivery_outcome(native))]);
        let checked = super::super::enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &recorded,
            TerminalResult::Succeeded {
                output: delivery_receipt(),
            },
        )
        .assert_value();
        assert_eq!(checked.result, delivery_unconfirmed());
        assert!(checked.delivery_execution.is_none());
    }
}

#[tokio::test]
async fn checked_receipt_reference_resolves_imported_checkpoint_evidence() {
    let admitted = admitted_with_delivery().await;
    let receipt = delivery_receipt();
    let mut recorded = snapshot(&admitted, [("deliver", delivery_outcome(receipt.clone()))]);
    recorded.execution_seed = super::super::durable_history(&recorded).assert_value();
    recorded.executions.clear();
    let checked = super::super::enforce_delivery_terminal(
        DeliveryPolicy::Required,
        &admitted,
        &recorded,
        TerminalResult::Succeeded {
            output: Value::Null,
        },
    )
    .assert_value();
    assert!(matches!(checked.result, TerminalResult::Succeeded { .. }));
    recorded.primary_graph = Some(checked);
    assert_eq!(recorded.delivery_receipt(), Some(receipt));
}

#[tokio::test]
async fn every_native_git_receipt_kind_is_independent_of_terminal_output() {
    use crate::native_v2_delivery::DeliveryMode;
    for (mode, version, label, outcome) in [
        (DeliveryMode::Push, "v1", "push", "pushed"),
        (DeliveryMode::PullRequest, "v1", "pr", "opened"),
        (DeliveryMode::PullRequestV2, "v2", "pr", "ready"),
        (DeliveryMode::MergeV1, "v1", "merge", "merged"),
        (DeliveryMode::Merge, "v2", "merge", "merged"),
        (DeliveryMode::MergeV3, "v3", "merge", "merged"),
    ] {
        let admitted = admitted_with_git_mode(mode).await;
        let mut native = delivery_receipt();
        native["version"] = json!(version);
        native["mode"] = json!(label);
        native["outcome"] = json!(outcome);
        if !matches!(mode, DeliveryMode::Merge | DeliveryMode::MergeV3) {
            native
                .as_object_mut()
                .assert_value()
                .remove("mergeRevision");
        }
        if mode == DeliveryMode::Push {
            native
                .as_object_mut()
                .assert_value()
                .remove("pullRequestId");
            native["headBranch"] = json!("zeroshot/delivery-proof");
        } else {
            native["links"] = json!({});
        }
        for head in [
            native["headRevision"].clone(),
            json!(admitted.source.as_ref().assert_value().revision.as_str()),
        ] {
            native["headRevision"] = head;
            let mut recorded = snapshot(&admitted, [("deliver", delivery_outcome(native.clone()))]);
            let result = TerminalResult::Succeeded {
                output: Value::Null,
            };
            let checked = super::super::enforce_delivery_terminal(
                DeliveryPolicy::Required,
                &admitted,
                &recorded,
                result.clone(),
            )
            .assert_value();
            assert_eq!(checked.result, result, "{mode:?}");
            recorded.primary_graph = Some(checked);
            assert_eq!(
                recorded.delivery_receipt(),
                Some(native.clone()),
                "{mode:?}"
            );
        }
    }
}

#[tokio::test]
async fn all_supported_builtin_graphs_keep_success_independent_of_receipt_projection() {
    use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
    use openengine_cluster_server::graph_verifier::graph_node_children;
    let mut checked_pairs = 0;
    for &template in BuiltinGraphTemplate::all() {
        for delivery in [
            TemplateDelivery::None,
            TemplateDelivery::Push,
            TemplateDelivery::PullRequest,
            TemplateDelivery::Merge,
            TemplateDelivery::Comment,
        ] {
            let Ok(graph) = template.materialize(delivery) else {
                continue;
            };
            let native = template.delivery_runtime_binding(delivery).assert_value();
            let mut pending = vec![&graph.root];
            let mut nodes = BTreeMap::new();
            while let Some(node) = pending.pop() {
                if matches!(
                    node,
                    openengine_cluster_protocol::GraphNode::Step(_)
                        | openengine_cluster_protocol::GraphNode::Verifier(_)
                ) {
                    nodes.insert(node.name().clone(), agent_binding());
                }
                pending.extend(graph_node_children(node));
            }
            if let Some((name, binding)) = &native {
                nodes.insert(name.clone(), binding.clone());
            }
            let runtime =
                crate::native_v2_candidate::test_support::codex_runtime(run_size("medium"), nodes);
            let is_pr = matches!(
                template,
                BuiltinGraphTemplate::PrReview | BuiltinGraphTemplate::CodeReview
            );
            let url = if is_pr {
                "https://github.com/acme/project/pull/42"
            } else {
                "https://github.com/acme/project/issues/42"
            };
            let initial_input = if matches!(
                template,
                BuiltinGraphTemplate::IssueReview
                    | BuiltinGraphTemplate::PrReview
                    | BuiltinGraphTemplate::CodeReview
            ) {
                let scope = if is_pr {
                    json!({"baseRepository":"acme/project","baseBranch":"main","baseRevision":"b".repeat(40),"headRepository":"acme/project","headBranch":"fix","headRevision":"a".repeat(40)})
                } else {
                    json!({})
                };
                let mut context =
                    json!({"title":"Review","body":"Review this change","reviewScope":scope});
                if is_pr {
                    context["linkedIssues"] = json!([]);
                }
                json!({"url":url,"context":context})
            } else {
                json!({"task":"Implement this change"})
            };
            let admitted = NativeV2Admission
                .admit(RunSubmission {
                    environment: None,
                    title: RunTitle::new("builtin independent delivery").assert_value(),
                    graph,
                    initial_input,
                    runtime,
                    source: Some(source()),
                    submission_key: IdempotencyKey::new("builtin-independent").assert_value(),
                })
                .await
                .assert_value();
            let expected = TerminalResult::Succeeded {
                output: json!({"summary":"authored output"}),
            };
            let mut recorded = snapshot(&admitted, []);
            let mut expected_receipt = None;
            if let Some((name, _)) = native {
                let mut receipt = if delivery == TemplateDelivery::Comment {
                    json!({"receipt":{"version":"v1","mode":"comment","outcome":"posted","destinationUrl":url,"repository":"acme/project","discussionKind":if is_pr {"pull_request"} else {"issue"},"discussionId":"42","commentId":"23","url":format!("{url}#issuecomment-23"),"bodyHash":"a".repeat(64),"sourceRevision":"a".repeat(40)}})
                } else {
                    delivery_receipt()
                };
                match delivery {
                    TemplateDelivery::Push => {
                        receipt["version"] = json!("v1");
                        receipt["mode"] = json!("push");
                        receipt["outcome"] = json!("pushed");
                        receipt["headBranch"] = json!("zeroshot/proof");
                        receipt
                            .as_object_mut()
                            .assert_value()
                            .remove("pullRequestId");
                        receipt
                            .as_object_mut()
                            .assert_value()
                            .remove("mergeRevision");
                    }
                    TemplateDelivery::PullRequest => {
                        receipt["outcome"] = json!("ready");
                        receipt["mode"] = json!("pr");
                        receipt
                            .as_object_mut()
                            .assert_value()
                            .remove("mergeRevision");
                        receipt["links"] = json!({});
                    }
                    TemplateDelivery::Merge => {
                        receipt["version"] = json!("v3");
                        receipt["links"] = json!({});
                    }
                    _ => {}
                }
                recorded = snapshot(
                    &admitted,
                    [(
                        if name.as_str() == "checkpoint_delivery" {
                            "checkpoint_delivery"
                        } else {
                            "deliver"
                        },
                        delivery_outcome(receipt.clone()),
                    )],
                );
                expected_receipt = Some(receipt);
            }
            let checked = super::super::enforce_delivery_terminal(
                DeliveryPolicy::Optional,
                &admitted,
                &recorded,
                expected.clone(),
            )
            .assert_value();
            assert_eq!(checked.result, expected, "{template:?}/{delivery:?}");
            recorded.primary_graph = Some(checked);
            assert_eq!(
                recorded.delivery_receipt(),
                expected_receipt,
                "{template:?}/{delivery:?}"
            );
            checked_pairs += 1;
        }
    }
    assert_eq!(checked_pairs, 11);
}

#[tokio::test]
async fn repeated_delivery_uses_the_final_visit_and_never_falls_back_to_an_older_receipt() {
    let admitted = admitted_with_delivery().await;
    let first = delivery_receipt();
    let mut last = first.clone();
    last["headRevision"] = json!("d".repeat(40));
    last["mergeRevision"] = json!("e".repeat(40));
    for successful in [true, false] {
        let mut final_receipt = last.clone();
        if !successful {
            final_receipt["outcome"] = json!("ci_failed");
        }
        let mut recorded = snapshot(
            &admitted,
            [
                ("worker", worker_outcome()),
                ("deliver", delivery_outcome(first.clone())),
                ("worker", worker_outcome()),
                ("deliver", delivery_outcome(final_receipt)),
            ],
        );
        let checked = super::super::enforce_delivery_terminal(
            DeliveryPolicy::Required,
            &admitted,
            &recorded,
            TerminalResult::Succeeded {
                output: first.clone(),
            },
        )
        .assert_value();
        assert_eq!(
            matches!(checked.result, TerminalResult::Succeeded { .. }),
            successful
        );
        recorded.primary_graph = Some(checked);
        assert_eq!(
            recorded.delivery_receipt(),
            successful.then(|| last.clone())
        );
    }
}
