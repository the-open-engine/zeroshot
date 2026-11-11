use std::time::Duration;

use openengine_cluster_protocol::StackState;
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
    assert!(matches!(error, NativeV2CliError::RunFailed(_)));
    assert_eq!(
        error.to_string(),
        "run finished unsuccessfully\nFull run log: zeroshot logs run-public --target prod"
    );
    assert!(
        String::from_utf8(output)
            .assert_value()
            .contains("\"reason\":\"worker_failed\"")
    );
}

#[tokio::test]
async fn watch_reports_a_terminal_failure_like_the_foreground_run() {
    let backend = FakeBackend::with_failed_watch();
    let mut output = Vec::new();

    let error = execute_watch(&backend, &mut NeverDetach, &mut output)
        .await
        .assert_error();

    assert!(matches!(error, NativeV2CliError::RunFailed(_)));
    assert_eq!(
        error.to_string(),
        "run finished unsuccessfully\nFull run log: zeroshot logs run-public --target prod"
    );
    assert!(
        String::from_utf8(output)
            .assert_value()
            .contains("\"reason\":\"worker_failed\"")
    );
}

#[tokio::test]
async fn foreground_run_failure_names_its_latest_error_logs() {
    let backend = FakeBackend::with_failed_watch_and_error_logs(7);
    let mut output = Vec::new();

    let error = execute_run_task("fail", &backend, &mut NeverDetach, &mut output)
        .await
        .assert_error();

    assert_eq!(
        error.to_string(),
        "run finished unsuccessfully\nLatest 5 of 7 error logs:\n  error 3\n  error 4\n  \
         error 5\n  error 6\n  error 7\nFull run log: zeroshot logs run-public --target prod"
    );
    let logs_cursors = backend
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Logs { from_cursor, .. } => Some(from_cursor),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(logs_cursors, [None, Some("v2:8".to_owned())]);
    let output = String::from_utf8(output).assert_value();
    assert!(output.contains("\"reason\":\"worker_failed\""));
    assert!(!output.contains("error 7"));
}

#[tokio::test]
async fn foreground_run_and_watch_settle_a_watch_closed_before_the_terminal_snapshot() {
    for watch in [false, true] {
        let backend = FakeBackend::with_runtime_failure_before_terminal();
        let mut output = Vec::new();

        let error = if watch {
            execute_watch(&backend, &mut NeverDetach, &mut output).await
        } else {
            execute_run_task("runtime failure", &backend, &mut NeverDetach, &mut output).await
        }
        .assert_error();

        assert!(matches!(error, NativeV2CliError::RunFailed(_)));
        assert!(backend.calls().contains(&Call::Status {
            target: Some("prod".to_owned()),
            run_id: "run-public".to_owned(),
        }));
        let output = String::from_utf8(output).assert_value();
        assert!(output.contains("\"phase\":\"running\""));
        assert!(!output.contains("\"phase\":\"finished\""));
    }
}

#[tokio::test]
async fn ctrl_c_detaches_while_settling_a_watch_closed_before_the_terminal_snapshot() {
    for watch in [false, true] {
        let (notification, _) = tokio::sync::watch::channel(());
        let backend = FakeBackend::with_unanswered_status_after_early_close(notification.clone());
        let mut signal = EdgeDetachSignal { notification };
        let mut output = Vec::new();

        let outcome = tokio::time::timeout(Duration::from_secs(5), async {
            if watch {
                execute_watch(&backend, &mut signal, &mut output).await
            } else {
                execute_run_task("runtime failure", &backend, &mut signal, &mut output).await
            }
        })
        .await
        .assert_value_with("Ctrl-C ends the status read")
        .assert_value();

        assert_eq!(outcome, CliOutcome::Detached);
        let calls = backend.calls();
        assert!(calls.contains(&Call::Status {
            target: Some("prod".to_owned()),
            run_id: "run-public".to_owned(),
        }));
        assert!(
            !calls
                .iter()
                .any(|call| matches!(call, Call::Logs { .. } | Call::Force { .. }))
        );
    }
}

