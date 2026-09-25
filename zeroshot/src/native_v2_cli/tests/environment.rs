use std::ffi::OsString;

use openengine_cluster_protocol::RuntimePlan;
use openengine_cluster_testkit::assertions::AssertValue;
#[cfg(unix)]
use openengine_cluster_testkit::assertions::AssertError;
use serde_json::json;

use super::*;
use crate::native_v2_cli::execution::{CliExecutionContext, execute_native_v2_cli_with_context};

fn runtime_with_environment() -> RuntimePlan {
    serde_json::from_value(json!({
        "harness":"codex",
        "provider":"openai",
        "size":"medium",
        "nodes":{
            "worker":{"kind":"agent","model":"gpt-5.6-sol","connections":{"test":["DECLARED","SHARED"]}}
        }
    }))
    .assert_value()
}

fn environment_graph() -> serde_json::Value {
    serde_json::to_value(
        BuiltinGraphTemplate::SingleWorker
            .materialize(TemplateDelivery::None)
            .assert_value(),
    )
    .assert_value()
}

fn software_change_runtime() -> RuntimePlan {
    serde_json::from_value(json!({
        "harness":"codex",
        "provider":"openai",
        "size":"medium",
        "nodes":{
            "worker":{"kind":"agent","model":"gpt-5.6-sol","effort":"max"},
            "acceptance":{"kind":"agent","model":"gpt-5.6-sol","effort":"max"},
            "code":{"kind":"agent","model":"gpt-5.6-sol","effort":"max"},
            "review_repair":{"kind":"agent","model":"gpt-5.6-sol","effort":"max"},
            "delivery_repair":{"kind":"agent","model":"gpt-5.6-sol","effort":"max"}
        }
    }))
    .assert_value()
}

fn environment_command(extra: &[&str]) -> (FixtureFiles, NativeV2CliCommand) {
    let files = FixtureFiles::with_runtime(
        environment_graph(),
        json!({"task":"ship it"}),
        runtime_with_environment(),
    );
    let command = parse_native_v2_args(run_args(&files.graph, &files.input, &files.runtime, extra))
        .assert_value();
    (files, command)
}

async fn execute_with_environment(
    command: NativeV2CliCommand,
    backend: &FakeBackend,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Result<CliOutcome, NativeV2CliError> {
    let context = CliExecutionContext::new(backend, environment);
    execute_native_v2_cli_with_context(command, &context, &mut NeverDetach, &mut Vec::new()).await
}

#[cfg(unix)]
async fn assert_declared_environment_rejected(
    command: NativeV2CliCommand,
    backend: &FakeBackend,
    environment: &dyn Fn(&str) -> Option<OsString>,
) {
    let error = execute_with_environment(command, backend, environment)
        .await
        .assert_error();
    assert!(error.to_string().contains("DECLARED"));
    assert!(backend.calls().is_empty());
}

#[tokio::test]
async fn template_run_preserves_authored_input_and_owned_delivery_binding() {
    let files = FixtureFiles::with_runtime(
        graph(),
        json!({"task":"ship it","issueNumber":"208"}),
        software_change_runtime(),
    );
    let command = parse_native_v2_args(args(&[
        "run",
        "--target",
        "prod",
        "--repository",
        "open-engine/zeroshot",
        "--branch",
        "main",
        "--revision",
        "0123456789abcdef0123456789abcdef01234567",
        "--title",
        "Ship change",
        "--template",
        "software-change",
        "--ship",
        "--no-pr-feedback",
        "--input",
        files.input.to_str().assert_value(),
        "--runtime-config",
        files.runtime.to_str().assert_value(),
        "--submission-key",
        "template-key",
        "-d",
    ]))
    .assert_value();
    let backend = FakeBackend::default();
    let available = |name: &str| (name == "GH_TOKEN").then(|| OsString::from("template-secret"));
    execute_with_environment(command, &backend, &available)
        .await
        .assert_value();
    let calls = backend.calls();
    let submitted = match calls.as_slice() {
        [
            Call::Submit {
                runtime,
                input,
                connections,
                ..
            },
        ] => Some((runtime, input, connections)),
        _ => None,
    }
    .assert_value();
    assert_eq!(submitted.1.pointer("/task"), Some(&json!("ship it")));
    assert_eq!(submitted.1.pointer("/issueNumber"), Some(&json!("208")));
    assert!(submitted.1.pointer("/acceptanceFeedback").is_none());
    assert!(submitted.1.pointer("/codeFeedback").is_none());
    assert!(submitted.1.pointer("/deliveryFeedback").is_none());
    let authored_input = std::fs::read(&files.input).assert_value();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&authored_input).assert_value(),
        json!({"task":"ship it","issueNumber":"208"})
    );

    let runtime = serde_json::to_value(submitted.0).assert_value();
    assert_eq!(
        runtime.pointer("/nodes/deliver/kind"),
        Some(&json!("git_delivery"))
    );
    assert_eq!(
        runtime.pointer("/nodes/deliver/connections/github/0"),
        Some(&json!("GH_TOKEN"))
    );
    assert_eq!(
        runtime.pointer("/nodes/deliver/pullRequestFeedback"),
        Some(&json!("ignore"))
    );
    assert_eq!(
        submitted
            .2
            .get(&ConnectionKey::new("github").assert_value())
            .and_then(|values| values.as_map().iter().next())
            .map(|(name, value)| (name.as_str(), value.as_str())),
        Some(("GH_TOKEN", "template-secret"))
    );
}

