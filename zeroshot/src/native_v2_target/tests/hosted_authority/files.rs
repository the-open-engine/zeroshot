use super::*;
use super::super::fixtures::{hosted_target, temp_root};
use openengine_cluster_protocol::{
    FilePath, FileRequest, FileResponse, FileScope, TARGET_FILES_KIND, TARGET_FILES_PATH,
};
use crate::native_v2_target::controller_authority::files::FileReply;
use zeroshot_engine::native_v2_files::new_transfer;

#[tokio::test]
async fn hosted_file_refusal_invalidates_access_without_replaying_and_list_reuses_refresh() {
    let root = temp_root();
    let (listener, address, origin) = bind_target_authority().await;
    let server_origin = origin.clone();
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        let mut token_index = 0;
        let mut files = 0;
        while files < 2 {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            let (status, body) = if request.path == TARGET_FILES_PATH {
                let bytes = request.body.as_bytes();
                let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
                let parsed: FileRequest = serde_json::from_slice(&bytes[4..]).unwrap();
                assert!(matches!(
                    parsed,
                    FileRequest::List {
                        scope: FileScope::Org,
                        ..
                    }
                ));
                assert_eq!(bytes.len(), length + 4, "no unrequested transfer bytes");
                files += 1;
                assert_eq!(
                    request.authorization,
                    Some(format!("Bearer access-{files}"))
                );
                if files == 1 {
                    ("401 Unauthorized", "{}".to_owned())
                } else {
                    ("200 OK", json!({"kind": "list", "paths": []}).to_string())
                }
            } else {
                let mut body =
                    authority_response(&request, &server_origin, address, &mut token_index);
                if request.path == "/.well-known/zeroshot-native-v2" {
                    let mut discovery: serde_json::Value = serde_json::from_str(&body).unwrap();
                    discovery["extensions"]["files"] = json!({"kind": TARGET_FILES_KIND});
                    body = discovery.to_string();
                }
                ("200 OK", body)
            };
            write_http_response_with_status(&mut stream, status, &body).await;
            captured.push(request);
        }
        assert_eq!(
            token_index, 2,
            "401 must retire the rejected access issuance"
        );
        captured
    });
    let (credentials, authority) = test_authority(&root);
    let target = hosted_target("files", origin);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    let scratch = root.path("transfers");
    let staged = new_transfer(&scratch).unwrap();
    let request = FileRequest::List {
        scope: FileScope::Org,
        path: FilePath::new("").unwrap(),
    };
    let error = authority
        .file_request(&target, &request, (&staged, &scratch))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("401"));
    let reply = authority
        .file_request(&target, &request, (&staged, &scratch))
        .await
        .unwrap();
    assert!(matches!(reply, FileReply::Json(FileResponse::List { paths }) if paths.is_empty()));
    let captured = tokio::time::timeout(std::time::Duration::from_secs(30), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        captured
            .iter()
            .filter(|request| request.path == TARGET_FILES_PATH)
            .count(),
        2
    );
    assert_eq!(
        std::fs::read_dir(&scratch).unwrap().count(),
        2,
        "only the request stage and namespace lock"
    );
}

async fn file_authority(
    hosted: bool,
    replies: Vec<Vec<u8>>,
) -> (String, tokio::task::JoinHandle<Vec<CapturedHttpRequest>>) {
    let (listener, address, origin) = bind_target_authority().await;
    let server_origin = origin.clone();
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        let mut token_index = 0;
        let mut replies = replies.into_iter();
        loop {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            if request.path == TARGET_FILES_PATH {
                stream
                    .write_all(&replies.next().assert_value())
                    .await
                    .assert_value();
                stream.shutdown().await.assert_value();
                captured.push(request);
                if replies.len() == 0 {
                    return captured;
                }
            } else {
                let body = if !hosted {
                    assert_eq!(request.path, "/.well-known/zeroshot-native-v2");
                    let mut discovery = serde_json::to_value(
                        zeroshot_engine::native_v2_target_authority::TargetDiscoveryDocument::direct(
                            zeroshot_engine::native_v2_target_authority::TargetAuthentication::None,
                        ),
                    ).assert_value();
                    discovery["extensions"]["files"] = json!({"kind":TARGET_FILES_KIND});
                    discovery.to_string()
                } else {
                    let body =
                        authority_response(&request, &server_origin, address, &mut token_index);
                    if request.path == "/.well-known/zeroshot-native-v2" {
                        let mut discovery: serde_json::Value =
                            serde_json::from_str(&body).assert_value();
                        discovery["extensions"]["files"] = json!({"kind":TARGET_FILES_KIND});
                        discovery.to_string()
                    } else {
                        body
                    }
                };
                write_http_response(&mut stream, &body).await;
                captured.push(request);
            }
        }
    });
    (origin, server)
}

