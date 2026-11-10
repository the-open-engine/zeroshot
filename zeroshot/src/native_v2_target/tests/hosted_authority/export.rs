use futures_util::StreamExt;
use openengine_cluster_protocol::{RunExportWorkspaceParams, RunId};
use openengine_cluster_testkit::assertions::AssertValue;
use super::*;
use super::super::super::controller_authority::credentials::test_support::MemoryDeviceCodeNotifier;

#[tokio::test]
async fn export_reuses_oauth_refresh_and_session_organization() {
    let root = super::super::fixtures::temp_root();
    let (listener, address, origin) = bind_target_authority().await;
    let target = super::super::fixtures::hosted_target("prod", origin.clone());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut token_index = 0;
        loop {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            if request.path.ends_with("/export-workspace") {
                assert_eq!(request.authorization.as_deref(), Some("Bearer access-1"));
                assert_eq!(request.organization, None); // Human organization comes from the verified session.
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&request.body).assert_value(),
                    json!({"runId":"run-export"})
                );
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: 7\r\nConnection: close\r\n\r\narchive").await.assert_value();
                requests.push(request);
                return requests;
            }
            let mut body = authority_response(&request, &origin, address, &mut token_index);
            if request.path == "/.well-known/zeroshot-native-v2" {
                let mut value: serde_json::Value = serde_json::from_str(&body).assert_value();
                value["extensions"]["workspace_export"] = json!({"kind":"openengine.workspace-export/v1", "route_template":"/native-v2/runs/{run_id}/export-workspace"});
                body = value.to_string();
            }
            write_http_response(&mut stream, &body).await;
            requests.push(request);
        }
    });
    let credentials = std::sync::Arc::new(LoginBlockingCredentialStore::new("refresh-0"));
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        std::sync::Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("refresh-locks"),
    );
    for run_id in [".", ".."] {
        let error = authority
            .hosted_run_export_workspace(
                &target,
                RunExportWorkspaceParams {
                    run_id: RunId::new(run_id),
                    from_checkpoint: None,
                },
            )
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("dot path segment"));
        assert_eq!(credentials.reads(), 0);
    }
    let mut stream = authority
        .hosted_run_export_workspace(
            &target,
            RunExportWorkspaceParams {
                run_id: RunId::new("run-export"),
                from_checkpoint: None,
            },
        )
        .await
        .assert_value();
    let mut contents = Vec::new();
    while let Some(bytes) = stream.next().await {
        contents.extend(bytes.assert_value());
    }
    assert_eq!(contents, b"archive");
    let requests = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .assert_value()
        .assert_value();
    assert!(requests.iter().any(|r| r.path == "/session"));
    assert!(
        requests
            .iter()
            .any(|r| r.path == "/oauth/token" && r.body.contains("refresh_token"))
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.path == "/native-v2/run" || r.path == "/native-v2/oecp-session")
    );
}

struct CancelExport(std::sync::Arc<Notify>);

#[async_trait::async_trait]
impl zeroshot_engine::native_v2_cli::DetachSignal for CancelExport {
    async fn wait(&mut self) {
        self.0.notified().await;
    }
}

#[tokio::test]
async fn export_cancellation_closes_transfer_and_cleans_temporary_output_on_every_platform() {
    use zeroshot_engine::native_v2_cli::{
        execute_native_v2_cli, parse_native_v2_args, oecp::NamedTargetCliBackend,
    };
    use super::super::fixtures::{MemoryRegistry, FakeDialer};
    let root = super::super::fixtures::temp_root();
    let (listener, _, origin) = bind_target_authority().await;
    let target = super::super::fixtures::hosted_target("prod", origin.clone());
    let started = std::sync::Arc::new(Notify::new());
    let cancelled = started.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            if request.path.ends_with("/export-workspace") {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nTransfer-Encoding: chunked\r\n\r\n7\r\narchive\r\n").await.assert_value();
                cancelled.notify_one();
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).await.assert_value(), 0);
                return;
            }
            let mut value: serde_json::Value =
                serde_json::from_str(&hosted_discovery(&origin)).assert_value();
            value["extensions"]["workspace_export"] = json!({"kind":"openengine.workspace-export/v1", "route_template":"/native-v2/runs/{run_id}/export-workspace"});
            write_http_response(&mut stream, &value.to_string()).await;
        }
    });
    let registry = MemoryRegistry::default();
    registry.insert(target).assert_value();
    let authority = super::super::fixtures::test_http_authority(root.path("locks"))
        .with_machine_key("machine-secret", None);
    let backend = NamedTargetCliBackend::new(NativeV2TargetConnector::new(
        registry,
        authority,
        FakeDialer::default(),
    ));
    let download = root.path("downloads");
    std::fs::create_dir(&download).assert_value();
    let destination = download.join("workspace.tar");
    let command = parse_native_v2_args(
        [
            "export-workspace",
            "run-export",
            "--target",
            "prod",
            "--output",
        ]
        .into_iter()
        .map(std::ffi::OsString::from)
        .chain([destination.into_os_string()]),
    )
    .assert_value();
    let error = execute_native_v2_cli(
        command,
        &backend,
        &mut CancelExport(started),
        &mut Vec::new(),
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("workspace export cancelled"));
    assert_eq!(std::fs::read_dir(download).assert_value().count(), 0);
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .assert_value()
        .assert_value();
}

#[tokio::test]
async fn export_oauth_errors_never_echo_bearer_credentials() {
    let root = super::super::fixtures::temp_root();
    let (listener, address, origin) = bind_target_authority().await;
    let target = super::super::fixtures::hosted_target("prod", origin.clone());
    let server = tokio::spawn(async move {
        let mut token_index = 0;
        loop {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            if request.path.ends_with("/export-workspace") {
                write_http_response_with_status(&mut stream, "403 Forbidden", &json!({"code":"export_refused", "message":"refused access-1", "details":{"echo":"access-1"}}).to_string()).await;
                return;
            }
            let mut body = authority_response(&request, &origin, address, &mut token_index);
            if request.path == "/.well-known/zeroshot-native-v2" {
                let mut value: serde_json::Value = serde_json::from_str(&body).assert_value();
                value["extensions"]["workspace_export"] = json!({"kind":"openengine.workspace-export/v1", "route_template":"/native-v2/runs/{run_id}/export-workspace"});
                body = value.to_string();
            }
            write_http_response(&mut stream, &body).await;
        }
    });
    let (credentials, authority) = test_authority(&root);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    let error = authority
        .hosted_run_export_workspace(
            &target,
            RunExportWorkspaceParams {
                run_id: RunId::new("run-export"),
                from_checkpoint: None,
            },
        )
        .await
        .err()
        .unwrap();
    assert!(!format!("{error:?}").contains("access-1"));
    assert!(error.to_string().contains("[REDACTED]"));
    server.await.assert_value();
}
