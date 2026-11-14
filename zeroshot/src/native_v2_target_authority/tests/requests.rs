use super::*;
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId};
use crate::v2_run_ledger::{CreateRun, RunEvent, RunLedger};
use openengine_cluster_protocol::{
    InboxListParams, NodeName, PositiveInteger, RequestState, RunStatusParams, RunSuspension,
    RunVector, RunVectorContent, Sha256Digest, UnixTimestampMillis, WorkspaceRecovery,
};

#[derive(Default)]
struct RequestAllocator {
    claims: std::sync::Mutex<BTreeMap<RunId, RunId>>,
}
#[async_trait]
impl CapsuleAllocator for RequestAllocator {
    fn supports_request_ask(&self) -> bool {
        true
    }
    fn suspension_policy(&self) -> Option<crate::native_v2_wait::WaitPolicy> {
        Some(Default::default())
    }
    async fn claim_controller(
        &self,
        run: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        NoAllocation.claim_controller(run).await
    }
    async fn allocate(
        &self,
        request: crate::native_v2_platform::CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        NoAllocation.allocate(request).await
    }
    async fn destroy_or_confirm_absent(
        &self,
        run: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        NoAllocation.destroy_or_confirm_absent(run, exit).await
    }
    async fn workspace_recovery(&self, run: &RunId) -> WorkspaceRecovery {
        let successor = self.claims.lock().assert_value().get(run).cloned();
        WorkspaceRecovery {
            recoverable: successor.is_none(),
            successor_run_id: successor,
            ..Default::default()
        }
    }
    async fn reserve_retained_workspace(
        &self,
        source: &RunId,
        run: &RunId,
        _: &crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection,
    ) -> Result<(), crate::native_v2_platform::RetainedAllocationUnavailable> {
        let mut claims = self.claims.lock().assert_value();
        if claims.insert(source.clone(), run.clone()).is_some() {
            return Err(CapsuleAllocationUnavailable::Runtime.into());
        }
        Ok(())
    }
}
struct RequestFactory {
    ledger: Arc<FakeRunLedger>,
    allocator: Arc<RequestAllocator>,
}
#[async_trait]
impl TargetControllerFactory for RequestFactory {
    fn supports_request_ask(&self) -> bool {
        true
    }
    fn supports_run_suspension(&self) -> bool {
        true
    }
    fn supports_workspace_recovery(&self) -> bool {
        true
    }
    fn run_sizes(&self) -> Option<openengine_cluster_protocol::TargetRunSizesDiscovery> {
        Some(fixture_run_sizes())
    }
    async fn create(&self) -> Result<Arc<NativeV2PlatformController>, TargetAuthorityError> {
        NativeV2PlatformController::new_with_delivery_policy(
            self.ledger.clone(),
            self.allocator.clone(),
            DeliveryPolicy::Optional,
        )
        .await
        .map(Arc::new)
        .map_err(|error| TargetAuthorityError::unavailable(error.to_string()))
    }
    async fn submit(
        &self,
        _: &NativeV2PlatformController,
        _: TargetRunRequest,
    ) -> Result<TargetRunReceipt, TargetAuthorityError> {
        Err(TargetAuthorityError::invalid("fixture only resumes"))
    }
}
async fn seed(ledger: &FakeRunLedger, index: u8, expired: bool) -> RunId {
    let id = RunId::new(format!("018f5e78-7f95-7c22-8d98-3f15af20c9{index:02}"));
    let mut submission = request().submission;
    submission.graph = serde_json::from_value(json!({
        "profile": "openengine.graph.full/v1",
        "initialInput": {
            "kind": "record",
            "fields": {"text": {"required": true, "type": {"kind": "string"}}}
        },
        "policy": {"policy": "policy.native-v2@1", "default": "deny"},
        "root": {
            "kind": "seq",
            "name": "root",
            "state": {
                "kind": "record",
                "fields": {"text": {"required": true, "type": {"kind": "string"}}}
            },
            "children": [
                {
                    "kind": "verifier",
                    "name": "approval",
                    "worker": crate::native_v2_request::ASK_WORKER_REF,
                    "input": crate::native_v2_request::input_schema().assert_value(),
                    "output": {"kind": "record", "fields": {}},
                    "signals": {"decision": ["approved", "rejected"]},
                    "diagnostic": crate::native_v2_wait::diagnostic_schema().assert_value(),
                    "inputBindings": [{
                        "target": ["text"],
                        "value": {"source": "state", "path": ["text"]}
                    }],
                    "writeBindings": [],
                    "attempts": 1
                },
                {"kind": "succeed", "name": "done", "output": {"kind": "null"}, "bindings": []}
            ],
            "promotedStatePaths": []
        }
    }))
    .assert_value();
    submission.initial_input = json!({"text":"Approve?"});
    submission.runtime=serde_json::from_value(json!({"harness":"codex","provider":"openai","size":"small","nodes":{"approval":{"kind":"request"}}})).assert_value();
    let admitted = NativeV2Admission
        .admit_with_policy(submission, DeliveryPolicy::Optional)
        .await
        .assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: id.clone(),
            submission_key: IdempotencyKey::new(format!("request-{index}")).assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted,
        })
        .await
        .assert_value();
    let suspension:RunSuspension=serde_json::from_value(json!({"reason":"request","node":"approval","createdAt":1000,"request":{"id":format!("question-{index}"),"text":"Approve?","signals":{"decision":["approved","rejected"]},"output":{"kind":"record","fields":{}},"expiresAt":if expired {1001} else {UnixTimestampMillis::new(openengine_cluster_protocol::MAX_SAFE_GENERATION).assert_value().get()}}})).assert_value();
    let RunSuspension::Request {
        request: question, ..
    } = &suspension
    else {
        unreachable!()
    };
    let reference = ExecutionRef {
        run_id: id.clone(),
        node: NodeName::new("approval").assert_value(),
        node_instance: NodeInstanceId::new(1).assert_value(),
        execution: ExecutionId::new(1).assert_value(),
    };
    ledger
        .append(
            &id,
            vec![
                RunEvent::RunStarted,
                RunEvent::NodeStarted {
                    reference: reference.clone(),
                    occurrence: crate::full_v1_reducer::StructuralOccurrence {
                        node: reference.node.clone(),
                        map_indices: vec![],
                    },
                    attempt: PositiveInteger::new(1).assert_value(),
                    input: json!({"text":"Approve?"}),
                },
                RunEvent::RequestDeferred { reference },
                RunEvent::Vector {
                    vector: RunVector {
                        id: question.id.clone(),
                        run_id: id.clone(),
                        created_at: UnixTimestampMillis::new(1000).assert_value(),
                        content: RunVectorContent::Request {
                            node: NodeName::new("approval").assert_value(),
                            text: question.text.clone(),
                            signals: question.signals.clone(),
                            output: question.output.clone(),
                            expires_at: question.expires_at,
                        },
                    },
                },
                RunEvent::Suspended { suspension },
            ],
        )
        .await
        .assert_value();
    id
}
#[tokio::test]
async fn direct_request_discovery_inbox_and_expiry_resume_use_the_controller_ledger() {
    let ledger = Arc::new(FakeRunLedger::new());
    let mut ids = vec![];
    for index in 1..=3 {
        ids.push(seed(&ledger, index, index == 3).await);
    }
    let factory = Arc::new(RequestFactory {
        ledger: ledger.clone(),
        allocator: Arc::new(RequestAllocator::default()),
    });
    let (address, endpoint, task) = direct_test_server(factory).await;
    let discovery = http(address, TestHttpRequest::empty("GET", DISCOVERY_PATH, None)).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&discovery.body).assert_value()["extensions"]["request_ask"]
            ["kind"],
        "openengine.request-ask/v1"
    );
    let client = connect_client(&endpoint, None).await;
    let page = client
        .inbox_list(InboxListParams {
            after: None,
            limit: Some(2),
        })
        .await
        .assert_value();
    assert_eq!(page.requests.len(), 2);
    assert_eq!(page.next_after, Some(ids[1].clone()));
    let page = client
        .inbox_list(InboxListParams {
            after: page.next_after,
            limit: Some(2),
        })
        .await
        .assert_value();
    assert_eq!(page.requests[0].state, RequestState::Expired);
    let show = client
        .inbox_show(RunStatusParams {
            run_id: ids[2].clone(),
        })
        .await
        .assert_value();
    assert_eq!(show.request.text, "Approve?");
    let params = RunResumeParams {
        run_id: ids[2].clone(),
        successor_run_id: other_run_id(),
        from: Some(
            serde_json::from_value(
                json!({"kind":"answer","signals":{"decision":"approved"},"output":{}}),
            )
            .assert_value(),
        ),
        connections: Default::default(),
        connection_resolver: None,
        github_token: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,
    };
    assert!(
        client
            .run_resume(params.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("expired")
    );
    let show = client
        .inbox_show(RunStatusParams {
            run_id: ids[2].clone(),
        })
        .await
        .assert_value();
    assert_eq!(show.successor_run_id, Some(other_run_id()));
    assert_eq!(show.state, RequestState::Expired);
    assert!(
        client
            .run_resume(params)
            .await
            .unwrap_err()
            .to_string()
            .contains(other_run_id().as_str())
    );
    assert_eq!(ledger.list().await.assert_value().len(), 4);
    task.abort();
}
