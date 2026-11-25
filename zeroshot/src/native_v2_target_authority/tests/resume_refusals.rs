use openengine_cluster_testkit::fixture::run_size;
use openengine_cluster_protocol::{
    ConnectionKey, EnvironmentVariableName, NodeName, NodeRuntimeBinding, RunStatusParams,
    RuntimeLane, Sha256Digest, StaticConnectionValues, WorkerErrorCode, WorkspaceRecovery,
    INTERNAL_ERROR_CODE,
};

use super::*;
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use crate::native_v2_platform::CapsuleAllocationRequest;
use crate::v2_run_ledger::{CreateRun, RunLedger};

/// Retains every failed run's workspace, as `target serve` does after a node failure.
struct RetainedWorkspace;

#[async_trait]
impl CapsuleAllocator for RetainedWorkspace {
    async fn claim_controller(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        NoAllocation.claim_controller(run_id).await
    }

    async fn allocate(
        &self,
        request: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        NoAllocation.allocate(request).await
    }

    async fn destroy_or_confirm_absent(
        &self,
        run_id: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        NoAllocation.destroy_or_confirm_absent(run_id, exit).await
    }

    async fn workspace_recovery(&self, _run_id: &RunId) -> WorkspaceRecovery {
        WorkspaceRecovery {
            recoverable: true,
            ..Default::default()
        }
    }

    async fn reserve_retained_workspace(
        &self,
        _source_run_id: &RunId,
        _run_id: &RunId,
        _selection: &crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection,
    ) -> Result<(), crate::native_v2_platform::RetainedAllocationUnavailable> {
        Ok(())
    }
}

struct FailedRunFactory {
    ledger: Arc<FakeRunLedger>,
    checkpoint_opt_out: bool,
}

#[async_trait]
impl TargetControllerFactory for FailedRunFactory {
    fn run_sizes(&self) -> Option<openengine_cluster_protocol::TargetRunSizesDiscovery> {
        Some(fixture_run_sizes())
    }

    fn supports_workspace_recovery(&self) -> bool {
        true
    }

    fn supports_workspace_checkpoint_opt_out(&self) -> bool {
        self.checkpoint_opt_out
    }

    async fn create(&self) -> Result<Arc<NativeV2PlatformController>, TargetAuthorityError> {
        NativeV2PlatformController::new_with_delivery_policy(
            self.ledger.clone(),
            Arc::new(RetainedWorkspace),
            crate::native_v2_admission::DeliveryPolicy::Optional,
        )
        .await
        .map(|controller| {
            Arc::new(
                controller
                    .with_run_sizes(fixture_run_sizes())
                    .with_hosted_workspace_checkpoint_opt_out(self.checkpoint_opt_out),
            )
        })
        .map_err(|error| TargetAuthorityError::unavailable(error.to_string()))
    }

    async fn submit(
        &self,
        _controller: &NativeV2PlatformController,
        _request: TargetRunRequest,
    ) -> Result<TargetRunReceipt, TargetAuthorityError> {
        Err(TargetAuthorityError::invalid(
            "resume fixture never submits a run",
        ))
    }
}

fn anthropic() -> ConnectionKey {
    ConnectionKey::new("anthropic").assert_value()
}

fn oauth_token() -> EnvironmentVariableName {
    EnvironmentVariableName::new("CLAUDE_CODE_OAUTH_TOKEN").assert_value()
}

