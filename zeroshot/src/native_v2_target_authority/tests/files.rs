use super::*;
use crate::native_v2_files::{DiskFileStore, SingleAccountFiles};
use openengine_cluster_protocol::{
    FileRequest, FileScope, FilePath, TARGET_FILES_MEDIA_TYPE, TARGET_FILES_KIND,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn optional_file_mount_preserves_existing_host_authentication_before_body_reads() {
    let temp =
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().assert_value()).assert_value();
    let target = Arc::new(NativeV2TargetAuthority::new(Arc::new(
        FakeFactory::default(),
    )));
    let host = DiskFileStore::open(
        temp.path().join("files"),
        "selected".to_owned(),
        Arc::new(SingleAccountFiles {
            principal: "test-user".to_owned(),
            tenant: "test-target".to_owned(),
        }),
    )
    .unwrap_or_else(|error| panic!("open target file store: {error:?}"));
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let server = NativeV2TargetServer::new_hosted(
        target,
        Arc::new(FakeSessions),
        format!("ws://{address}/native-v2/oecp"),
    )
    .assert_value()
    .with_files(TargetFilesMount {
        target: "selected".to_owned(),
        host: Arc::new(host),
        transfers: temp.path().join("transfers"),
    })
    .assert_value();
    let server = Arc::new(server);
    let task = tokio::spawn(server.clone().serve(listener));
    let client = reqwest::Client::builder().no_proxy().build().assert_value();
    let document: TargetDiscoveryDocument = client
        .get(format!("http://{address}{DISCOVERY_PATH}"))
        .send()
        .await
        .assert_value()
        .json()
        .await
        .assert_value();
    assert_eq!(
        document.extensions.files.assert_value().kind,
        TARGET_FILES_KIND
    );
    // Refuse an unauthorized stalled body from the header alone.
    let unauthorized = format!(
        "POST /native-v2/files HTTP/1.1\r\nHost: {address}\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nContent-Length: 1000000\r\n\r\n"
    );
    let mut socket = tokio::net::TcpStream::connect(address).await.assert_value();
    socket
        .write_all(unauthorized.as_bytes())
        .await
        .assert_value();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.read_to_end(&mut response),
    )
    .await
    .assert_value()
    .assert_value();
    assert!(response.starts_with(b"HTTP/1.1 401"));
    // Duplicate transfer-encoding headers must not be mistaken for an absent header.
    let malformed = format!(
        "POST /native-v2/files HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer control-token\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: identity\r\n\r\n"
    );
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(malformed.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 400"));
    assert_file_rejection_consumes_only_headers(&server, &unauthorized, b"HTTP/1.1 401").await;
    assert_file_rejection_consumes_only_headers(&server, &malformed, b"HTTP/1.1 400").await;
    assert_eq!(
        std::fs::read_dir(temp.path().join("transfers"))
            .assert_value()
            .filter(|entry| entry
                .as_ref()
                .assert_value()
                .file_name()
                .to_string_lossy()
                .starts_with("transfer-"))
            .count(),
        0
    );
    let metadata = serde_json::to_vec(&FileRequest::List {
        scope: FileScope::User,
        path: FilePath::new("").assert_value(),
    })
    .assert_value();
    let mut body = (metadata.len() as u32).to_be_bytes().to_vec();
    body.extend(metadata);
    let response = client
        .post(format!("http://{address}/native-v2/files"))
        .bearer_auth("control-token")
        .header("Content-Type", TARGET_FILES_MEDIA_TYPE)
        .body(body)
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.assert_value(),
        json!({"kind":"list", "paths":[]})
    );
    task.abort();
}

async fn assert_file_rejection_consumes_only_headers(
    server: &NativeV2TargetServer,
    headers: &str,
    status: &[u8],
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let stream = stream.into_std().unwrap();
    // Retain the receive queue after the handler closes its socket. This exposes unread header
    // bytes on every OS, even where they do not reset the peer before its HTTP error arrives.
    let mut observer = tokio::net::TcpStream::from_std(stream.try_clone().unwrap()).unwrap();
    let stream = tokio::net::TcpStream::from_std(stream).unwrap();
    const BODY: &[u8] = b"body must remain unread";
    let mut request = headers.as_bytes().to_vec();
    request.extend_from_slice(BODY);
    client.write_all(&request).await.unwrap();
    server.serve_connection(stream).await.unwrap();
    let mut remaining = [0_u8; BODY.len()];
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        observer.read_exact(&mut remaining),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(remaining, BODY);
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(response.starts_with(status));
}

#[path = "files/cleanup.rs"]
mod cleanup;
