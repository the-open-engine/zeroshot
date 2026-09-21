use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    CheckpointId, GetParams, GetResult, InitializeParams, InitializeResult, RunCheckpointsParams,
    RunCheckpointsResult, RunId, RunResumeFrom, RunResumeParams, RunResumeResult,
    APPLICATION_ERROR, INVALID_PARAMS, INVALID_PHASE, SCHEMA_VIOLATION,
};
use openengine_cluster_server::{BackendError, ClusterBackend, ConnectionContext, Dispatcher};
use openengine_cluster_testkit::{assertions::AssertValue, EmptyBackend};
use serde_json::{json, Value};

#[derive(Default)]
struct CheckpointBackend {
    calls: AtomicUsize,
}

#[async_trait]
impl ClusterBackend for CheckpointBackend {
    async fn initialize(
        &self,
        context: &ConnectionContext,
        params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        EmptyBackend.initialize(context, params).await
    }

    async fn get(
        &self,
        context: &ConnectionContext,
        params: GetParams,
    ) -> Result<GetResult, BackendError> {
        EmptyBackend.get(context, params).await
    }

    async fn run_checkpoints(
        &self,
        _context: &ConnectionContext,
        params: RunCheckpointsParams,
    ) -> Result<RunCheckpointsResult, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            params.after.as_ref().map(CheckpointId::as_str),
            Some("entry-1")
        );
        assert_eq!(params.page_limit(), 1);
        Ok(serde_json::from_value(json!({
            "runId": params.run_id,
            "checkpoints": [{
                "checkpointId": "entry-2", "sequence": 2, "node": "writers",
                "mapIndices": [], "loopIterations": [0], "createdAt": 42
            }],
            "nextAfter": "entry-2"
        }))
        .assert_value())
    }

    async fn run_resume(
        &self,
        _context: &ConnectionContext,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            params.from,
            Some(RunResumeFrom::Checkpoint {
                checkpoint_id: CheckpointId::new("entry-2").assert_value(),
            })
        );
        Ok(RunResumeResult {
            run_id: params.successor_run_id,
            resumed_from: params.run_id,
        })
    }
}

async fn call<B: ClusterBackend>(dispatcher: &Dispatcher<B>, method: &str, params: Value) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    serde_json::from_str(&dispatcher.dispatch(&request.to_string()).await).assert_value()
}

#[tokio::test]
async fn checkpoint_listing_and_selected_resume_reach_the_backend() {
    let backend = Arc::new(CheckpointBackend::default());
    let dispatcher = Dispatcher::from_shared(Arc::clone(&backend), ConnectionContext::default());
    let response = call(
        &dispatcher,
        "run/checkpoints",
        json!({
            "runId": "prior", "after": "entry-1", "limit": 1
        }),
    )
    .await;
    assert_eq!(response["result"]["runId"], "prior");
    assert_eq!(response["result"]["checkpoints"][0]["node"], "writers");
    assert_eq!(response["result"]["nextAfter"], "entry-2");
    let resumed = call(
        &dispatcher,
        "run/resume",
        json!({
            "runId": "prior", "successorRunId": "successor",
            "from": {"kind": "checkpoint", "checkpointId": "entry-2"}
        }),
    )
    .await;
    assert_eq!(
        resumed["result"],
        json!({"runId": "successor", "resumedFrom": "prior"})
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn invalid_wire_and_typed_queries_are_rejected_before_backend_dispatch() {
    let backend = Arc::new(CheckpointBackend::default());
    let dispatcher = Dispatcher::from_shared(Arc::clone(&backend), ConnectionContext::default());
    for params in [
        json!({"runId": "prior", "limit": 0}),
        json!({"runId": "prior", "limit": 101}),
        json!({"runId": "prior", "after": ""}),
    ] {
        let response = call(&dispatcher, "run/checkpoints", params).await;
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
        assert_eq!(response["error"]["data"]["code"], SCHEMA_VIOLATION);
    }
    let error = dispatcher
        .run_checkpoints(RunCheckpointsParams {
            run_id: RunId::new("prior"),
            after: None,
            limit: Some(0),
        })
        .await
        .err()
        .assert_value();
    assert_eq!(error.code, SCHEMA_VIOLATION);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn legacy_backends_explicitly_reject_checkpoints() {
    let dispatcher = Dispatcher::new(EmptyBackend, ConnectionContext::default());
    let response = call(&dispatcher, "run/checkpoints", json!({"runId": "prior"})).await;
    assert_eq!(response["error"]["code"], APPLICATION_ERROR);
    assert_eq!(response["error"]["data"]["code"], INVALID_PHASE);
}