/// The reported run: single-worker without delivery, one Claude node declaring `anthropic`, failed.
async fn failed_run_declaring_a_connection() -> Arc<FakeRunLedger> {
    let mut submission = request().submission;
    submission.graph = BuiltinGraphTemplate::SingleWorker
        .materialize(TemplateDelivery::None)
        .assert_value();
    submission.initial_input = json!({"task": "say hi"});
    submission.runtime = crate::native_v2_contract::RuntimePlan::new(
        RuntimeLane::new("claude", "anthropic").assert_value(),
        run_size("small"),
        BTreeMap::from([(
            NodeName::new("worker").assert_value(),
            serde_json::from_value::<NodeRuntimeBinding>(json!({
                "kind": "agent",
                "model": "claude-haiku-4-5",
                "sessionScope": "execution",
                "connections": {"anthropic": ["CLAUDE_CODE_OAUTH_TOKEN"]},
            }))
            .assert_value(),
        )]),
    );
    let ledger = Arc::new(FakeRunLedger::new());
    ledger
        .create_or_get(CreateRun {
            run_id: run_id(),
            submission_key: IdempotencyKey::new("resume-refusal").assert_value(),
            submission_digest: Sha256Digest::new("b".repeat(64)).assert_value(),
            admitted: NativeV2Admission.admit(submission).await.assert_value(),
        })
        .await
        .assert_value();
    let stored = ledger.get(&run_id()).await.assert_value().assert_value();
    let failure = crate::native_v2_supervisor::logging::terminal_failure_events(
        &stored.snapshot,
        "runtime_unavailable",
        openengine_cluster_protocol::WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
    )
    .assert_value();
    ledger.append(&run_id(), failure).await.assert_value();
    ledger
}

fn resume(
    successor_run_id: RunId,
    connections: openengine_cluster_protocol::RunConnectionValues,
) -> RunResumeParams {
    RunResumeParams {
        runtime: None,
        part_id: run_id(),
        run_id: run_id(),
        successor_run_id,
        from: None,
        connections,
        connection_resolver: None,
        github_token: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,
    }
}

/// The code and message the CLI prints as `remote operation failed with {code}: {message}`.
fn cli_remote_error(error: ClientError) -> (String, String) {
    match error {
        ClientError::Backend(error) => (error.code, error.message),
        ClientError::Rpc(error) => (
            error.data.map(|data| data.code).unwrap_or_default(),
            error.message,
        ),
        error => panic!("expected an OECP refusal, received {error:?}"),
    }
}

#[tokio::test]
async fn resume_runtime_rpc_rejects_contained_checkpoint_opt_out_without_mutation() {
    let ledger = failed_run_declaring_a_connection().await;
    let before = ledger.get(&run_id()).await.assert_value().assert_value();
    let (_address, endpoint, task) = direct_test_server(Arc::new(FailedRunFactory {
        ledger: ledger.clone(),
        checkpoint_opt_out: false,
    }))
    .await;
    let client = connect_client(&endpoint, None).await;
    for uniform in [false, true] {
        let runtime = if uniform {
            json!({"kind":"uniform","runtime":{"harness":"claude","provider":"anthropic","model":"fixture","workspaceCheckpoints":false}})
        } else {
            let mut runtime = serde_json::to_value(&before.admitted.runtime).assert_value();
            runtime["workspaceCheckpoints"] = json!(false);
            json!({"kind":"exact","runtime":runtime})
        };
        let runtime: openengine_cluster_protocol::RunResumeRuntime =
            serde_json::from_value(runtime).assert_value();
        let query = client
            .run_resume_requirements(openengine_cluster_protocol::RunResumeRequirementsParams {
                part_id: None,
                run_id: run_id(),
                runtime: runtime.clone(),
            })
            .await
            .expect_err("contained query must reject checkpoint opt-out");
        let mut params = resume(other_run_id(), BTreeMap::new());
        params.runtime = Some(runtime);
        let rejected = client
            .run_resume(params)
            .await
            .expect_err("contained resume must reject opt-out");
        for error in [query, rejected] {
            let (code, _) = cli_remote_error(error);
            assert_eq!(code, openengine_cluster_protocol::SCHEMA_VIOLATION);
        }
        assert!(ledger.get(&other_run_id()).await.assert_value().is_none());
        let after = ledger.get(&run_id()).await.assert_value().assert_value();
        assert_eq!(after.admitted, before.admitted);
        assert_eq!(after.snapshot, before.snapshot);
    }
    task.abort();
}

