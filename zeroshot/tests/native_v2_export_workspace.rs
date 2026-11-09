#[path = "native_v2_export_workspace/fixture.rs"]
mod fixture;

#[cfg(unix)]
use std::time::Duration;
use fixture::{Fixture, Reply, capability, API_KEY};
use serde_json::{Value, json};

#[tokio::test]
async fn built_cli_exports_latest_and_checkpoint_with_machine_organization_auth() {
    for (reply, checkpoint) in [(Reply::Length, false), (Reply::Chunked("chunked"), true)] {
        let fixture = Fixture::new(reply, capability()).await;
        let output = fixture.export(checkpoint).await;
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture.started.notified(),
        )
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read(&fixture.output).unwrap(), fixture.archive);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&fixture.output)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt, json!({"output":fixture.output}));
        let requests = fixture.requests.lock().unwrap();
        let export = requests
            .iter()
            .find(|request| request.path.ends_with("/export-workspace"))
            .unwrap();
        let expected = if checkpoint {
            json!({"runId":"run-export","fromCheckpoint":"entry-1"})
        } else {
            json!({"runId":"run-export"})
        };
        assert_eq!(export.body, expected);
        assert!(!export.body.to_string().contains(API_KEY));
        fixture.assert_clean();
    }
}

#[tokio::test]
async fn built_cli_rejects_missing_or_future_export_without_other_operations() {
    for capability in [
        Value::Null,
        json!({"kind":"future", "route_template":"/export/{run_id}"}),
    ] {
        let fixture = Fixture::new(Reply::Length, capability).await;
        let output = fixture.export(false).await;
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("does not advertise openengine.workspace-export/v1")
        );
        assert!(
            fixture
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.path == "/.well-known/zeroshot-native-v2")
        );
        assert!(!fixture.output.exists());
        fixture.assert_clean();
    }
}

#[tokio::test]
async fn built_cli_rejects_dot_run_ids_before_authenticated_export() {
    for run_id in [".", ".."] {
        let fixture = Fixture::new(Reply::Length, capability()).await;
        let output = fixture
            .command()
            .args([
                "export-workspace",
                run_id,
                "--target",
                "fixture",
                "--output",
            ])
            .arg(&fixture.output)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("dot path segment"));
        assert!(
            fixture
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.path == "/.well-known/zeroshot-native-v2")
        );
        assert!(!fixture.output.exists());
        fixture.assert_clean();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn built_cli_rejects_non_utf8_output_before_download_or_publication() {
    use std::os::unix::ffi::OsStringExt;

    let mut fixture = Fixture::new(Reply::Length, capability()).await;
    fixture.output = fixture
        .output
        .with_file_name(std::ffi::OsString::from_vec(b"workspace-\xff.tar".to_vec()));
    let output = fixture.export(false).await;
    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(101));
    assert!(String::from_utf8_lossy(&output.stderr).contains("UTF-8"));
    assert!(fixture.requests.lock().unwrap().is_empty());
    assert!(!fixture.output.exists());
    fixture.assert_clean();
}

#[tokio::test]
async fn built_cli_refuses_existing_files_directories_and_destination_races() {
    for directory in [false, true] {
        let fixture = Fixture::new(Reply::Length, capability()).await;
        if directory {
            std::fs::create_dir(&fixture.output).unwrap();
        } else {
            std::fs::write(&fixture.output, b"existing").unwrap();
        }
        let output = fixture.export(false).await;
        assert!(!output.status.success());
        assert!(fixture.requests.lock().unwrap().is_empty());
        if !directory {
            assert_eq!(std::fs::read(&fixture.output).unwrap(), b"existing");
        }
        fixture.assert_clean();
    }
    let fixture = Fixture::new(Reply::Race, capability()).await;
    assert!(!fixture.export(false).await.status.success());
    assert_eq!(
        std::fs::read(&fixture.output).unwrap(),
        b"racing destination"
    );
    fixture.assert_clean();
}

#[tokio::test]
async fn built_cli_rejects_failures_including_exporter_failure_after_a_complete_tar() {
    for reply in [
        Reply::Status,
        Reply::Redirect,
        Reply::WrongType,
        Reply::Partial,
        Reply::TruncatedLength,
        Reply::TruncatedChunks("chunked"),
        Reply::ExporterFailure("chunked"),
        Reply::CloseDelimited,
    ] {
        let fixture = Fixture::new(reply, capability()).await;
        let output = fixture.export(false).await;
        assert!(!output.status.success());
        if matches!(reply, Reply::Redirect) {
            assert!(String::from_utf8_lossy(&output.stderr).contains("status 307"));
        }
        assert!(!String::from_utf8_lossy(&output.stderr).contains(API_KEY));
        assert!(!fixture.output.exists());
        fixture.assert_clean();
    }
}

