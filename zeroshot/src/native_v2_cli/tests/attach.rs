use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

async fn execute_attach(backend: &FakeBackend) -> Result<(CliOutcome, String), NativeV2CliError> {
    let command = parse_native_v2_args(args(&[
        "attach",
        "run-public",
        "exec-9",
        "--target",
        "prod",
    ]))
    .assert_value();
    let mut output = Vec::new();
    let outcome = execute_native_v2_cli(command, backend, &mut NeverDetach, &mut output).await?;
    Ok((outcome, String::from_utf8(output).assert_value()))
}

fn assert_same_attach_reopened(calls: &[Call], attempts: usize) {
    assert_eq!(calls.len(), attempts);
    for call in calls {
        assert_eq!(
            call,
            &Call::Attach {
                target: Some("prod".to_owned()),
                run_id: "run-public".to_owned(),
                execution: "exec-9".to_owned(),
            }
        );
    }
}

async fn assert_attach_reconnects(backend: FakeBackend, attempts: usize) {
    let (outcome, output) = execute_attach(&backend).await.assert_value();
    assert_eq!(outcome, CliOutcome::Completed);
    assert_same_attach_reopened(&backend.calls(), attempts);
    assert_eq!(output.matches("before reconnect").count(), 1);
    assert_eq!(output.matches("after reconnect").count(), 1);
}

#[tokio::test]
async fn reopens_the_same_execution_after_transport_disconnect() {
    for (backend, attempts) in [
        (FakeBackend::with_reconnecting_attach_after_disconnect(), 2),
        (FakeBackend::with_attach_connection_failure(2), 3),
    ] {
        assert_attach_reconnects(backend, attempts).await;
    }
}

#[tokio::test]
async fn reopens_the_same_execution_after_transport_eof() {
    assert_attach_reconnects(FakeBackend::with_reconnecting_attach_after_eof(), 2).await;
}

#[tokio::test]
async fn reopens_the_same_execution_after_slow_consumer_close() {
    assert_attach_reconnects(
        FakeBackend::with_reconnecting_attach_after_slow_consumer(),
        2,
    )
    .await;
}

#[tokio::test]
async fn done_closes_cleanly_without_reopening() {
    let backend = FakeBackend::default();
    let (outcome, output) = execute_attach(&backend).await.assert_value();
    assert_eq!(outcome, CliOutcome::Completed);
    assert!(output.is_empty());
    assert!(matches!(backend.calls().as_slice(), [Call::Attach { .. }]));
}

#[tokio::test]
async fn permanent_stream_error_surfaces_without_reopening() {
    let backend = FakeBackend::with_failed_attach();
    let error = execute_attach(&backend).await.assert_error();
    assert!(matches!(error, NativeV2CliError::Protocol(message) if message == "attach rejected"));
    assert!(matches!(backend.calls().as_slice(), [Call::Attach { .. }]));
}

#[tokio::test]
async fn initial_connection_failure_reports_the_target_without_retrying() {
    let backend = FakeBackend::with_attach_connection_failure(1);
    let error = execute_attach(&backend).await.assert_error();
    assert!(matches!(error, NativeV2CliError::TargetTransport { .. }));
    let message = error.to_string();
    assert!(message.contains("prod"));
    assert!(message.contains("https://target.example"));
    assert!(message.contains("connection refused"));
    assert!(matches!(backend.calls().as_slice(), [Call::Attach { .. }]));
}