#[tokio::test]
async fn resume_runtime_rpc_query_and_successor_share_contained_policy_and_provider_defaults() {
    for (uniform, checkpoint_opt_out) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let ledger = failed_run_declaring_a_connection().await;
        let before = ledger.get(&run_id()).await.assert_value().assert_value();
        let (_address, endpoint, task) = direct_test_server(Arc::new(FailedRunFactory {
            ledger: ledger.clone(),
            checkpoint_opt_out,
        }))
        .await;
        let client = connect_client(&endpoint, None).await;
        let mut runtime = if uniform {
            json!({"kind":"uniform","runtime":{"harness":"claude","provider":"anthropic","model":"fixture"}})
        } else {
            let source = ledger.get(&run_id()).await.assert_value().assert_value();
            let mut runtime = serde_json::to_value(source.admitted.runtime).assert_value();
            runtime["nodes"]["worker"]["connections"] = json!({});
            json!({"kind":"exact","runtime":runtime})
        };
        runtime["runtime"]["workspaceCheckpoints"] = json!(!checkpoint_opt_out);
        let runtime: openengine_cluster_protocol::RunResumeRuntime =
            serde_json::from_value(runtime).assert_value();
        let requirements = client
            .run_resume_requirements(openengine_cluster_protocol::RunResumeRequirementsParams {
                part_id: None,
                run_id: run_id(),
                runtime: runtime.clone(),
            })
            .await;
        assert!(requirements.is_ok(), "{requirements:?}");
        let requirements = requirements.assert_value().connections;
        assert_eq!(
            serde_json::to_value(&requirements).assert_value(),
            json!({"anthropic":["ANTHROPIC_API_KEY"]})
        );
        let field = EnvironmentVariableName::new("ANTHROPIC_API_KEY").assert_value();
        let mut params = resume(
            other_run_id(),
            BTreeMap::from([(
                anthropic(),
                StaticConnectionValues::new(BTreeMap::from([(
                    field,
                    "fresh-api-token".to_owned(),
                )]))
                .assert_value(),
            )]),
        );
        params.runtime = Some(runtime);
        let resumed = client.run_resume(params).await;
        assert!(resumed.is_ok(), "{resumed:?}");
        resumed.assert_value();
        let successor = ledger
            .get(&other_run_id())
            .await
            .assert_value()
            .assert_value();
        assert_eq!(
            successor.admitted.runtime.workspace_checkpoints,
            !checkpoint_opt_out
        );
        assert_eq!(
            ledger
                .get(&run_id())
                .await
                .assert_value()
                .assert_value()
                .admitted,
            before.admitted
        );
        let actual = successor
            .admitted
            .runtime
            .connection_requirements()
            .into_iter()
            .map(|(key, fields)| (key, fields.into_iter().collect::<Vec<_>>()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(actual, requirements);
        task.abort();
    }
}

#[tokio::test]
async fn direct_target_resume_names_a_missing_declared_connection() {
    let factory = Arc::new(FailedRunFactory {
        ledger: failed_run_declaring_a_connection().await,
        checkpoint_opt_out: false,
    });
    let (_address, endpoint, task) = direct_test_server(factory).await;
    let client = connect_client(&endpoint, None).await;

    // `zeroshot resume --target` reads these requirements and selects their values from its shell.
    let status = client
        .run_status(RunStatusParams::new(run_id()))
        .await
        .assert_value();
    assert!(status.workspace_recovery.recoverable);
    assert_eq!(
        status.workspace_recovery.connection_requirements,
        BTreeMap::from([(anthropic(), vec![oauth_token()])])
    );

    // The variable is unset in the resuming shell, so the CLI sends no connection values.
    let refusal = client
        .run_resume(resume(other_run_id(), BTreeMap::new()))
        .await
        .expect_err("resume without the declared connection value must be refused");
    let (code, message) = cli_remote_error(refusal);
    assert_ne!(code, INTERNAL_ERROR_CODE, "{code}: {message}");
    assert!(
        message.contains("anthropic"),
        "the refusal must name the missing connection: {code}: {message}"
    );

    let values =
        StaticConnectionValues::new(BTreeMap::from([(oauth_token(), "fresh-token".to_owned())]))
            .assert_value();
    let resumed = client
        .run_resume(resume(
            RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c993"),
            BTreeMap::from([(anthropic(), values)]),
        ))
        .await
        .assert_value();
    assert_eq!(resumed.resumed_from, run_id());
    task.abort();
}