#[tokio::test]
async fn run_collects_only_the_distinct_declared_environment_before_submission() {
    let (_files, command) = environment_command(&["--submission-key", "environment-key", "-d"]);
    let requested = std::cell::RefCell::new(Vec::new());
    let backend = FakeBackend::default();
    let available = |name: &str| {
        requested.borrow_mut().push(name.to_owned());
        Some(OsString::from(format!("value-for-{name}")))
    };
    execute_with_environment(command, &backend, &available)
        .await
        .assert_value();

    assert_eq!(
        requested.into_inner(),
        ["OPENAI_API_KEY", "DECLARED", "SHARED", "GH_TOKEN"]
    );
    let calls = backend.calls();
    let (connections, github_token) = match calls.as_slice() {
        [
            Call::Submit {
                connections,
                github_token,
                ..
            },
        ] => Some((connections, github_token)),
        _ => None,
    }
    .assert_value();
    assert_eq!(github_token.as_deref(), Some("value-for-GH_TOKEN"));
    assert_eq!(
        connections
            .get(&ConnectionKey::new("test").assert_value())
            .assert_value()
            .as_map()
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect::<Vec<_>>(),
        [
            ("DECLARED", "value-for-DECLARED"),
            ("SHARED", "value-for-SHARED")
        ]
    );
}

