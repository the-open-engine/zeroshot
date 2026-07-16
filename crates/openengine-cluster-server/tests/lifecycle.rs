use async_trait::async_trait;
use openengine_cluster_protocol::{
    ClusterStatus, Cursor, DispatchState, Generation, GetParams, GetResult, InitializeParams,
    InitializeResult, OperationalStatus, Phase, RunId, ServerCapabilities, StopMode, StopParams,
    StopResult, UpdateParams, UpdateResult, SCHEMA_VIOLATION,
};
use openengine_cluster_server::{BackendError, ClusterBackend, ConnectionContext, Dispatcher};
use serde_json::json;

struct RoutingBackend;

fn operational(state: DispatchState, mode: Option<StopMode>) -> OperationalStatus {
    OperationalStatus {
        dispatch_state: state,
        stop_mode: mode,
        ..OperationalStatus::default()
    }
}

#[async_trait]
impl ClusterBackend for RoutingBackend {
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        Ok(InitializeResult::new(
            ServerCapabilities::default(),
            ClusterStatus::empty(),
        ))
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        _params: GetParams,
    ) -> Result<GetResult, BackendError> {
        unreachable!()
    }

    async fn update(
        &self,
        _context: &ConnectionContext,
        _params: UpdateParams,
    ) -> Result<UpdateResult, BackendError> {
        Ok(UpdateResult {
            generation: Generation::new(1).unwrap(),
            run_id: RunId::new("run-1"),
            phase: Phase::Running,
            operational: operational(DispatchState::Suspended, None),
            at_cursor: Cursor::new("cursor-2"),
            deduped: false,
        })
    }

    async fn stop(
        &self,
        _context: &ConnectionContext,
        _params: StopParams,
    ) -> Result<StopResult, BackendError> {
        Ok(StopResult {
            generation: Generation::new(1).unwrap(),
            run_id: RunId::new("run-1"),
            phase: Phase::Finished,
            accepted_mode: StopMode::Force,
            effective_mode: StopMode::Force,
            operational: operational(DispatchState::Stopped, Some(StopMode::Force)),
            at_cursor: Cursor::new("cursor-4"),
            deduped: false,
        })
    }
}

#[tokio::test]
async fn lifecycle_dispatch_routes_typed_methods_and_rejects_mutation_fields() {
    let dispatcher = Dispatcher::new(RoutingBackend, ConnectionContext::default());
    let update: serde_json::Value = serde_json::from_str(
        &dispatcher
            .dispatch(
                &json!({
                    "jsonrpc":"2.0","id":1,"method":"update",
                    "params":{"suspended":true,"ifGeneration":1,"idempotencyKey":"suspend"}
                })
                .to_string(),
            )
            .await,
    )
    .unwrap();
    assert_eq!(
        update["result"]["operational"]["dispatchState"],
        "suspended"
    );

    let stop: serde_json::Value = serde_json::from_str(
        &dispatcher
            .dispatch(
                &json!({
                    "jsonrpc":"2.0","id":2,"method":"stop",
                    "params":{"mode":"force","ifGeneration":1,"idempotencyKey":"force"}
                })
                .to_string(),
            )
            .await,
    )
    .unwrap();
    assert_eq!(stop["result"]["effectiveMode"], "force");

    for params in [
        json!({"ifGeneration":1,"idempotencyKey":"empty"}),
        json!({"graph":{},"ifGeneration":1,"idempotencyKey":"graph"}),
        json!({"input":null,"ifGeneration":1,"idempotencyKey":"input"}),
        json!({"policy":{},"ifGeneration":1,"idempotencyKey":"policy"}),
        json!({"worker":"x","ifGeneration":1,"idempotencyKey":"worker"}),
    ] {
        let response: serde_json::Value = serde_json::from_str(
            &dispatcher
                .dispatch(
                    &json!({"jsonrpc":"2.0","id":3,"method":"update","params":params}).to_string(),
                )
                .await,
        )
        .unwrap();
        assert_eq!(response["error"]["data"]["code"], SCHEMA_VIOLATION);
    }
}
