use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    CodexProvider, DeclaredConnections, EnumLabel, FieldName, IdempotencyKey, NodeName,
    PositiveInteger, RunId, RunSize, RunTitle, SessionScope, SourceBranchId, SourceRepositoryId,
    SourceRevisionId, ResolvedSource, TerminalResult, WorkerOutcome,
};
use serde_json::{Value, json};

use super::super::enforce_delivery_terminal;
use crate::full_v1_reducer::StructuralOccurrence;
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_candidate::test_support::{full_graph, git_delivery_node, success_node};
use crate::native_v2_contract::{
    AdmittedRun, ExecutionId, ExecutionRef, NodeInstanceId, NodeRuntimeBinding, ReasoningEffort,
    RunSubmission, RuntimePlan,
};
use crate::native_v2_delivery::{DELIVERY_MERGED_LABEL, DELIVERY_SIGNAL_FIELD};
use crate::v2_run_ledger::{NodeSnapshot, NodeState, RunPhase, RunSnapshot, cursor_for};
use openengine_cluster_testkit::assertions::AssertValue;

fn agent_binding() -> NodeRuntimeBinding {
    NodeRuntimeBinding::Agent {
        model: crate::worker_catalog::ModelId::new("gpt-5.6-sol").assert_value(),
        effort: Some(ReasoningEffort::Max),
        session_scope: SessionScope::Execution,
        connections: DeclaredConnections::empty(),
    }
}

fn source() -> ResolvedSource {
    ResolvedSource {
        repository: SourceRepositoryId::new("acme/project").assert_value(),
        branch: SourceBranchId::new("main").assert_value(),
        revision: SourceRevisionId::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").assert_value(),
    }
}

async fn admitted_with_delivery() -> AdmittedRun {
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
        git_delivery_node(),
        success_node(),
    ]);
    let runtime = RuntimePlan::Codex {
        provider: CodexProvider::OpenAi,
        size: RunSize::Medium,
        nodes: BTreeMap::from([
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
    };
    NativeV2Admission
        .admit_with_policy(
            RunSubmission {
                environment: None,
                title: RunTitle::new("Required delivery gate").assert_value(),
                graph,
                initial_input: Value::Null,
                runtime,
                source: source(),
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
            runtime: RuntimePlan::Codex {
                provider: CodexProvider::OpenAi,
                size: RunSize::Medium,
                nodes: BTreeMap::new(),
            },
            source: source(),
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
    WorkerOutcome::Verifier {
        output: receipt,
        signals: BTreeMap::from([(
            FieldName::new(DELIVERY_SIGNAL_FIELD).assert_value(),
            EnumLabel::new(DELIVERY_MERGED_LABEL).assert_value(),
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
    let mut snapshot = RunSnapshot::admitted(run_id.clone(), admitted);
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
        reason: EnumLabel::new("delivery_unconfirmed").assert_value(),
    }
}

#[tokio::test]
async fn required_success_needs_exact_terminal_receipt_from_last_completed_writer() {
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
            missing_inline_receipt,
        )
        .assert_value(),
        delivery_unconfirmed()
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
        delivery_unconfirmed()
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
            &RunSnapshot::admitted(RunId::new("local-run"), &admitted),
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
    let RuntimePlan::Codex { nodes, .. } = &mut runtime else {
        panic!("Codex fixture");
    };
    nodes.insert(NodeName::new("fast").assert_value(), agent_binding());
    let submission_key = IdempotencyKey::new("cancelled-writer-delivery").assert_value();
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            environment: None,
            title: RunTitle::new("Cancelled writer delivery").assert_value(),
            graph,
            initial_input: Value::Null,
            runtime,
            source: source(),
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
    assert!(super::super::has_required_delivery_receipt(
        &admitted, &restored, &receipt
    ));
    let new_writer = snapshot(&admitted, [("worker", worker_outcome())]);
    restored.executions = new_writer.executions;
    assert!(!super::super::has_required_delivery_receipt(
        &admitted, &restored, &receipt
    ));
}
