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
                    part_id: None,
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

#[tokio::test]
async fn failed_resume_selects_newest_entry_across_pages_for_local_and_named_runs() {
    for target in [None, Some("docker")] {
        let backend = FakeBackend::with_resume_requirements(Default::default())
            .with_resume_part("part-failed")
            .with_checkpoint_pages(vec![
                checkpoint_page("z-first", 1, Some("z-first")),
                checkpoint_page("a-latest", 2, None),
            ]);
        let mut values = vec!["resume", "run-failed"];
        if let Some(target) = target {
            values.extend(["--target", target]);
        } else {
            values.push("--discard-workspace-changes");
        }
        execute_with_environment(
            parse_native_v2_args(args(&values)).assert_value(),
            &backend,
            &|_| None,
        )
        .await
        .assert_value();
        let calls = backend.calls();
        let [
            Call::Status { .. },
            Call::Checkpoints { params: first, .. },
            Call::Checkpoints { params: second, .. },
            Call::Resume {
                params,
                discard_workspace_changes,
                ..
            },
            Call::Logs { .. },
        ] = calls.as_slice()
        else {
            panic!("unexpected calls: {calls:?}");
        };
        assert_eq!(first.run_id.as_str(), "run-failed");
        assert_eq!(second.run_id, first.run_id);
        assert_eq!(
            first.part_id.as_ref().map(RunId::as_str),
            Some("part-failed")
        );
        assert_eq!(second.part_id, first.part_id);
        assert_eq!(params.run_id.as_str(), "run-failed");
        assert_eq!(params.part_id.as_str(), "part-failed");
        assert_eq!(first.after, None);
        assert_eq!(first.limit, Some(100));
        assert_eq!(
            second.after.as_ref().map(CheckpointId::as_str),
            Some("z-first")
        );
        assert_eq!(
            params.from,
            Some(RunResumeFrom::Checkpoint {
                checkpoint_id: CheckpointId::new("a-latest").assert_value()
            })
        );
        assert_eq!(*discard_workspace_changes, target.is_none());
    }
}

#[tokio::test]
async fn failed_resume_without_checkpoints_requires_explicit_restart_before_credentials() {
    for route in [
        &[][..],
        &["--target", "docker"][..],
        &["--controller", "contained"][..],
    ] {
        let backend = FakeBackend::with_resume_requirements(Default::default())
            .with_checkpoint_pages(vec![RunCheckpointsResult {
                run_id: RunId::new("run-failed"),
                part_id: RunId::new("run-failed"),
                controller: None,
                checkpoints: vec![],
                next_after: None,
            }]);
        let mut values = vec!["resume", "run-failed"];
        values.extend_from_slice(route);
        let error = execute_with_environment(
            parse_native_v2_args(args(&values)).assert_value(),
            &backend,
            &|name| panic!("read credential {name}"),
        )
        .await
        .assert_error();
        let target_flag = if route.is_empty() {
            String::new()
        } else {
            format!(" {}", route.join(" "))
        };
        assert!(
            matches!(&error, NativeV2CliError::Usage(message)
            if message.contains("no entry checkpoints") && message.contains(&format!("zeroshot resume run-failed{target_flag} --restart"))),
            "{error}"
        );
        assert!(
            !backend
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Resume { .. }))
        );
    }
}

#[tokio::test]
async fn sdk_api_default_preserves_omitted_selector_for_local_and_named_runs() {
    for target in [None, Some("docker")] {
        let backend = FakeBackend::with_resume_requirements(Default::default());
        let mut values = vec!["resume", "run-failed", "--api-default"];
        if let Some(target) = target {
            values.extend(["--target", target]);
        }
        execute_with_environment(
            parse_native_v2_args(args(&values)).assert_value(),
            &backend,
            &|_| None,
        )
        .await
        .assert_value();
        let calls = backend.calls();
        let [
            Call::Status { .. },
            Call::Resume { params, .. },
            Call::Logs { .. },
        ] = calls.as_slice()
        else {
            panic!("unexpected calls: {calls:?}");
        };
        assert_eq!(params.from, None);
    }
}

fn checkpoint_page(id: &str, sequence: u64, next: Option<&str>) -> RunCheckpointsResult {
    serde_json::from_value(json!({
        "runId":"run-failed", "partId":"part-failed", "checkpoints":[{
            "checkpointId":id, "sequence":sequence, "node":"review", "mapIndices":[],
            "loopIterations":[sequence - 1], "createdAt":42
        }], "nextAfter":next
    }))
    .assert_value()
}

#[tokio::test]
async fn default_resume_rejects_checkpoint_pages_that_do_not_advance() {
    let backend =
        FakeBackend::with_resume_requirements(Default::default()).with_checkpoint_pages(vec![
            checkpoint_page("first", 2, Some("first")),
            checkpoint_page("stale", 1, None),
        ]);
    let error = execute_with_environment(
        parse_native_v2_args(args(&["resume", "run-failed", "--target", "docker"])).assert_value(),
        &backend,
        &|name| panic!("read credential {name}"),
    )
    .await
    .assert_error();
    assert!(
        error
            .to_string()
            .contains("checkpoint listing did not advance"),
        "{error}"
    );
    assert!(
        !backend
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Resume { .. }))
    );
}
