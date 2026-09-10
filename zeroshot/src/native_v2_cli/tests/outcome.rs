use openengine_cluster_protocol::MergePlanState;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::*;
use crate::native_v2_cli::execution::{CliExecutionContext, execute_native_v2_cli_with_context};

#[test]
fn version_reports_the_packaged_binary_version_without_a_backend() {
    let mut output = Vec::new();
    let outcome = try_execute_native_v2_static(&NativeV2CliCommand::Version, &mut output)
        .assert_value()
        .assert_value();
    assert_eq!(outcome, CliOutcome::Completed);
    assert_eq!(String::from_utf8(output).assert_value(), VERSION);
}

#[tokio::test]
async fn foreground_run_reports_a_terminal_failure_after_printing_it() {
    let files = FixtureFiles::new(graph(), json!({"task":"fail"}));
    let command = parse_native_v2_args(run_args(&files.graph, &files.input, &files.runtime, &[]))
        .assert_value();
    let backend = FakeBackend::with_failed_watch();
    let mut output = Vec::new();
    let error = execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut output)
        .await
        .assert_error();
    assert!(matches!(error, NativeV2CliError::RunFailed));
    assert!(
        String::from_utf8(output)
            .assert_value()
            .contains("\"reason\":\"worker_failed\"")
    );
}

fn merge_plan_fixture() -> FixtureFiles {
    let expires = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    let expires_at = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        expires.year(),
        u8::from(expires.month()),
        expires.day(),
        expires.hour(),
        expires.minute(),
        expires.second()
    );
    FixtureFiles::new(
        graph(),
        json!({
            "schema":"zeroshot.merge-plan/v1",
            "title":"Release",
            "source":{"repository":"open-engine/zeroshot","branch":"main"},
            "profile":"org:software-change",
            "expiresAt":expires_at,
            "runs":{"build":{"input":{"task":"fail"}}}
        }),
    )
}

fn plan_submit_command(file: &Path, detach: bool) -> NativeV2CliCommand {
    let mut values = vec![
        OsString::from("plan"),
        OsString::from("submit"),
        file.as_os_str().to_owned(),
        OsString::from("--target"),
        OsString::from("prod"),
        OsString::from("--submission-key"),
        OsString::from("release-1"),
    ];
    if detach {
        values.push(OsString::from("--detach"));
    }
    parse_native_v2_args(values).assert_value()
}

async fn execute_plan(
    command: NativeV2CliCommand,
    state: MergePlanState,
) -> (Result<CliOutcome, NativeV2CliError>, String) {
    let backend = FakeBackend::with_terminal_plan_state(state);
    let environment = |name: &str| (name == "GH_TOKEN").then(|| OsString::from("test-token"));
    let context = CliExecutionContext::new(&backend, &environment);
    let mut output = Vec::new();
    let result =
        execute_native_v2_cli_with_context(command, &context, &mut NeverDetach, &mut output).await;
    (result, String::from_utf8(output).assert_value())
}

#[tokio::test]
async fn foreground_plan_submit_reports_a_terminal_failure_after_printing_it() {
    let files = merge_plan_fixture();
    let command = plan_submit_command(&files.input, false);

    let (result, output) = execute_plan(command, MergePlanState::Failed).await;

    assert!(matches!(
        result.assert_error(),
        NativeV2CliError::MergePlanFailed
    ));
    assert!(output.contains("\"planId\":\"plan-public\""));
    assert!(output.contains("\"state\":\"failed\""));
}

#[tokio::test]
async fn plan_watch_reports_each_unsuccessful_terminal_state_after_printing_it() {
    for (state, expected) in [
        (MergePlanState::Failed, "failed"),
        (MergePlanState::Cancelled, "cancelled"),
        (MergePlanState::Expired, "expired"),
    ] {
        let command =
            parse_native_v2_args(args(&["plan", "watch", "plan-public", "--target", "prod"]))
                .assert_value();

        let (result, output) = execute_plan(command, state).await;

        assert!(matches!(
            result.assert_error(),
            NativeV2CliError::MergePlanFailed
        ));
        assert!(output.contains(&format!("\"state\":\"{expected}\"")));
    }
}

#[tokio::test]
async fn detached_plan_submit_does_not_project_a_terminal_failure() {
    let files = merge_plan_fixture();
    let command = plan_submit_command(&files.input, true);

    let (result, output) = execute_plan(command, MergePlanState::Failed).await;

    assert_eq!(result.assert_value(), CliOutcome::Detached);
    assert!(output.contains("\"state\":\"failed\""));
}

#[tokio::test]
async fn plan_status_and_force_stop_keep_ordinary_command_outcomes() {
    for operation in ["status", "force-stop"] {
        let command = parse_native_v2_args(args(&[
            "plan",
            operation,
            "plan-public",
            "--target",
            "prod",
        ]))
        .assert_value();

        let (result, output) = execute_plan(command, MergePlanState::Expired).await;

        assert_eq!(result.assert_value(), CliOutcome::Completed);
        assert!(output.contains("\"state\":\"expired\""));
    }
}
