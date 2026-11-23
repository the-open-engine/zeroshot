use super::*;
use crate::native_v2_hosting::{
    HostedWorkspace, HostedWorkspaceRequest, HostedWorkspaceStorage, ProductionHostingConfig,
    ProductionTargetControllerFactory,
};
use crate::native_v2_supervisor::checkpoints::CheckpointError;

struct UnusedStorage;

#[async_trait]
impl HostedWorkspaceStorage for UnusedStorage {
    async fn retain_delivery_state(
        &self,
        _: &RunId,
        _: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), CheckpointError> {
        panic!("admission must not reach storage")
    }

    async fn prepare(
        &self,
        _: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, CheckpointError> {
        panic!("admission must not reach storage")
    }
}

#[tokio::test]
async fn production_http_checkpoint_opt_out_requires_explicit_host_storage_and_admission() {
    for enabled in [false, true] {
        for hosted in [false, true] {
            let root = tempfile::tempdir().assert_value();
            let factory = Arc::new(ProductionTargetControllerFactory::new(
                ProductionHostingConfig {
                    storage_root: root.path().to_owned(),
                    run_sizes: Some(fixture_run_sizes()),
                    workspace_checkpoint_opt_out: enabled,
                    workspace_storage: hosted
                        .then(|| Arc::new(UnusedStorage) as Arc<dyn HostedWorkspaceStorage>),
                    ..Default::default()
                },
            ));
            let (address, _, task) = direct_test_server(factory.clone()).await;
            let document = target_discovery(address).await;
            assert_eq!(
                document.extensions.workspace_checkpoint_opt_out.is_some(),
                enabled && hosted
            );
            let mut body = request();
            body.submission.runtime.workspace_checkpoints = false;
            // A nested imported wait must be rejected by trusted admission even if the client
            // bypasses discovery. The malformed fixed contract is deliberately secondary.
            body.submission.graph = serde_json::from_value(json!({
                "profile":"openengine.graph.full/v1", "initialInput":{"kind":"null"},
                "policy":{"policy":"policy.native-v2@1", "default":"deny"},
                "root":{"kind":"profile", "name":"imported", "profile":"saved",
                    "initialInput":{"kind":"null"},
                    "body":{"kind":"seq", "name":"inner", "state":{"kind":"null"},
                        "promotedStatePaths":[], "children":[{
                            "kind":"verifier", "name":"wait", "worker":crate::native_v2_wait::WAIT_WORKER_REF,
                            "input":{"kind":"null"}, "output":{"kind":"null"},
                            "inputBindings":[], "writeBindings":[], "signals":{},
                            "diagnostic":{"kind":"null"}, "attempts":1
                        }]}}
            })).assert_value();
            let response = http(
                address,
                TestHttpRequest::body(
                    "POST",
                    RUN_PATH,
                    None,
                    &serde_json::to_vec(&body).assert_value(),
                ),
            )
            .await;
            assert_eq!(
                response.status,
                400,
                "{}",
                String::from_utf8_lossy(&response.body)
            );
            let problem: TargetHttpProblem = serde_json::from_slice(&response.body).assert_value();
            assert!(
                problem.message().contains(if enabled && hosted {
                    "hosted request and wait nodes require workspaceCheckpoints: true"
                } else {
                    "does not support disabling workspace checkpoints"
                }),
                "{}",
                problem.message()
            );
            let controller = factory.create_controller().await.assert_value();
            assert!(controller.list().await.assert_value().is_empty());
            task.abort();
        }
    }
}
