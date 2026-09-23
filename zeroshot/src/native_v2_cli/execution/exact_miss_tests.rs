use openengine_cluster_protocol::{
    ConnectionListRequest, ConnectionScope, RunId, SubscriptionCloseReason,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;
use crate::native_v2_cli::tests::support::{Call, FakeBackend};
use crate::native_v2_cli::{NeverDetach, parse_native_v2_args};

fn args(values: &[&str]) -> Vec<std::ffi::OsString> {
    values.iter().map(std::ffi::OsString::from).collect()
}

async fn assert_private_dispatch_refusals(backend: &FakeBackend) {
    for error in [
        execute_target(NativeV2CliCommand::Version, backend)
            .await
            .assert_error(),
        execute_run_unary(NativeV2CliCommand::Version, backend, &mut Vec::new())
            .await
            .assert_error(),
        execute_run_subscription(
            NativeV2CliCommand::Version,
            backend,
            &mut NeverDetach,
            &mut Vec::new(),
        )
        .await
        .assert_error(),
    ] {
        assert!(matches!(error, NativeV2CliError::Usage(_)));
    }
}

fn assert_durable_duplicate_and_close_edges() {
    assert_eq!(
        DurableFollowKind::Watch.done_outcome(),
        CliOutcome::Detached
    );
    let cursor = Cursor::new("v2:11");
    let mut from_cursor = Some(cursor.clone());
    let mut duplicate_output = Vec::new();
    let duplicate = serde_json::from_value(serde_json::json!({
        "subscriptionId":"wave11",
        "runId":RunId::new("run-settled"),
        "title":"Exact miss",
        "source":{
            "repository":"open-engine/zeroshot",
            "branch":"main",
            "revision":"0123456789abcdef0123456789abcdef01234567"
        },
        "size":"small",
        "cursor":cursor,
        "status":{"phase":"admitted"}
    }))
    .assert_value();
    assert!(
        write_durable_event(
            DurableItem::Watch(duplicate),
            &mut from_cursor,
            &mut duplicate_output,
        )
        .assert_value()
        .is_none()
    );
    assert!(duplicate_output.is_empty());
    assert!(
        write_durable_event(
            DurableItem::Closed(SubscriptionCloseReason::Done),
            &mut None,
            &mut Vec::new(),
        )
        .assert_value()
        .is_none()
    );
}

fn validation_files() -> (tempfile::NamedTempFile, tempfile::NamedTempFile) {
    let input = tempfile::NamedTempFile::new().assert_value();
    let runtime = tempfile::NamedTempFile::new().assert_value();
    std::fs::write(input.path(), r#"{"task":"validate exact routes"}"#).assert_value();
    std::fs::write(
        runtime.path(),
        r#"{
            "harness":"codex",
            "provider":"openai",
            "size":"small",
            "nodes":{"worker":{"kind":"agent","model":"provider-model"}}
        }"#,
    )
    .assert_value();
    (input, runtime)
}

async fn assert_validate_only_uses_both_preflight_and_profile_routes(backend: &FakeBackend) {
    let (input_file, runtime_file) = validation_files();
    let input = input_file.path();
    let runtime = runtime_file.path();
    let validation = ["--validate-only"];

    let mut inline = vec![
        std::ffi::OsString::from("run"),
        std::ffi::OsString::from("--title"),
        std::ffi::OsString::from("Inline validation"),
        std::ffi::OsString::from("--input"),
        input.as_os_str().to_owned(),
        std::ffi::OsString::from("--template"),
        std::ffi::OsString::from("single-worker"),
        std::ffi::OsString::from("--runtime-config"),
        runtime.as_os_str().to_owned(),
    ];
    inline.extend(validation.map(std::ffi::OsString::from));
    let mut output = Vec::new();
    assert_eq!(
        execute_native_v2_cli(
            parse_native_v2_args(inline).assert_value(),
            backend,
            &mut NeverDetach,
            &mut output,
        )
        .await
        .assert_value(),
        CliOutcome::Completed
    );
    assert_eq!(output, b"{\"valid\":true}\n");

    let mut profile = vec![
        std::ffi::OsString::from("run"),
        std::ffi::OsString::from("--target"),
        std::ffi::OsString::from("prod"),
        std::ffi::OsString::from("--title"),
        std::ffi::OsString::from("Profile validation"),
        std::ffi::OsString::from("--input"),
        input.as_os_str().to_owned(),
        std::ffi::OsString::from("--profile"),
        std::ffi::OsString::from("user:alpha"),
    ];
    profile.extend(validation.map(std::ffi::OsString::from));
    let mut output = Vec::new();
    assert_eq!(
        execute_native_v2_cli(
            parse_native_v2_args(profile).assert_value(),
            backend,
            &mut NeverDetach,
            &mut output,
        )
        .await
        .assert_value(),
        CliOutcome::Completed
    );
    assert_eq!(output, b"{\"valid\":true}\n");
}

#[tokio::test]
async fn wave11_cli_contract_exact_dispatch_refusals_and_discard_are_serialized() {
    let backend = FakeBackend::default();

    let mut version = Vec::new();
    assert_eq!(
        execute_native_v2_cli(
            NativeV2CliCommand::Version,
            &backend,
            &mut NeverDetach,
            &mut version,
        )
        .await
        .assert_value(),
        CliOutcome::Completed
    );
    assert_eq!(version, crate::native_v2_cli::VERSION.as_bytes());

    let connection = parse_native_v2_args(args(&[
        "connection",
        "list",
        "--target",
        "prod",
        "--scope",
        "org",
    ]))
    .assert_value();
    execute_native_v2_cli(connection, &backend, &mut NeverDetach, &mut Vec::new())
        .await
        .assert_value();

    let resume = parse_native_v2_args(args(&["resume", "run-settled"])).assert_value();
    assert!(
        execute_native_v2_cli(resume, &backend, &mut NeverDetach, &mut Vec::new())
            .await
            .assert_error()
            .to_string()
            .contains("does not have a recoverable workspace")
    );

    let discard = parse_native_v2_args(args(&[
        "discard-workspace",
        "run-settled",
        "--target",
        "prod",
    ]))
    .assert_value();
    let mut output = Vec::new();
    assert_eq!(
        execute_native_v2_cli(discard, &backend, &mut NeverDetach, &mut output)
            .await
            .assert_value(),
        CliOutcome::Completed
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).assert_value(),
        serde_json::json!({"runId":"run-settled", "discarded":true})
    );
    assert!(matches!(
        backend.calls().as_slice(),
        [
            Call::ConnectionList {
                target,
                request: ConnectionListRequest {
                    scope: ConnectionScope::Org,
                },
            },
            Call::Status { run_id, .. },
            Call::DiscardWorkspace {
                target: discard_target,
                run_id: discarded,
            },
        ] if target.as_deref() == Some("prod")
            && run_id == "run-settled"
            && discard_target.as_deref() == Some("prod")
            && discarded == "run-settled"
    ));

    assert_private_dispatch_refusals(&backend).await;
    assert_durable_duplicate_and_close_edges();
    assert_validate_only_uses_both_preflight_and_profile_routes(&backend).await;
}
