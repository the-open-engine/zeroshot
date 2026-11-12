use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

struct RejectedRunner(NodeRunnerError);

#[async_trait]
impl NodeRunner for RejectedRunner {
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        let (handle, bridge) = remote_node_handle(request.invocation.reference);
        bridge.finish(Err(self.0.clone()));
        Ok(handle)
    }

    async fn close_run(&self, _run_id: &RunId) {}
}

#[tokio::test]
async fn provider_rejection_class_and_model_diagnostic_survive_the_capsule_transport() {
    for (error, expected) in [
        (
            NodeRunnerError::AuthenticationRequired("rejected credential".into()),
            CapsuleNodeFailure::AuthenticationRequired,
        ),
        (
            NodeRunnerError::ModelUnavailable("model unavailable-model was rejected".into()),
            CapsuleNodeFailure::ModelUnavailable,
        ),
        (
            NodeRunnerError::DriverDetail("response stream cut".into()),
            CapsuleNodeFailure::ExecutionFailed,
        ),
    ] {
        let endpoint = Arc::new(NativeCapsuleNodeEndpoint::new(Arc::new(RejectedRunner(
            error.clone(),
        ))));
        let proxy = RemoteCapsuleNodeRunner::new(endpoint);
        let mut handle = proxy
            .start(request("provider-rejection", 1))
            .await
            .assert_value();
        let mut durable = handle.take_initial_output().assert_value();
        let collect = async {
            let mut logs = Vec::new();
            while let Ok(event) = durable.recv().await {
                if let DurableNodeEvent::Output { output, .. } = event {
                    logs.push(output.text);
                }
            }
            logs
        };
        let (logs, result) = tokio::join!(collect, handle.completion());
        let received = result.unwrap_err();
        assert_eq!(CapsuleNodeFailure::from_runner(&received), expected);
        assert!(logs.join("\n").contains(&error.to_string()));
        let wire =
            serde_json::to_string(&CapsuleNodeEvent::Failed { failure: expected }).assert_value();
        let event: CapsuleNodeEvent = serde_json::from_str(&wire).assert_value();
        let CapsuleNodeEvent::Failed { failure } = event else {
            panic!("expected failed event")
        };
        assert_eq!(
            CapsuleNodeFailure::from_runner(&failure.into_runner()),
            expected
        );
    }
}