fn framed_response(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
    response.extend_from_slice(body);
    response
}

#[tokio::test]
async fn authenticated_upload_release_checks_each_receipt_without_replaying_refusals() {
    use zeroshot_engine::native_v2_files::FilePin;
    use openengine_cluster_protocol::{IdempotencyKey, RunId};
    for refused in [false, true] {
        let root = super::super::fixtures::TempRoot::new_in(
            "file-authority",
            &std::env::temp_dir().canonicalize().assert_value(),
        )
        .assert_value();
        let response = |body: &[u8]| framed_response("application/json", body);
        let (origin, server) = file_authority(
            true,
            vec![
                response(br#"{"kind":"released"}"#),
                response(if refused {
                    br#"{"kind":"list","paths":[]}"#
                } else {
                    br#"{"kind":"released"}"#
                }),
            ],
        )
        .await;
        let (credentials, authority) = test_authority(&root);
        let target = hosted_target("uploads", origin);
        credentials
            .set(&target.id, "refresh-0")
            .await
            .assert_value();
        let pins: Vec<_> = ["one", "two"]
            .into_iter()
            .map(|operation| FilePin {
                run_id: RunId::new("losing-answer"),
                operation: IdempotencyKey::new(operation).assert_value(),
            })
            .collect();
        let result = authority.release_staged_uploads(&target, &pins).await;
        if refused {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("unexpected upload release response")
            );
        } else {
            result.assert_value();
        }
        let captured = tokio::time::timeout(std::time::Duration::from_secs(30), server)
            .await
            .assert_value()
            .assert_value();
        let requests: Vec<_> = captured
            .iter()
            .filter(|r| r.path == TARGET_FILES_PATH)
            .collect();
        assert_eq!(
            requests.len(),
            2,
            "a refusal cannot cause another release mutation"
        );
        for (request, pin) in requests.into_iter().zip(pins) {
            assert_eq!(request.authorization.as_deref(), Some("Bearer access-1"));
            let bytes = request.body.as_bytes();
            let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
            assert_eq!(length + 4, bytes.len(), "release carries no file bytes");
            assert_eq!(
                serde_json::from_slice::<FileRequest>(&bytes[4..]).assert_value(),
                FileRequest::ReleaseStaged {
                    run_id: pin.run_id,
                    operation: pin.operation,
                }
            );
        }
        assert_eq!(
            std::fs::read_dir(root.path("refresh-locks/file-transfers"))
                .assert_value()
                .count(),
            1,
            "only the namespace lock survives"
        );
    }
}

