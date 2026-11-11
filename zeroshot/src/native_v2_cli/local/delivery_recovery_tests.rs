use super::*;
use crate::native_v2_candidate::test_support::{
    TestDirectory, full_graph, git_delivery_node, success_node,
};
use super::super::local_contract_tests::{contract_submission, create_contract_run};
use crate::native_v2_delivery::DeliveryMode;
use crate::v2_run_ledger::{RunEvent, NodeState, cursor_for};
use crate::native_v2_contract::{ExecutionRef, NodeCompletion};
use crate::full_v1_reducer::{ExecutionId, NodeInstanceId, StructuralOccurrence};
use openengine_cluster_protocol::{NodeName, PositiveInteger, WorkerOutcome, FieldName, EnumLabel};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

fn submission(key: &str) -> RunSubmission {
    let mut submission = contract_submission(key);
    let mode = DeliveryMode::PullRequest;
    let mut node = git_delivery_node();
    node["worker"] = json!(mode.worker_ref());
    node["output"] = serde_json::to_value(
        crate::native_v2_delivery::delivery_result_schema(mode).assert_value(),
    )
    .assert_value();
    node["signals"] =
        json!({"delivery":crate::native_v2_delivery::delivery_signal_labels(mode).assert_value()});
    submission.graph = full_graph(vec![node, success_node()]);
    submission.runtime.nodes_mut().insert(
        NodeName::new("deliver").assert_value(),
        crate::native_v2_contract::NodeRuntimeBinding::GitDelivery {
            connections: Default::default(),
            pull_request_feedback: Default::default(),
        },
    );
    submission
}