#[tokio::test]
async fn missing_inline_environment_is_left_for_connection_resolution() {
    let (_files, command) = environment_command(&["--submission-key", "missing-environment", "-d"]);
    let backend = FakeBackend::default();
    let available = |_: &str| None;
    execute_with_environment(command, &backend, &available)
        .await
        .assert_value();
    let calls = backend.calls();
    assert!(matches!(
        calls.as_slice(),
        [Call::Submit { connections, .. }] if connections.is_empty()
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_declared_environment_fails_before_backend_contact() {
    use std::os::unix::ffi::OsStringExt as _;

    let (_files, command) =
        environment_command(&["--submission-key", "non-utf8-environment", "-d"]);
    let backend = FakeBackend::default();
    let available = |name: &str| {
        if name == "OPENAI_API_KEY" {
            Some(OsString::from("provider-secret"))
        } else {
            Some(OsString::from_vec(vec![0xff]))
        }
    };
    assert_declared_environment_rejected(command, &backend, &available).await;
}

fn uniform_runtime_command(title: &str, runtime: Value) -> (FixtureFiles, NativeV2CliCommand) {
    uniform_runtime_command_for_placement(title, runtime, true)
}

fn local_uniform_runtime_command(
    title: &str,
    runtime: Value,
) -> (FixtureFiles, NativeV2CliCommand) {
    uniform_runtime_command_for_placement(title, runtime, false)
}

fn uniform_runtime_command_for_placement(
    title: &str,
    runtime: Value,
    contained: bool,
) -> (FixtureFiles, NativeV2CliCommand) {
    let files = FixtureFiles::new(graph(), json!({"task":"inspect it"}));
    std::fs::write(&files.runtime, serde_json::to_vec(&runtime).assert_value()).assert_value();
    let mut values = vec![OsString::from("run")];
    if contained {
        values.extend(
            [
                "--target",
                "prod",
                "--repository",
                "open-engine/zeroshot",
                "--branch",
                "main",
                "--revision",
                "0123456789abcdef0123456789abcdef01234567",
            ]
            .map(OsString::from),
        );
    }
    values.extend([
        OsString::from("--title"),
        OsString::from(title),
        OsString::from("--template"),
        OsString::from("single-worker"),
        OsString::from("--input"),
        files.input.as_os_str().to_owned(),
        OsString::from("--uniform-runtime-config"),
        files.runtime.as_os_str().to_owned(),
        OsString::from("-d"),
    ]);
    let command = parse_native_v2_args(values).assert_value();
    (files, command)
}

async fn contained_uniform_submission(
    title: &str,
    runtime: Value,
    available: &dyn Fn(&str) -> Option<OsString>,
) -> (Value, Value) {
    let (_files, command) = uniform_runtime_command(title, runtime);
    let backend = FakeBackend::default();
    execute_with_environment(command, &backend, available)
        .await
        .assert_value();
    let calls = backend.calls();
    let (runtime, connections) = match calls.as_slice() {
        [
            Call::Submit {
                runtime,
                connections,
                ..
            },
        ] => Some((runtime, connections)),
        _ => None,
    }
    .assert_value();
    (
        serde_json::to_value(runtime).assert_value(),
        serde_json::to_value(connections).assert_value(),
    )
}

#[tokio::test]
async fn local_native_harnesses_do_not_require_invented_provider_keys() {
    for (harness, provider) in [
        ("codex", "openai"),
        ("claude", "anthropic"),
        ("copilot", "github"),
    ] {
        let (_files, command) = local_uniform_runtime_command(
            &format!("Local {harness} runtime"),
            json!({
                "harness":harness,
                "provider":provider,
                "model":"provider-owned-model"
            }),
        );
        let backend = FakeBackend::default();
        let requested = std::cell::RefCell::new(Vec::new());
        let available = |name: &str| {
            requested.borrow_mut().push(name.to_owned());
            Some(OsString::from("must-not-be-requested"))
        };
        execute_with_environment(command, &backend, &available)
            .await
            .assert_value();

        assert!(requested.into_inner().is_empty());
        let calls = backend.calls();
        let (target, runtime, connections) = match calls.as_slice() {
            [
                Call::Submit {
                    target,
                    runtime,
                    connections,
                    ..
                },
            ] => Some((target, runtime, connections)),
            _ => None,
        }
        .assert_value();
        assert!(target.is_none());
        assert!(connections.is_empty());
        assert!(runtime.connection_requirements().is_empty());
    }
}

#[tokio::test]
async fn contained_native_harnesses_receive_canonical_provider_requirements() {
    for (harness, provider, connection, field) in [
        ("codex", "openai", "openai", "OPENAI_API_KEY"),
        ("claude", "anthropic", "anthropic", "ANTHROPIC_API_KEY"),
        ("copilot", "github", "github", "COPILOT_GITHUB_TOKEN"),
    ] {
        let available = |name: &str| (name == field).then(|| OsString::from("provider-secret"));
        let (runtime, connections) = contained_uniform_submission(
            &format!("Contained {harness} runtime"),
            json!({
                "harness":harness,
                "provider":provider,
                "model":"provider-owned-model"
            }),
            &available,
        )
        .await;
        assert_eq!(
            runtime["nodes"]["worker"]["connections"][connection],
            json!([field])
        );
        assert_eq!(connections[connection][field], "provider-secret");
    }
}

#[tokio::test]
async fn uniform_runtime_is_materialized_by_rust_against_the_selected_graph() {
    let (_files, command) = uniform_runtime_command(
        "Uniform runtime",
        json!({
            "harness":"codex",
            "provider":"openrouter",
            "model":"openai/provider-owned-model",
            "effort":"max"
        }),
    );
    let backend = FakeBackend::default();
    let available =
        |name: &str| (name == "OPENROUTER_API_KEY").then(|| OsString::from("provider-secret"));
    execute_with_environment(command, &backend, &available)
        .await
        .assert_value();

    let calls = backend.calls();
    let runtime = match calls.as_slice() {
        [Call::Submit { runtime, .. }] => Some(runtime),
        _ => None,
    }
    .assert_value();
    let runtime = serde_json::to_value(runtime).assert_value();
    assert_eq!(
        runtime.pointer("/nodes/worker/model"),
        Some(&json!("openai/provider-owned-model"))
    );
    assert_eq!(
        runtime.pointer("/nodes/worker/connections/openrouter/0"),
        Some(&json!("OPENROUTER_API_KEY"))
    );
    assert_eq!(runtime.pointer("/size"), Some(&json!("medium")));
}

#[tokio::test]
async fn uniform_gateway_and_bedrock_runtime_materializes_for_both_harnesses_with_exact_defaults() {
    for (provider, first, first_value, second, second_value) in [
        (
            "bedrock",
            "AWS_BEARER_TOKEN_BEDROCK",
            "bedrock-secret",
            "AWS_REGION",
            "us-east-1",
        ),
        (
            "gateway",
            "GATEWAY_API_KEY",
            "gateway-secret",
            "GATEWAY_BASE_URL",
            "https://gateway.example/api/v1",
        ),
    ] {
        for harness in ["codex", "claude"] {
            let available = |name: &str| {
                [(first, first_value), (second, second_value)]
                    .into_iter()
                    .find(|(field, _)| *field == name)
                    .map(|(_, value)| OsString::from(value))
            };
            let (runtime, connections) = contained_uniform_submission(
                &format!("Uniform {provider} {harness} runtime"),
                json!({
                    "harness":harness,
                    "provider":provider,
                    "model":"provider-owned-model"
                }),
                &available,
            )
            .await;
            assert_eq!(runtime.pointer("/harness"), Some(&json!(harness)));
            assert_eq!(runtime.pointer("/provider"), Some(&json!(provider)));
            assert_eq!(
                runtime.pointer(&format!("/nodes/worker/connections/{provider}")),
                Some(&json!([first, second]))
            );
            assert_eq!(
                connections,
                json!({
                    provider: { first: first_value, second: second_value }
                })
            );
        }
    }
}

#[tokio::test]
async fn uniform_runtime_requires_harness_without_contacting_backend() {
    let (_files, command) = uniform_runtime_command(
        "Explicit harness",
        json!({
            "provider":"openrouter",
            "model":"openai/provider-owned-model"
        }),
    );
    let backend = FakeBackend::default();
    let error = rejected_without_backend_contact(command, &backend).await;
    assert!(error.to_string().contains("missing field `harness`"));
}

#[tokio::test]
async fn uniform_runtime_rejects_only_known_incompatible_pair_without_contacting_backend() {
    let (_files, command) = uniform_runtime_command(
        "Incompatible pair",
        json!({
            "harness":"claude",
            "provider":"openai",
            "model":"provider-owned-model"
        }),
    );
    let backend = FakeBackend::default();
    let error = rejected_without_backend_contact(command, &backend).await;
    assert!(error.to_string().contains("incompatible"));
}

#[tokio::test]
async fn validation_only_reports_graph_and_runtime_causes_without_backend_contact() {
    let mut invalid = graph();
    invalid["root"]["output"] = json!({
        "kind":"record",
        "fields":{"issueNumber":{"type":{"kind":"string"},"required":true}}
    });
    for (graph, expected) in [
        (
            environment_graph(),
            "run validation failed: runtime plan has no binding for executable node worker",
        ),
        (
            invalid,
            concat!(
                "run validation failed: graph verification rejected the graph: ",
                "required payload target issueNumber is not defined by a binding"
            ),
        ),
    ] {
        let files = FixtureFiles::new(graph, json!({"task":"inspect it"}));
        let command = parse_native_v2_args(run_args(
            &files.graph,
            &files.input,
            &files.runtime,
            &["--validate-only"],
        ))
        .assert_value();
        let backend = FakeBackend::default();
        let error = rejected_without_backend_contact(command, &backend).await;
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test]
async fn local_copilot_uniform_runtime_preserves_model_without_inventing_a_token() {
    let (_files, command) = local_uniform_runtime_command(
        "Copilot runtime",
        json!({
            "harness":"copilot", "provider":"github", "model":"opaque-future-model",
        }),
    );
    let backend = FakeBackend::default();
    let available = |_name: &str| None;
    execute_with_environment(command, &backend, &available)
        .await
        .assert_value();
    let calls = backend.calls();
    let runtime = match calls.as_slice() {
        [Call::Submit { runtime, .. }] => Some(runtime),
        _ => None,
    }
    .assert_value();
    let runtime = serde_json::to_value(runtime).assert_value();
    assert_eq!(runtime["harness"], "copilot");
    assert_eq!(runtime["provider"], "github");
    assert_eq!(runtime["nodes"]["worker"]["model"], "opaque-future-model");
    assert!(runtime["nodes"]["worker"]["connections"].is_null());
}

#[tokio::test]
async fn explicit_run_environment_is_independent_of_inline_or_named_profile_selection() {
    let directory = tempfile::tempdir().assert_value();
    let path = directory.path().join("environment.json");
    let definition = json!({"setup":"echo install", "startup":"npm ci", "variables":{"CI":"true"},
        "connections":{"registry":["NPM_TOKEN"]}});
    std::fs::write(&path, serde_json::to_vec(&definition).assert_value()).assert_value();
    for named in [false, true] {
        let (_files, mut command) =
            environment_command(&["--environment", path.to_str().unwrap(), "-d"]);
        if named {
            let NativeV2CliCommand::Run(run) = &mut command else {
                unreachable!()
            };
            run.selection = RunSelection::Profile(Some(ProfileReference {
                qualifier: Some(ProfileQualifier::Org),
                name: openengine_cluster_protocol::RunProfileName::new("alpha").assert_value(),
            }));
        }
        let backend = FakeBackend::default();
        let available =
            |name: &str| (name == "NPM_TOKEN").then(|| OsString::from("package-secret"));
        execute_with_environment(command, &backend, &available)
            .await
            .assert_value();
        let calls = backend.calls();
        let (environment, connections) = calls
            .iter()
            .find_map(|call| match call {
                Call::Submit {
                    environment,
                    connections,
                    ..
                } => Some((environment, connections)),
                _ => None,
            })
            .assert_value();
        assert_eq!(serde_json::to_value(environment).assert_value(), definition);
        let token = connections
            .get(&ConnectionKey::new("registry").assert_value())
            .assert_value();
        assert_eq!(
            token.as_map().values().collect::<Vec<_>>(),
            vec!["package-secret"]
        );
    }
}

#[tokio::test]
async fn no_environment_is_an_explicit_empty_override_and_omission_stays_unselected() {
    for flag in [false, true] {
        let extra: &[&str] = if flag {
            &["--no-environment", "-d"]
        } else {
            &["-d"]
        };
        let (_files, command) = environment_command(extra);
        let backend = FakeBackend::default();
        execute_with_environment(command, &backend, &|_| None)
            .await
            .assert_value();
        let calls = backend.calls();
        let Some(Call::Submit { environment, .. }) = calls.last() else {
            panic!("expected submit")
        };
        assert_eq!(environment.as_ref(), flag.then_some(&Default::default()));
    }
    let files = FixtureFiles::with_runtime(
        environment_graph(),
        json!({"task":"ship it"}),
        runtime_with_environment(),
    );
    assert!(
        parse_native_v2_args(run_args(
            &files.graph,
            &files.input,
            &files.runtime,
            &["--environment", "environment.json", "--no-environment"]
        ))
        .is_err()
    );
}