async fn execute_watch<S: DetachSignal>(
    backend: &FakeBackend,
    signal: &mut S,
    output: &mut Vec<u8>,
) -> Result<CliOutcome, NativeV2CliError> {
    let command =
        parse_native_v2_args(args(&["watch", "run-public", "--target", "prod"])).assert_value();
    execute_native_v2_cli(command, backend, signal, output).await
}

#[tokio::test]
async fn named_target_resume_resolves_current_values_for_original_requirements() {
    for checkpoint in [None, Some("entry-2")] {
        for forward in [false, true] {
            assert_resume_resolves_current_values(Some("docker"), checkpoint, forward).await;
        }
    }
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
    for extra in [vec![], vec!["--from-checkpoint", "entry-2"]] {
        let mut values = vec!["resume", "run-failed", "--target", "docker"];
        values.extend(extra);
        let command = parse_native_v2_args(args(&values)).assert_value();
        let accessed = std::sync::Mutex::new(Vec::new());
        let environment = |name: &str| {
            accessed.lock().assert_value().push(name.to_owned());
            Some(OsString::from("must-not-be-read"))
        };
        let error = execute_with_environment(command, &backend, &environment)
            .await
            .assert_error();

        assert!(error.to_string().contains("do not match the original run"));
        assert!(accessed.lock().assert_value().is_empty());
        assert_no_resume_call(&backend);
    }
}

#[tokio::test]
async fn local_resume_resolves_current_values_for_original_requirements() {
    for checkpoint in [None, Some("entry-2")] {
        assert_resume_resolves_current_values(None, checkpoint, false).await;
    }
}

async fn assert_resume_resolves_current_values(
    expected_target: Option<&str>,
    checkpoint: Option<&str>,
    forward: bool,
) {
    let key = openengine_cluster_protocol::ConnectionKey::new("openai").assert_value();
    let field =
        openengine_cluster_protocol::EnvironmentVariableName::new("OPENAI_API_KEY").assert_value();
    let backend = FakeBackend::with_resume_requirements(std::collections::BTreeMap::from([(
        key.clone(),
        vec![field.clone()],
    )]));
    let mut command_args = match expected_target {
        Some(target) => vec!["resume", "run-failed", "--target", target],
        None => vec!["resume", "run-failed"],
    };
    if let Some(id) = checkpoint {
        command_args.extend(["--from-checkpoint", id]);
    }
    // Only a local checkpoint restore replaces the caller's checkout and needs consent.
    let discard = expected_target.is_none() && checkpoint.is_some();
    if discard {
        command_args.push("--discard-workspace-changes");
    }
    if forward {
        command_args.push("--forward-gh-token");
    }
    let command = parse_native_v2_args(args(&command_args)).assert_value();
    let environment = |name: &str| match name {
        "OPENAI_API_KEY" => Some(OsString::from("fresh-openai-token")),
        "GH_TOKEN" => Some(OsString::from("fresh-github-token")),
        _ => None,
    };
    execute_with_environment(command, &backend, &environment)
        .await
        .assert_value();

    let calls = backend.calls();
    let Call::Resume {
        target,
        params,
        discard_workspace_changes,
    } = &calls[1]
    else {
        panic!("resume call was not recorded");
    };
    assert_eq!(target.as_deref(), expected_target);
    assert_eq!(*discard_workspace_changes, discard);
    assert_eq!(
        params.from,
        checkpoint.map(|id| RunResumeFrom::Checkpoint {
            checkpoint_id: CheckpointId::new(id).assert_value(),
        })
    );
    assert_ne!(params.run_id, params.successor_run_id);
    assert!(params.connection_resolver.is_none());
    assert_eq!(
        params.connections[&key]
            .as_map()
            .get(&field)
            .map(String::as_str),
        Some("fresh-openai-token")
    );
    // Local controllers read GH_TOKEN directly; named targets receive it only on opt-in.
    assert_eq!(
        params.github_token.as_deref(),
        (expected_target.is_none() || forward).then_some("fresh-github-token")
    );
}

