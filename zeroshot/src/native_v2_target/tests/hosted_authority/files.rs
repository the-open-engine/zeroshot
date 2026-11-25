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
