use super::*;
use crate::native_v2_cli::execution::{CliExecutionContext, execute_native_v2_cli_with_context};

#[test]
fn review_url_input_and_default_comment_delivery_are_unambiguous() {
    let command = parse_native_v2_args(args(&[
        "run",
        "--title",
        "Review",
        "--task-from",
        "https://github.com/acme/project/pull/42",
        "--template",
        "issue-review",
        "--uniform-runtime-config",
        "runtime.json",
    ]))
    .assert_value();
    let NativeV2CliCommand::Run(command) = command else {
        panic!("expected run");
    };
    assert_eq!(
        command.input,
        RunInput::TaskFrom("https://github.com/acme/project/pull/42".into())
    );
    for extra in [
        vec!["--input", "input.json"],
        vec!["--repository", "acme/project"],
        vec!["--source-connection", "source"],
    ] {
        let mut flags = vec![
            "run",
            "--title",
            "Review",
            "--target",
            "cloud",
            "--task-from",
            "https://github.com/acme/project/issues/42",
        ];
        flags.extend(extra);
        assert!(parse_native_v2_args(args(&flags)).is_err());
    }
    assert!(
        parse_native_v2_args(args(&[
            "template",
            "show",
            "software-change",
            "--delivery",
            "comment"
        ]))
        .is_err()
    );
    assert!(parse_native_v2_args(args(&["template", "show", "issue-review", "--pr"])).is_err());
}

#[test]
fn review_defaults_to_comment_for_run_profile_creation_and_template_export() {
    for (overrides, expected) in [
        (vec![], TemplateDelivery::Comment),
        (vec!["--delivery", "comment"], TemplateDelivery::Comment),
        (vec!["--delivery", "none"], TemplateDelivery::None),
    ] {
        assert_template_delivery("issue-review", &overrides, expected);
        if expected != TemplateDelivery::None {
            assert_template_delivery("pr-review", &overrides, expected);
        }
    }
    for template in ["single-worker", "auto-research"] {
        assert!(matches!(
            parse_native_v2_args(args(&["template", "show", template])).assert_value(),
            NativeV2CliCommand::TemplateShow {
                delivery: TemplateDelivery::None,
                ..
            }
        ));
    }
}

#[test]
fn software_change_defaults_to_pr_and_keeps_all_explicit_delivery_modes() {
    for (overrides, expected) in [
        (vec![], TemplateDelivery::PullRequest),
        (vec!["--pr"], TemplateDelivery::PullRequest),
        (
            vec!["--delivery", "pull_request"],
            TemplateDelivery::PullRequest,
        ),
        (vec!["--delivery", "none"], TemplateDelivery::None),
        (vec!["--push"], TemplateDelivery::Push),
        (vec!["--delivery", "push"], TemplateDelivery::Push),
        (vec!["--ship"], TemplateDelivery::Merge),
        (vec!["--delivery", "merge"], TemplateDelivery::Merge),
    ] {
        assert_template_delivery("software-change", &overrides, expected);
    }
}

fn assert_template_delivery(template: &str, overrides: &[&str], expected: TemplateDelivery) {
    for mut flags in [
        vec!["template", "show", template],
        vec![
            "run",
            "--title",
            "Change",
            "--input",
            "input.json",
            "--template",
            template,
            "--uniform-runtime-config",
            "runtime.json",
        ],
        vec![
            "profile",
            "set",
            "example",
            "--template",
            template,
            "--uniform-runtime-config",
            "runtime.json",
        ],
    ] {
        flags.extend(overrides.iter().copied());
        let actual = match parse_native_v2_args(args(&flags)).assert_value() {
            NativeV2CliCommand::TemplateShow { delivery, .. } => delivery,
            NativeV2CliCommand::Run(command) => {
                let RunSelection::Inline { graph, .. } = command.selection else {
                    panic!("expected inline template");
                };
                graph_delivery(graph)
            }
            NativeV2CliCommand::ProfileSet(command) => graph_delivery(command.graph),
            _ => panic!("expected template selection"),
        };
        assert_eq!(actual, expected, "{flags:?}");
    }
}

fn graph_delivery(graph: RunGraph) -> TemplateDelivery {
    let RunGraph::Template { delivery, .. } = graph else {
        panic!("expected template graph");
    };
    delivery
}

fn remote_review_command(directory: &std::path::Path) -> NativeV2CliCommand {
    let runtime = directory.join("runtime.json");
    std::fs::write(
        &runtime,
        json!({"harness":"codex","provider":"openai","size":"small","model":"opaque-model"})
            .to_string(),
    )
    .assert_value();
    parse_native_v2_args(args(&[
        "run",
        "--title",
        "Review",
        "--target",
        "cloud",
        "--cwd",
        directory.to_str().assert_value(),
        "--task-from",
        "https://github.com/acme/project/issues/42",
        "--template",
        "issue-review",
        "--uniform-runtime-config",
        runtime.to_str().assert_value(),
        "--detach",
    ]))
    .assert_value()
}