#[tokio::test]
async fn resume_rejects_an_invalid_github_token_before_submission() {
    let backend = FakeBackend::with_resume_requirements(Default::default());
    let command = parse_native_v2_args(args(&["resume", "run-failed"])).assert_value();
    let oversized = "x".repeat(4_097);
    let environment = |name: &str| (name == "GH_TOKEN").then(|| OsString::from(&oversized));
    let error = execute_with_environment(command, &backend, &environment)
        .await
        .assert_error();

    assert!(matches!(error, NativeV2CliError::GitHubToken));
    assert_no_resume_call(&backend);
}

#[tokio::test]
async fn resume_refusal_for_an_unrecoverable_workspace_is_not_a_target_outage() {
    // The run failed on the target at once, so the target retained no workspace for it.
    let backend = FakeBackend::with_runtime_failure_before_terminal();
    let command =
        parse_native_v2_args(args(&["resume", "run-failed", "--target", "d1"])).assert_value();
    let error = execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut Vec::new())
        .await
        .assert_error();
    let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();

    assert!(
        diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("does not have a recoverable workspace")),
        "{diagnostic}"
    );
    assert_ne!(diagnostic["code"], "target.unavailable", "{diagnostic}");
    assert_ne!(diagnostic["kind"], "target", "{diagnostic}");
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

fn stack_fixture(delivery: &str, runs: Value) -> FixtureFiles {
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
            "schema":"zeroshot.stack/v1",
            "title":"Release",
            "delivery":delivery,
            "source":{"repository":"open-engine/zeroshot","branch":"main"},
            "profile":"org:software-change",
            "expiresAt":expires_at,
            "runs":runs
        }),
    )
}

fn merge_stack_fixture() -> FixtureFiles {
    stack_fixture("merge", json!({"build":{"input":{"task":"fail"}}}))
}