#[tokio::test]
async fn built_cli_accepts_case_insensitive_chunked_only_after_complete_http_framing() {
    for encoding in ["Chunked", "CHUNKED", " \tChUnKeD \t"] {
        let fixture = Fixture::new(Reply::Chunked(encoding), capability()).await;
        let output = fixture.export(false).await;
        assert!(
            output.status.success(),
            "{encoding}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read(&fixture.output).unwrap(), fixture.archive);
        fixture.assert_clean();

        for reply in [
            Reply::TruncatedChunks(encoding),
            Reply::ExporterFailure(encoding),
        ] {
            let fixture = Fixture::new(reply, capability()).await;
            let output = fixture.export(false).await;
            assert!(!output.status.success(), "accepted truncated {encoding}");
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("workspace export transfer failed")
            );
            assert!(!fixture.output.exists());
            fixture.assert_clean();
        }
    }
}

#[tokio::test]
async fn built_cli_never_uses_length_when_transfer_encoding_is_present() {
    for encoding in ["identity", "gzip", "chunked\r\nTransfer-Encoding: identity"] {
        let fixture = Fixture::new(Reply::LengthAndTransferEncoding(encoding), capability()).await;
        let output = fixture.export(false).await;
        assert!(!output.status.success(), "accepted {encoding}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("requires explicit HTTP framing"));
        assert!(!fixture.output.exists());
        fixture.assert_clean();
    }
    for encoding in ["gzip, chunked", "chunked\r\nTransfer-Encoding: identity"] {
        let fixture = Fixture::new(Reply::Chunked(encoding), capability()).await;
        assert!(!fixture.export(false).await.status.success());
        assert!(!fixture.output.exists());
        fixture.assert_clean();
    }
}

#[tokio::test]
async fn built_cli_requires_explicit_target_and_output() {
    let fixture = Fixture::new(Reply::Length, capability()).await;
    for arguments in [
        vec!["export-workspace", "run-export", "--target", "fixture"],
        vec!["export-workspace", "run-export", "--output", "archive.tar"],
    ] {
        let output = fixture.command().args(arguments).output().await.unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("required"));
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
    fixture.assert_clean();
}

#[cfg(unix)]
#[tokio::test]
async fn built_cli_refuses_existing_and_dangling_symlinks() {
    for dangling in [false, true] {
        let fixture = Fixture::new(Reply::Length, capability()).await;
        let target = fixture.root.path().join("victim");
        if !dangling {
            std::fs::write(&target, b"original").unwrap();
        }
        std::os::unix::fs::symlink(&target, &fixture.output).unwrap();
        assert!(!fixture.export(false).await.status.success());
        assert!(
            std::fs::symlink_metadata(&fixture.output)
                .unwrap()
                .is_symlink()
        );
        assert!(fixture.requests.lock().unwrap().is_empty());
        if !dangling {
            assert_eq!(std::fs::read(target).unwrap(), b"original");
        }
        fixture.assert_clean();
    }
}

#[cfg(windows)]
#[tokio::test]
async fn built_cli_refuses_windows_directory_links_without_replacement() {
    let fixture = Fixture::new(Reply::Length, capability()).await;
    let target = fixture.root.path().join("victim");
    std::fs::create_dir(&target).unwrap();
    // Junctions do not require the Windows symlink privilege.
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&fixture.output)
        .arg(&target)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!fixture.export(false).await.status.success());
    assert!(fixture.requests.lock().unwrap().is_empty());
    fixture.assert_clean();
}

#[cfg(unix)]
#[tokio::test]
async fn ctrl_c_cancels_before_headers_or_during_transfer_and_removes_temporary_output() {
    for reply in [Reply::StallHeaders, Reply::StallBody] {
        let fixture = Fixture::new(reply, capability()).await;
        let mut command = fixture.export_command();
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = command.spawn().unwrap();
        tokio::time::timeout(Duration::from_secs(10), fixture.started.notified())
            .await
            .unwrap();
        assert_eq!(
            unsafe { libc::kill(child.id().unwrap() as i32, libc::SIGINT) },
            0
        );
        let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("workspace export cancelled"));
        assert!(!fixture.output.exists());
        fixture.assert_clean();
    }
}
