use super::*;
use crate::native_v2_capsule::{NativeCapsuleNodeEndpoint, RemoteCapsuleNodeRunner};
use crate::native_v2_observability::NativeV2Observability;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use openengine_cluster_protocol::{RunStatus, RunStatusParams};

const QUOTA: &str = "Claude usage quota exhausted (status=rejected, type=five_hour, resetsAt=2030-01-01T00:00:00Z). Resume with `zeroshot resume RUN_ID` after provider usage becomes available.";

#[tokio::test]
async fn builtin_review_failure_preserves_quota_node_reset_and_resume_guidance_in_status() {
    for remote in [false, true] {
        let driver = FakeDriver::scripted([
            (
                "worker",
                vec![Behavior::Complete {
                    delay: Duration::ZERO,
                    outcome: WorkerOutcome::Verified {
                        output: json!({"reviewResponses":[]}),
                        artifacts: Vec::new(),
                    },
                }],
            ),
            (
                "acceptance",
                vec![Behavior::Complete {
                    delay: Duration::ZERO,
                    outcome: WorkerOutcome::Verifier {
                        output: Value::Null,
                        signals: BTreeMap::from([(
                            "verdict".parse().assert_value(),
                            "accepted".parse().assert_value(),
                        )]),
                        diagnostic: json!({"message":"accepted"}),
                        artifacts: Vec::new(),
                    },
                }],
            ),
            (
                "code",
                vec![Behavior::Fail(NodeRunnerError::QuotaExhausted(
                    QUOTA.into(),
                ))],
            ),
        ]);
        let mut harness = harness(
            BuiltinGraphTemplate::SoftwareChange
                .materialize(TemplateDelivery::None)
                .assert_value(),
            json!({"task":"test quota exhaustion"}),
            driver.clone(),
        )
        .await;
        if remote {
            let endpoint = NativeCapsuleNodeEndpoint::new(harness.supervisor.runner.clone());
            harness.supervisor.runner = Arc::new(RemoteCapsuleNodeRunner::new(Arc::new(endpoint)));
        }
        let terminal = harness.supervisor.drive().await.assert_value();
        let TerminalResult::Failed {
            reason,
            context: Some(ref context),
            ..
        } = terminal
        else {
            panic!("expected terminal failure context")
        };
        assert_eq!(reason.as_str(), "review_failed");
        assert!(context.recorded.iter().any(|failure| {
            failure
                .execution
                .as_ref()
                .is_some_and(|execution| execution.node.as_str() == "code")
                && failure.message.as_str().contains(QUOTA)
        }));
        assert_eq!(driver.starts("worker"), 1);
        assert_eq!(driver.starts("code"), 1);
        let stored = stored_run(&harness.ledger).await;
        let code = stored
            .snapshot
            .executions
            .values()
            .find(|node| node.reference.node.as_str() == "code")
            .assert_value();
        assert_eq!(
            code.outcome(),
            Some(&WorkerOutcome::declared_failure(WorkerErrorCode::Refusal))
        );
        let status = NativeV2Observability::new(harness.ledger.clone())
            .status(RunStatusParams {
                run_id: stored.snapshot.run_id.clone(),
            })
            .await
            .assert_value();
        assert!(
            matches!(status.status, RunStatus::Finished { terminal_result, .. }
            if Some(&terminal_result) == stored.snapshot.terminal.as_ref())
        );
    }
}

#[tokio::test]
async fn authored_refusal_recovery_remains_in_control() {
    let worker = step("worker", 1_000);
    let route = json!({
        "kind":"choice", "name":"handle_refusal", "state":null_type(),
        "promotedStatePaths":[], "branches":[{
            "when":{"kind":"in","value":{"name":"worker","source":"error"},"labels":["refusal"]},
            "node":step("recovery", 1_000)
        }], "otherwise":{"kind":"fail","name":"unexpected","reason":"unexpected_failure"}
    });
    let driver = FakeDriver::scripted([(
        "worker",
        vec![Behavior::Fail(NodeRunnerError::QuotaExhausted(
            QUOTA.into(),
        ))],
    )]);
    let harness = harness(
        graph(
            sequence(vec![worker, route, succeed("done")], null_type()),
            null_type(),
        ),
        Value::Null,
        driver.clone(),
    )
    .await;
    assert!(matches!(
        harness.supervisor.drive().await.assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(driver.starts("worker"), 1);
    assert_eq!(driver.starts("recovery"), 1);
}

#[tokio::test]
async fn authored_loop_can_revisit_a_quota_refusal_without_a_global_cooldown() {
    let driver = FakeDriver::scripted([(
        "worker",
        vec![
            Behavior::Fail(NodeRunnerError::QuotaExhausted(QUOTA.into())),
            Behavior::Fail(NodeRunnerError::QuotaExhausted(QUOTA.into())),
        ],
    )]);
    let repeated = json!({
        "kind":"loop", "name":"authored_recovery", "state":null_type(),
        "maxIterations":2, "promotedStatePaths":[], "body":step("worker", 1_000)
    });
    let harness = harness(
        graph(
            sequence(vec![repeated, succeed("done")], null_type()),
            null_type(),
        ),
        Value::Null,
        driver.clone(),
    )
    .await;
    assert!(matches!(
        harness.supervisor.drive().await.assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(driver.starts("worker"), 2);
}