#[tokio::test]
async fn downloads_require_complete_framing_and_preserve_binary_bytes() {
    use std::io::Read as _;
    use openengine_cluster_protocol::{
        FileAddress, FileSelection, TARGET_FILES_MEDIA_TYPE, MAX_FILE_TRANSFER_BYTES,
    };
    use super::super::fixtures::direct_target;
    let root = super::super::fixtures::TempRoot::new_in(
        "file-authority",
        &std::env::temp_dir().canonicalize().assert_value(),
    )
    .assert_value();
    let source = root.path("source.bin");
    std::fs::write(&source, [0, 255, 13, 10, 128]).assert_value();
    let mut packed = Vec::new();
    zeroshot_engine::native_v2_files::pack(&source, &mut packed).assert_value();
    let replies = vec![
        framed_response(TARGET_FILES_MEDIA_TYPE, &packed),
        framed_response("application/json", &packed),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/vnd.openengine.file-tree.v1\r\nConnection: close\r\n\r\n0\r\n\r\n".to_vec(),
        format!("HTTP/1.1 200 OK\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_FILE_TRANSFER_BYTES + 1).into_bytes(),
        format!("HTTP/1.1 200 OK\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nContent-Length: 10\r\nConnection: close\r\n\r\nshort").into_bytes(),
    ];
    let cases = replies.len();
    let (origin, server) = file_authority(false, replies).await;
    let (_, authority) = test_authority(&root);
    let target = direct_target(origin);
    let scratch = root.path("downloads");
    let staged = new_transfer(&scratch).assert_value();
    let request = FileRequest::Download {
        selection: FileSelection {
            address: FileAddress {
                scope: FileScope::User,
                path: FilePath::new("library/source.bin").assert_value(),
            },
            member: FilePath::new("").assert_value(),
        },
    };
    for case in 0..cases {
        let reply = authority
            .file_request(&target, &request, (&staged, &scratch))
            .await;
        if case == 0 {
            let FileReply::Tree(mut tree) = reply.ok().assert_value() else {
                panic!("expected file tree")
            };
            let mut actual = Vec::new();
            tree.read_to_end(&mut actual).assert_value();
            assert_eq!(actual, packed);
        } else {
            assert!(reply.is_err(), "malformed download {case} was accepted");
        }
        assert_eq!(
            std::fs::read_dir(&scratch).assert_value().count(),
            2,
            "failed download cannot leave private transfer files"
        );
    }
    let captured = tokio::time::timeout(std::time::Duration::from_secs(30), server)
        .await
        .assert_value()
        .assert_value();
    assert_eq!(
        captured
            .iter()
            .filter(|r| r.path == TARGET_FILES_PATH)
            .count(),
        cases
    );
    assert!(
        captured.iter().all(|r| r.authorization.is_none()),
        "direct targets cannot acquire or send OAuth tokens"
    );
}

#[tokio::test]
async fn staging_sends_the_framed_selection_and_exact_transfer_bytes() {
    use std::io::Write as _;
    use openengine_cluster_protocol::{IdempotencyKey, RunId};
    use super::super::fixtures::direct_target;
    let root = super::super::fixtures::TempRoot::new_in(
        "file-authority",
        &std::env::temp_dir().canonicalize().assert_value(),
    )
    .assert_value();
    let (origin, server) = file_authority(
        false,
        vec![framed_response(
            "application/json",
            br#"{"kind":"staged","runId":"r","operation":"o"}"#,
        )],
    )
    .await;
    let (_, authority) = test_authority(&root);
    let target = direct_target(origin);
    let scratch = root.path("stage");
    let mut transfer = new_transfer(&scratch).assert_value();
    let bytes = b"accepted private bytes".repeat(4_000);
    transfer.write_all(&bytes).assert_value();
    let request = FileRequest::Stage {
        run_id: RunId::new("r"),
        operation: IdempotencyKey::new("o").assert_value(),
    };
    assert!(matches!(
        authority
            .file_request(&target, &request, (&transfer, &scratch))
            .await
            .ok()
            .assert_value(),
        FileReply::Json(FileResponse::Staged { .. })
    ));
    let captured = tokio::time::timeout(std::time::Duration::from_secs(30), server)
        .await
        .assert_value()
        .assert_value();
    let sent = captured
        .iter()
        .find(|r| r.path == TARGET_FILES_PATH)
        .assert_value()
        .body
        .as_bytes();
    let length = u32::from_be_bytes(sent[..4].try_into().unwrap()) as usize;
    assert_eq!(
        serde_json::from_slice::<FileRequest>(&sent[4..4 + length]).assert_value(),
        request
    );
    assert_eq!(&sent[4 + length..], bytes);
}
