use openengine_cluster_protocol::RunCheckpointsResult;

use super::*;
use crate::native_v2_cli::execution::{CliExecutionContext, execute_native_v2_cli_with_context};

#[tokio::test]
async fn checkpoint_listing_emits_one_typed_page_without_resolving_secrets() {
    for target in [None, Some("docker")] {
        let mut values = vec![
            "checkpoints",
            "run-failed",
            "--after",
            "entry-7",
            "--limit",
            "1",
        ];
        if let Some(target) = target {
            values.extend(["--target", target]);
        }
        let command = parse_native_v2_args(args(&values)).assert_value();
        let backend = FakeBackend::default();
        let environment = |name: &str| -> Option<OsString> {
            panic!("checkpoint listing read environment field {name}");
        };
        let context = CliExecutionContext::new(&backend, &environment);
        let mut output = Vec::new();
        let outcome =
            execute_native_v2_cli_with_context(command, &context, &mut NeverDetach, &mut output)
                .await
                .assert_value();
        assert_eq!(outcome, CliOutcome::Completed);
        assert_eq!(
            backend.calls(),
            [Call::Checkpoints {
                target: target.map(str::to_owned),
                params: RunCheckpointsParams {
                    run_id: RunId::new("run-failed"),
                    after: Some(CheckpointId::new("entry-7").assert_value()),
                    limit: Some(1),
                },
            }]
        );
        let result: RunCheckpointsResult = serde_json::from_slice(&output).assert_value();
        assert_eq!(result.run_id.as_str(), "run-failed");
        assert_eq!(
            result.next_after.as_ref().map(CheckpointId::as_str),
            Some("entry-8")
        );
        assert_eq!(result.checkpoints.len(), 1);
        let checkpoint = &result.checkpoints[0];
        assert_eq!(checkpoint.checkpoint_id.as_str(), "entry-8");
        assert_eq!(checkpoint.sequence.get(), 8);
        assert_eq!(checkpoint.node.as_str(), "repair");
        assert_eq!(checkpoint.map_indices, [2, 1]);
        assert_eq!(checkpoint.loop_iterations, [3]);
        assert_eq!(checkpoint.created_at.get(), 42);
    }
}