fn stack_submit_command(file: &Path, detach: bool) -> NativeV2CliCommand {
    let mut values = vec![
        OsString::from("stack"),
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

async fn execute_stack(
    command: NativeV2CliCommand,
    backend: &FakeBackend,
) -> (Result<CliOutcome, NativeV2CliError>, String) {
    let environment = |name: &str| (name == "GH_TOKEN").then(|| OsString::from("test-token"));
    let context = CliExecutionContext::new(backend, &environment);
    let mut output = Vec::new();
    let result =
        execute_native_v2_cli_with_context(command, &context, &mut NeverDetach, &mut output).await;
    (result, String::from_utf8(output).assert_value())
}

async fn execute_terminal_stack(
    command: NativeV2CliCommand,
    state: StackState,
) -> (Result<CliOutcome, NativeV2CliError>, String) {
    execute_stack(command, &FakeBackend::with_terminal_stack_state(state)).await
}

#[tokio::test]
async fn foreground_stack_submit_reports_a_terminal_failure_after_printing_it() {
    let files = merge_stack_fixture();
    let command = stack_submit_command(&files.input, false);

    let (result, output) = execute_terminal_stack(command, StackState::Failed).await;

    assert!(matches!(
        result.assert_error(),
        NativeV2CliError::StackFailed
    ));
    assert!(output.contains("\"stackId\":\"stack-public\""));
    assert!(output.contains("\"state\":\"failed\""));
}

#[tokio::test]
async fn stack_watch_reports_each_unsuccessful_terminal_state_after_printing_it() {
    for (state, expected) in [
        (StackState::Failed, "failed"),
        (StackState::Cancelled, "cancelled"),
        (StackState::Expired, "expired"),
    ] {
        let command = parse_native_v2_args(args(&[
            "stack",
            "watch",
            "stack-public",
            "--target",
            "prod",
        ]))
        .assert_value();

        let (result, output) = execute_terminal_stack(command, state).await;

        let error = result.assert_error();
        assert!(matches!(error, NativeV2CliError::StackFailed));
        assert_eq!(error.to_string(), "stack finished unsuccessfully");
        assert!(output.contains(&format!("\"state\":\"{expected}\"")));
    }
}

#[tokio::test]
async fn detached_stack_submit_does_not_project_a_terminal_failure() {
    let files = merge_stack_fixture();
    let command = stack_submit_command(&files.input, true);

    let (result, output) = execute_terminal_stack(command, StackState::Failed).await;

    assert_eq!(result.assert_value(), CliOutcome::Detached);
    assert!(output.contains("\"state\":\"failed\""));
}

#[tokio::test]
async fn stack_status_and_force_stop_keep_ordinary_command_outcomes() {
    for operation in ["status", "force-stop"] {
        let command = parse_native_v2_args(args(&[
            "stack",
            operation,
            "stack-public",
            "--target",
            "prod",
        ]))
        .assert_value();

        let (result, output) = execute_terminal_stack(command, StackState::Expired).await;

        assert_eq!(result.assert_value(), CliOutcome::Completed);
        assert!(output.contains("\"state\":\"expired\""));
    }
}

fn diamond_runs() -> Value {
    json!({
        "base":{"input":{"task":"base"}},
        "right":{"needs":["base"],"input":{"task":"right"}},
        "left":{"needs":["base"],"input":{"task":"left"}},
        "top":{"needs":["right","left"],"input":{"task":"top"}}
    })
}

fn submitted_needs(backend: &FakeBackend) -> Vec<(String, Vec<String>)> {
    let submitted = backend.submitted_stacks();
    assert_eq!(submitted.len(), 1);
    submitted[0]
        .runs
        .iter()
        .map(|run| {
            (
                run.name.as_str().to_owned(),
                run.needs
                    .iter()
                    .map(|need| need.as_str().to_owned())
                    .collect(),
            )
        })
        .collect()
}

#[tokio::test]
async fn stacked_pull_request_submission_sends_one_linear_stack() {
    let files = stack_fixture("stacked_pull_requests", diamond_runs());
    let backend =
        FakeBackend::with_stack_profile(StackState::Succeeded, TemplateDelivery::PullRequest);

    let (result, _) = execute_stack(stack_submit_command(&files.input, true), &backend).await;

    assert_eq!(result.assert_value(), CliOutcome::Detached);
    assert_eq!(
        backend.submitted_stacks()[0].delivery,
        openengine_cluster_protocol::StackDelivery::StackedPullRequests
    );
    let expected: Vec<(String, Vec<String>)> = vec![
        ("base".to_owned(), vec![]),
        ("left".to_owned(), vec!["base".to_owned()]),
        ("right".to_owned(), vec!["left".to_owned()]),
        ("top".to_owned(), vec!["right".to_owned()]),
    ];
    assert_eq!(submitted_needs(&backend), expected);
}

#[tokio::test]
async fn merge_stack_submission_keeps_fan_in() {
    let files = stack_fixture("merge", diamond_runs());
    let backend = FakeBackend::with_stack_profile(StackState::Succeeded, TemplateDelivery::Merge);

    let (result, _) = execute_stack(stack_submit_command(&files.input, true), &backend).await;

    assert_eq!(result.assert_value(), CliOutcome::Detached);
    assert_eq!(
        backend.submitted_stacks()[0].delivery,
        openengine_cluster_protocol::StackDelivery::Merge
    );
    assert_eq!(
        submitted_needs(&backend)[3],
        (
            "top".to_owned(),
            vec!["left".to_owned(), "right".to_owned()]
        )
    );
}

#[tokio::test]
async fn stack_submission_rejects_a_profile_for_the_other_delivery() {
    for (delivery, template) in [
        ("stacked_pull_requests", TemplateDelivery::Merge),
        ("merge", TemplateDelivery::PullRequest),
    ] {
        let files = stack_fixture(delivery, diamond_runs());
        let backend = FakeBackend::with_stack_profile(StackState::Succeeded, template);

        let (result, output) =
            execute_stack(stack_submit_command(&files.input, true), &backend).await;

        let error = result.assert_error().to_string();
        assert!(
            error.contains(&format!("stack delivery \"{delivery}\" requires")),
            "{error}"
        );
        assert!(output.is_empty());
        assert!(backend.submitted_stacks().is_empty());
    }
}

#[tokio::test]
async fn named_target_resume_leaves_gh_token_to_the_target_without_opt_in() {
    let accessed = std::sync::Mutex::new(Vec::new());
    let environment = |field: &str| {
        accessed.lock().assert_value().push(field.to_owned());
        (field == "GH_TOKEN").then(|| OsString::from("ambient-token"))
    };
    // Hosted targets resolve retained connections themselves and return no requirements.
    let backend =
        FakeBackend::with_resume_requirements(Default::default()).with_resume_source(source());
    let command =
        parse_native_v2_args(args(&["resume", "run-failed", "--target", "docker"])).assert_value();
    execute_with_environment(command, &backend, &environment)
        .await
        .assert_value();
    let calls = backend.calls();
    let [_, Call::Resume { params, .. }] = calls.as_slice() else {
        panic!("resume call was not recorded");
    };
    assert!(params.connections.is_empty());
    assert_eq!(params.github_token, None);
    assert!(accessed.lock().assert_value().is_empty());
}

#[tokio::test]
async fn direct_resume_needing_gh_token_points_to_the_opt_in() {
    let backend = FakeBackend::with_resume_requirements(std::collections::BTreeMap::from([(
        openengine_cluster_protocol::ConnectionKey::new("github").assert_value(),
        vec![openengine_cluster_protocol::EnvironmentVariableName::new("GH_TOKEN").assert_value()],
    )]))
    .with_resume_source(source());
    let command =
        parse_native_v2_args(args(&["resume", "run-failed", "--target", "docker"])).assert_value();
    let error = execute_with_environment(command, &backend, &|_: &str| {
        Some(OsString::from("ambient-token"))
    })
    .await
    .assert_error();

    assert!(
        matches!(&error, NativeV2CliError::Usage(message) if message.contains("--forward-gh-token")),
        "{error}"
    );
    assert_no_resume_call(&backend);
}

#[tokio::test]
async fn stack_submission_sends_the_shell_gh_token_only_on_opt_in() {
    for forward in [false, true] {
        let files = stack_fixture("merge", diamond_runs());
        let backend =
            FakeBackend::with_stack_profile(StackState::Succeeded, TemplateDelivery::Merge);
        let mut command = stack_submit_command(&files.input, true);
        if let NativeV2CliCommand::StackSubmit(submit) = &mut command {
            submit.forward_github_token = forward;
        }

        let (result, _) = execute_stack(command, &backend).await;

        assert_eq!(result.assert_value(), CliOutcome::Detached);
        let submitted = &backend.submitted_stacks()[0];
        assert_eq!(
            submitted.github_token.as_deref(),
            forward.then_some("test-token")
        );
        let sent_in_connections = submitted
            .connections
            .values()
            .any(|values| values.as_map().values().any(|value| value == "test-token"));
        assert_eq!(
            sent_in_connections,
            forward && !submitted.connections.is_empty()
        );
    }
}

#[tokio::test]
async fn named_target_resume_opt_in_fails_without_gh_token_for_every_source() {
    for retained in [
        source(),
        forge_resume_source("source.github", "https://ghe.example.test"),
        forge_resume_source("source.gitlab", "https://gitlab.example.test"),
    ] {
        let backend =
            FakeBackend::with_resume_requirements(Default::default()).with_resume_source(retained);
        let command = parse_native_v2_args(args(&[
            "resume",
            "run-failed",
            "--target",
            "docker",
            "--forward-gh-token",
        ]))
        .assert_value();
        let error = execute_with_environment(command, &backend, &|_: &str| None)
            .await
            .assert_error();

        assert!(
            matches!(&error, NativeV2CliError::Usage(message) if message.contains("GH_TOKEN")),
            "{error}"
        );
        assert_no_resume_call(&backend);
    }
}

#[test]
fn forward_gh_token_requires_a_named_target() {
    for arguments in [
        &["resume", "run-failed", "--forward-gh-token"][..],
        &[
            "run",
            "--title",
            "Local",
            "--input",
            "input.json",
            "--forward-gh-token",
        ][..],
    ] {
        assert!(parse_native_v2_args(args(arguments)).is_err());
    }
}

fn forge_resume_source(provider: &str, base: &str) -> Value {
    let mut retained = source();
    retained["remote"] = json!({
        "url":format!("{base}/open-engine/zeroshot.git"),
        "forge":{
            "provider":provider,
            "repository":"open-engine/zeroshot",
            "baseUrl":base,
        },
    });
    retained
}

#[tokio::test]
async fn resume_keeps_local_trusted_authority_and_scopes_named_github_fallback() {
    let key = openengine_cluster_protocol::ConnectionKey::new("company-forge").assert_value();
    for (retained, field, fallback, target) in [
        (
            forge_resume_source("source.github", "https://ghe.example.test"),
            Some("GH_TOKEN"),
            false,
            Some("docker"),
        ),
        (
            forge_resume_source("source.github", "https://company.ghe.com"),
            Some("GH_TOKEN"),
            false,
            Some("docker"),
        ),
        (
            forge_resume_source("source.gitlab", "https://gitlab.example.test"),
            Some("GITLAB_TOKEN"),
            false,
            Some("docker"),
        ),
        (
            forge_resume_source("source.github", "https://github.com"),
            Some("GH_TOKEN"),
            true,
            Some("docker"),
        ),
        (source(), Some("GH_TOKEN"), true, Some("docker")),
        (Value::Null, Some("GITLAB_TOKEN"), false, Some("docker")),
        (Value::Null, None, true, None),
    ] {
        let name = field.map(|field| {
            openengine_cluster_protocol::EnvironmentVariableName::new(field).assert_value()
        });
        let requirements = name
            .iter()
            .map(|name| (key.clone(), vec![name.clone()]))
            .collect();
        let backend =
            FakeBackend::with_resume_requirements(requirements).with_resume_source(retained);
        let mut arguments = vec!["resume", "run-failed"];
        if let Some(target) = target {
            arguments.extend(["--target", target, "--forward-gh-token"]);
        }
        let command = parse_native_v2_args(args(&arguments)).assert_value();
        let accessed = std::sync::Mutex::new(Vec::new());
        let environment = |field: &str| {
            accessed.lock().assert_value().push(field.to_owned());
            match field {
                "GH_TOKEN" => Some(OsString::from("github-connection-token")),
                "GITLAB_TOKEN" => Some(OsString::from("gitlab-connection-token")),
                _ => None,
            }
        };
        execute_with_environment(command, &backend, &environment)
            .await
            .assert_value();
        let calls = backend.calls();
        let [
            _,
            Call::Resume {
                target: actual_target,
                params,
                ..
            },
        ] = calls.as_slice()
        else {
            panic!("resume call was not recorded");
        };
        assert_eq!(actual_target.as_deref(), target);
        if let Some(name) = &name {
            let expected_token = if field == Some("GH_TOKEN") {
                "github-connection-token"
            } else {
                "gitlab-connection-token"
            };
            assert_eq!(params.connections[&key].as_map()[name], expected_token);
        } else {
            assert!(params.connections.is_empty());
        }
        assert_eq!(
            params.github_token.as_deref(),
            fallback.then_some("github-connection-token")
        );
        let mut expected_reads = field
            .map(|field| vec![field.to_owned()])
            .unwrap_or_default();
        // Named targets pass --forward-gh-token, which always checks GH_TOKEN.
        if fallback || target.is_some() {
            expected_reads.push("GH_TOKEN".to_owned());
        }
        assert_eq!(*accessed.lock().assert_value(), expected_reads);
    }
}
