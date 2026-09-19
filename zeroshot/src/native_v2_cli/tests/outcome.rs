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

#[tokio::test]
async fn named_target_resume_resolves_current_values_for_original_requirements() {
    assert_resume_resolves_current_values(Some("docker")).await;
}

#[tokio::test]
async fn named_target_resume_rejects_target_selected_environment_fields_before_lookup() {
    let key = openengine_cluster_protocol::ConnectionKey::new("openai").assert_value();
    let trusted =
        openengine_cluster_protocol::EnvironmentVariableName::new("OPENAI_API_KEY").assert_value();
    let selected =
        openengine_cluster_protocol::EnvironmentVariableName::new("AWS_SECRET_ACCESS_KEY")
            .assert_value();
    let backend = FakeBackend::with_untrusted_resume_requirements(
        std::collections::BTreeMap::from([(key.clone(), vec![selected])]),
        std::collections::BTreeMap::from([(key, vec![trusted])]),
    );
    let command =
        parse_native_v2_args(args(&["resume", "run-failed", "--target", "docker"])).assert_value();
    let accessed = std::sync::Mutex::new(Vec::new());
    let environment = |name: &str| {
        accessed.lock().assert_value().push(name.to_owned());
        Some(OsString::from("must-not-be-read"))
    };
    let error = execute_resume_expect_error(command, &backend, &environment).await;

    assert!(error.to_string().contains("do not match the original run"));
    assert!(accessed.lock().assert_value().is_empty());
    assert_no_resume_call(&backend);
}

#[tokio::test]
async fn local_resume_resolves_current_values_for_original_requirements() {
    assert_resume_resolves_current_values(None).await;
}

async fn assert_resume_resolves_current_values(expected_target: Option<&str>) {
    let key = openengine_cluster_protocol::ConnectionKey::new("openai").assert_value();
    let field =
        openengine_cluster_protocol::EnvironmentVariableName::new("OPENAI_API_KEY").assert_value();
    let backend = FakeBackend::with_resume_requirements(std::collections::BTreeMap::from([(
        key.clone(),
        vec![field.clone()],
    )]));
    let command_args = match expected_target {
        Some(target) => vec!["resume", "run-failed", "--target", target],
        None => vec!["resume", "run-failed"],
    };
    let command = parse_native_v2_args(args(&command_args)).assert_value();
    let environment = |name: &str| match name {
        "OPENAI_API_KEY" => Some(OsString::from("fresh-openai-token")),
        "GH_TOKEN" => Some(OsString::from("fresh-github-token")),
        _ => None,
    };
    let context = CliExecutionContext::new(&backend, &environment);

    execute_native_v2_cli_with_context(command, &context, &mut NeverDetach, &mut Vec::new())
        .await
        .assert_value();

    let calls = backend.calls();
    let Call::Resume { target, params } = &calls[1] else {
        panic!("resume call was not recorded");
    };
    assert_eq!(target.as_deref(), expected_target);
    assert_eq!(
        params.connections[&key]
            .as_map()
            .get(&field)
            .map(String::as_str),
        Some("fresh-openai-token")
    );
    assert_eq!(params.github_token.as_deref(), Some("fresh-github-token"));
}

#[tokio::test]
async fn resume_rejects_an_invalid_github_token_before_submission() {
    let backend = FakeBackend::with_resume_requirements(Default::default());
    let command = parse_native_v2_args(args(&["resume", "run-failed"])).assert_value();
    let oversized = "x".repeat(4_097);
    let environment = |name: &str| (name == "GH_TOKEN").then(|| OsString::from(&oversized));
    let error = execute_resume_expect_error(command, &backend, &environment).await;

    assert!(matches!(error, NativeV2CliError::GitHubToken));
    assert_no_resume_call(&backend);
}

fn assert_no_resume_call(backend: &FakeBackend) {
    assert!(
        backend
            .calls()
            .iter()
            .all(|call| !matches!(call, Call::Resume { .. }))
    );
}

async fn execute_resume_expect_error(
    command: NativeV2CliCommand,
    backend: &FakeBackend,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> NativeV2CliError {
    let context = CliExecutionContext::new(backend, environment);
    execute_native_v2_cli_with_context(command, &context, &mut NeverDetach, &mut Vec::new())
        .await
        .assert_error()
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
