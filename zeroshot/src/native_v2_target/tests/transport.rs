use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;
use tokio::net::TcpListener;

use super::super::controller_authority::credentials::test_support::{
    MemoryCredentialStore, MemoryDeviceCodeNotifier,
};
use super::super::*;
use super::fixtures::*;
use super::hosted_authority::{read_http_request, write_http_response};

fn unreachable_target(origin: String, access: TargetAccess) -> TargetRecord {
    TargetRecord {
        name: "lab".to_owned(),
        origin,
        access,
        ..target()
    }
}

#[tokio::test]
async fn offline_target_submission_identifies_target_and_connection_refusal() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let origin = format!("http://{}", listener.local_addr().assert_value());
    drop(listener);
    for access in [TargetAccess::Direct, target().access] {
        let root = temp_root();
        let error = submission_failure(
            unreachable_target(origin.clone(), access),
            root.path("locks"),
        )
        .await;
        assert!(matches!(error, NativeV2CliError::TargetTransport { .. }));
        let message = error.to_string();
        assert!(message.contains("target \"lab\""));
        assert!(message.contains(&origin));
        assert!(message.contains("connection refused"));
        assert!(message.contains("Docker and the target container"));
        assert!(!message.contains("observation transport disconnected"));
        let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
        assert_eq!(diagnostic["code"], "target.unavailable");
        assert_eq!(
            diagnostic["details"],
            json!({"target":"lab", "origin":origin})
        );
    }
}

#[tokio::test]
async fn interrupted_submission_preserves_operation_without_claiming_it_was_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let origin = format!("http://{}", listener.local_addr().assert_value());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.assert_value();
        let request = read_http_request(&mut stream).await;
        assert_eq!(request.path, "/.well-known/zeroshot-native-v2");
        write_http_response(
            &mut stream,
            &json!({
                "kind":"zeroshot.native-v2-target/v2", "authentication":"none",
                "runPath":"/native-v2/run", "sessionPath":"/native-v2/oecp-session",
                "oecpPath":"/native-v2/oecp", "audience":"controller"
            })
            .to_string(),
        )
        .await;
        drop(stream);
        let (mut stream, _) = listener.accept().await.assert_value();
        let request = read_http_request(&mut stream).await;
        assert_eq!(request.path, "/native-v2/run");
        // The request reached the server, but no receipt reached the caller. The server then
        // stops, so every replay is refused and the admission stays unconfirmed.
        drop(stream);
    });
    let root = temp_root();
    let error = submission_failure(
        unreachable_target(origin, TargetAccess::Direct),
        root.path("locks"),
    )
    .await;
    assert!(matches!(
        error,
        NativeV2CliError::SubmissionOutcomeUnknown { .. }
    ));
    let message = error.to_string();
    assert!(message.contains("target run request failed: connection interrupted"));
    assert!(message.contains("--submission-key target-test"));
    assert!(!message.contains("rejected"));
    assert!(!message.contains("connection refused"));
    server.await.assert_value();
}

#[tokio::test]
async fn only_a_run_request_that_may_have_reached_the_target_is_unconfirmed() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let run_url = format!(
        "http://{}/native-v2/run",
        listener.local_addr().assert_value()
    );
    // The listener accepts the connection and never answers.
    let timed_out = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(25))
        .build()
        .assert_value()
        .post(&run_url)
        .send()
        .await
        .assert_error();
    drop(listener);
    let refused = reqwest::Client::builder()
        .no_proxy()
        .build()
        .assert_value()
        .post(&run_url)
        .send()
        .await
        .assert_error();
    let failure =
        |error: &reqwest::Error| TargetAuthorityError::request_failed("target run request", error);
    assert!(failure(&timed_out).submission().is_unconfirmed_submission());
    assert!(!failure(&refused).submission().is_unconfirmed_submission());
    // Requests before the run request, such as discovery, cannot admit a run.
    assert!(!failure(&timed_out).is_unconfirmed_submission());
}

#[tokio::test]
async fn timeout_diagnostic_excludes_secret_request_url() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let origin = format!("http://{}", listener.local_addr().assert_value());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(25))
        .build()
        .assert_value();
    let error = client
        .get(format!("{origin}/private?token=must-not-leak"))
        .send()
        .await
        .assert_error();
    let error = TargetAuthorityError::request_failed("target discovery", &error)
        .into_cli(&unreachable_target(origin, TargetAccess::Direct));
    let diagnostic = serde_json::to_string(&error.diagnostic()).assert_value();
    assert!(diagnostic.contains("request timed out"));
    assert!(!diagnostic.contains("must-not-leak"));
    assert!(!diagnostic.contains("/private"));
}

async fn submission_failure(target: TargetRecord, locks: PathBuf) -> NativeV2CliError {
    let registry = MemoryRegistry::default();
    registry.insert(target).assert_value();
    let authority = TargetHttpControlAuthority::with_dependencies(
        Arc::new(MemoryCredentialStore::default()),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        locks,
    );
    let connector = NativeV2TargetConnector::new(registry, authority, FakeDialer::default());
    connector.submit("lab", run_request()).await.assert_error()
}