#[tokio::test]
async fn remote_review_resolves_context_and_source_without_a_client_git_checkout() {
    let directory = tempfile::tempdir().assert_value();
    let command = remote_review_command(directory.path());
    let prepared = prepared_review_fixture();
    let backend = FakeBackend::with_review(prepared.clone());
    let environment = |name: &str| {
        matches!(name, "OPENAI_API_KEY" | "GH_TOKEN")
            .then(|| OsString::from("test-only-credential"))
    };
    let mut output = Vec::new();
    let outcome = execute_native_v2_cli_with_context(
        command,
        &CliExecutionContext::new(&backend, &environment),
        &mut NeverDetach,
        &mut output,
    )
    .await
    .assert_value();
    assert_eq!(outcome, CliOutcome::Detached);
    assert!(!output.is_empty());
    let calls = backend.calls();
    assert!(matches!(&calls[0],Call::ResolveReview {target:Some(target),..} if target=="cloud"));
    let Call::Submit { input, runtime, .. } = &calls[1] else {
        panic!("expected prepared submission");
    };
    assert_eq!(input, &prepared.initial_input);
    assert!(matches!(
        runtime
            .nodes()
            .get(&openengine_cluster_protocol::NodeName::new("deliver").assert_value()),
        Some(openengine_cluster_protocol::NodeRuntimeBinding::CommentDelivery { .. })
    ));
}

#[tokio::test]
async fn review_submission_reuses_environment_captured_before_metadata_resolution() {
    let directory = tempfile::tempdir().assert_value();
    let path = directory.path().join("environment.json");
    let expected: openengine_cluster_protocol::RuntimeEnvironment =
        serde_json::from_value(json!({"variables":{"MODE":"captured"}})).assert_value();
    std::fs::write(&path, serde_json::to_vec(&expected).assert_value()).assert_value();
    let mut command = remote_review_command(directory.path());
    let NativeV2CliCommand::Run(run) = &mut command else {
        panic!("expected run");
    };
    run.environment = Some(RunEnvironmentInput::File(path.clone()));
    let changed = json!({"variables":{"MODE":"changed-during-resolution"}});
    let backend = FakeBackend::with_review(prepared_review_fixture())
        .with_review_file_change(path.clone(), changed.to_string());
    let environment = |name: &str| {
        matches!(name, "OPENAI_API_KEY" | "GH_TOKEN")
            .then(|| OsString::from("test-only-credential"))
    };
    execute_native_v2_cli_with_context(
        command,
        &CliExecutionContext::new(&backend, &environment),
        &mut NeverDetach,
        &mut Vec::new(),
    )
    .await
    .assert_value();
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(path).assert_value()).assert_value(),
        changed
    );
    let calls = backend.calls();
    let Call::Submit { environment, .. } = &calls[1] else {
        panic!("expected prepared submission");
    };
    assert_eq!(environment, &Some(expected));
}

#[tokio::test]
async fn task_from_accepts_a_custom_profile_requiring_complete_pr_scope() {
    let directory = tempfile::tempdir().assert_value();
    let runtime = directory.path().join("runtime.json");
    std::fs::write(
        &runtime,
        json!({"harness":"codex","provider":"openai","model":"opaque-model"}).to_string(),
    )
    .assert_value();
    let mut graph = serde_json::to_value(
        BuiltinGraphTemplate::IssueReview
            .materialize(TemplateDelivery::Comment)
            .assert_value(),
    )
    .assert_value();
    for field in graph["initialInput"]["fields"]["context"]["type"]["fields"]["reviewScope"]["type"]
        ["fields"]
        .as_object_mut()
        .assert_value()
        .values_mut()
    {
        field["required"] = json!(true);
    }
    let graph_path = directory.path().join("graph.json");
    std::fs::write(&graph_path, graph.to_string()).assert_value();
    let command = parse_native_v2_args(args(&[
        "run",
        "--title",
        "Review",
        "--target",
        "cloud",
        "--task-from",
        "https://github.com/acme/project/pull/42",
        "--graph",
        graph_path.to_str().assert_value(),
        "--uniform-runtime-config",
        runtime.to_str().assert_value(),
        "--detach",
    ]))
    .assert_value();
    let mut prepared = prepared_review_fixture();
    prepared.initial_input["url"] = json!("https://github.com/acme/project/pull/42");
    prepared.initial_input["context"]["reviewScope"] = json!({
        "baseRepository":"acme/project", "baseBranch":"main", "baseRevision":"b".repeat(40),
        "headRepository":"acme/project", "headBranch":"feature", "headRevision":"a".repeat(40),
    });
    let backend = FakeBackend::with_review(prepared.clone());
    let environment = |name: &str| {
        matches!(name, "OPENAI_API_KEY" | "GH_TOKEN")
            .then(|| OsString::from("test-only-credential"))
    };
    let mut output = Vec::new();
    execute_native_v2_cli_with_context(
        command,
        &CliExecutionContext::new(&backend, &environment),
        &mut NeverDetach,
        &mut output,
    )
    .await
    .assert_value();
    assert!(
        backend
            .calls()
            .iter()
            .any(|call| matches!(call,Call::Submit {input,..} if input==&prepared.initial_input))
    );
}

#[test]
fn pr_review_requires_comment_delivery_for_exact_comparison_preparation() {
    assert!(
        parse_native_v2_args(args(&[
            "template",
            "show",
            "pr-review",
            "--delivery",
            "none"
        ]))
        .is_err()
    );
}