fn receipt() -> Value {
    json!({"version":"v1","mode":"pr","outcome":"opened",
        "repository":"open-engine/zeroshot","targetBranch":"main",
        "headRevision":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","pullRequestId":"17"})
}

async fn record(ledger: &SqliteRunLedger, run: &RunId, output: Value) {
    record_outcome(
        ledger,
        run,
        WorkerOutcome::Verifier {
            output,
            signals: std::collections::BTreeMap::from([(
                FieldName::new("delivery").assert_value(),
                EnumLabel::new("opened").assert_value(),
            )]),
            diagnostic: json!({"message":"confirmed publication"}),
            artifacts: Vec::new(),
        },
    )
    .await;
}

async fn record_outcome(ledger: &SqliteRunLedger, run: &RunId, outcome: WorkerOutcome) {
    let reference = ExecutionRef {
        run_id: run.clone(),
        node: NodeName::new("deliver").assert_value(),
        node_instance: NodeInstanceId::new(1).assert_value(),
        execution: ExecutionId::new(1).assert_value(),
    };
    ledger
        .append(
            run,
            vec![
                RunEvent::RunStarted,
                RunEvent::NodeStarted {
                    reference: reference.clone(),
                    occurrence: StructuralOccurrence {
                        node: reference.node.clone(),
                        map_indices: Vec::new(),
                    },
                    attempt: PositiveInteger::new(1).assert_value(),
                    input: Value::Null,
                },
                RunEvent::NodeCompleted {
                    completion: NodeCompletion { reference, outcome },
                },
            ],
        )
        .await
        .assert_value();
}

#[tokio::test]
async fn confirmed_review_survives_failed_delivery_and_a_crash_without_a_settled_receipt() {
    for settled in [false, true] {
        let root = TestDirectory::new("early-review");
        let backend = LocalCliBackend::new(
            root.child("state"),
            PathBuf::from("zeroshot"),
            root.path().to_owned(),
            PathBuf::from("git"),
        );
        let run = RunId::new("0199f33f-3b44-7d21-9000-0000000000d5");
        let submission = submission("incomplete-delivery");
        let ledger = create_contract_run(&backend, &run, &submission).await;
        if settled {
            record_outcome(
                &ledger,
                &run,
                WorkerOutcome::declared_failure(
                    openengine_cluster_protocol::WorkerErrorCode::Timeout,
                ),
            )
            .await;
        }
        let workspace = root.child("workspace");
        let branch = "feature/reused";
        backend
            .write_recovery_document(
                &run,
                &LocalRecoveryDocument {
                    delivery_receipt: None,
                    submission: submission.clone(),
                    workspace: workspace.clone(),
                    local_branch: Some(branch.to_owned()),
                    invoking_workspace: None,
                    request_digest: None,
                    workspace_revision: None,
                    resumed_from: None,
                    delivery_run_id: Some(run.clone()),
                    successor_run_id: None,
                    pending_restore: None,
                },
            )
            .assert_value();
        let target = recovery_target(
            &backend.read_recovery_document(&run).assert_value(),
            DeliveryMode::PullRequest,
        )
        .assert_value();
        let review =
            contract::review_from_receipt(&receipt(), DeliveryMode::PullRequest, &target, branch)
                .assert_value();
        let recorder = review_binding_recorder(
            backend.paths(&run).assert_value().storage(),
            &workspace,
            Some(branch),
            &run,
        )
        .assert_value()
        .assert_value();
        recorder(&review, false).assert_value();
        let prepared = PreparedLocalRun {
            run_id: RunId::new("0199f33f-3b44-7d21-9000-0000000000d6"),
            delivery_run_id: run.clone(),
            environment: RunEnvironment::exact(&submission.runtime, None, Default::default())
                .assert_value(),
            submission,
            workspace,
            local_branch: Some(branch.to_owned()),
            invoking_workspace: None,
            request_digest: None,
            workspace_revision: None,
            github_token: None,
            github_token_file: None,
            delivery_validator: None,
            git_credentials: None,
            native_environment: Default::default(),
        };
        let recovered = backend
            .recover_delivery_receipt(&run, &prepared)
            .await
            .assert_value()
            .assert_value();
        let bound =
            contract::review_from_recovery(&recovered, DeliveryMode::PullRequest, &target, branch)
                .assert_value();
        assert_eq!(bound.review, review);
        assert!(!bound.published);
        assert!(!crate::native_v2_delivery::is_matching_success_receipt(
            &recovered["receipt"],
            DeliveryMode::PullRequest,
            &target
        ));
        let mut other = review.clone();
        other.review_id = "18".to_owned();
        assert!(recorder(&other, false).is_err());
        assert_eq!(
            backend
                .read_recovery_document(&run)
                .assert_value()
                .delivery_receipt,
            Some(recovered)
        );
        // Observations cannot promote first publication, downgrade it, or replace its
        // verified head with an unverified remote rewrite.
        recorder(&review, true).assert_value();
        let published = backend
            .read_recovery_document(&run)
            .assert_value()
            .delivery_receipt;
        let mut observed = review;
        observed.head_revision = "c".repeat(40);
        recorder(&observed, false).assert_value();
        let saved = backend
            .recover_delivery_receipt(&run, &prepared)
            .await
            .assert_value()
            .assert_value();
        assert_eq!(Some(saved.clone()), published);
        assert!(
            contract::review_from_recovery(&saved, DeliveryMode::PullRequest, &target, branch)
                .assert_value()
                .published
        );
    }
}

#[tokio::test]
async fn local_resume_recovers_the_owned_review_across_successors_without_delivery_evidence() {
    let root = TestDirectory::new("review-lineage");
    let backend = LocalCliBackend::new(
        root.child("state"),
        PathBuf::from("zeroshot"),
        root.path().to_owned(),
        PathBuf::from("git"),
    );
    let first = RunId::new("0199f33f-3b44-7d21-9000-0000000000d1");
    let second = RunId::new("0199f33f-3b44-7d21-9000-0000000000d2");
    let workspace = root.child("workspace");
    let branch = "feature/reused";
    for (run, predecessor) in [(&first, None), (&second, Some(first.clone()))] {
        let submission = submission(run.as_str());
        let ledger = create_contract_run(&backend, run, &submission).await;
        if run == &first {
            record(&ledger, run, receipt()).await;
        }
        backend
            .write_recovery_document(
                run,
                &LocalRecoveryDocument {
                    delivery_receipt: None,
                    submission,
                    workspace: workspace.clone(),
                    local_branch: Some(branch.to_owned()),
                    invoking_workspace: None,
                    request_digest: None,
                    workspace_revision: None,
                    resumed_from: predecessor,
                    delivery_run_id: Some(first.clone()),
                    successor_run_id: None,
                    pending_restore: None,
                },
            )
            .assert_value();
    }
    let submission = submission("resume-review");
    let mut prepared = PreparedLocalRun {
        run_id: RunId::new("0199f33f-3b44-7d21-9000-0000000000d3"),
        delivery_run_id: first,
        environment: RunEnvironment::exact(
            &submission.runtime,
            submission.environment.as_ref(),
            Default::default(),
        )
        .assert_value(),
        submission,
        workspace,
        local_branch: Some(branch.to_owned()),
        invoking_workspace: None,
        request_digest: None,
        workspace_revision: None,
        github_token: None,
        github_token_file: None,
        delivery_validator: None,
        git_credentials: None,
        native_environment: Default::default(),
    };
    assert_eq!(
        backend
            .recover_delivery_receipt(&second, &prepared)
            .await
            .assert_value(),
        Some(receipt())
    );
    prepared.workspace = root.child("another-workspace");
    assert!(
        backend
            .recover_delivery_receipt(&second, &prepared)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn recovery_uses_delivery_evidence_in_checkpoint_seeds_and_ignores_other_nodes() {
    let root = TestDirectory::new("review-seed");
    let backend = LocalCliBackend::new(
        root.child("state"),
        PathBuf::from("zeroshot"),
        root.path().to_owned(),
        PathBuf::from("git"),
    );
    let run = RunId::new("0199f33f-3b44-7d21-9000-0000000000d4");
    let ledger = create_contract_run(&backend, &run, &submission("seed-review")).await;
    record(&ledger, &run, receipt()).await;
    let mut stored = ledger.get(&run).await.assert_value().assert_value();
    let mut unrelated = stored
        .snapshot
        .executions
        .values()
        .next()
        .assert_value()
        .clone();
    unrelated.reference.node = NodeName::new("untrusted").assert_value();
    unrelated.reference.execution = ExecutionId::new(2).assert_value();
    unrelated.occurrence.node = unrelated.reference.node.clone();
    unrelated.started_at = cursor_for(10);
    let NodeState::Completed {
        at,
        outcome: WorkerOutcome::Verifier { output, .. },
    } = &mut unrelated.state
    else {
        panic!("completed verifier")
    };
    *at = cursor_for(11);
    output["pullRequestId"] = json!("999");
    stored
        .snapshot
        .executions
        .insert(unrelated.reference.execution, unrelated);
    assert_eq!(
        recorded_review(&stored, "feature/reused").assert_value(),
        Some(receipt())
    );
    stored.snapshot.execution_seed =
        crate::native_v2_supervisor::durable_history(&stored.snapshot).assert_value();
    stored.snapshot.executions.clear();
    assert_eq!(
        recorded_review(&stored, "feature/reused").assert_value(),
        Some(receipt())
    );
    let DurableExecutionState::Settled {
        outcome: WorkerOutcome::Verifier { output, .. },
        ..
    } = &mut stored.snapshot.execution_seed[0].state
    else {
        panic!("settled delivery")
    };
    output["repository"] = json!("another/repository");
    assert_eq!(
        recorded_review(&stored, "feature/reused").assert_value(),
        None
    );
}
