use super::*;
use openengine_cluster_protocol::{
    MAX_FILE_TRANSFER_BYTES, TARGET_FILES_KIND, TARGET_FILES_PATH, TargetAuthentication,
    TargetDiscoveryDocument, TargetFilesDiscovery,
};
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn local_errors_preserve_selections_and_caller_destinations() {
    let fixture = Fixture::new();
    let source = fixture.root.path().join("input");
    std::fs::write(&source, b"original").unwrap();
    fixture
        .cli(&[
            "file",
            "upload",
            source.to_str().unwrap(),
            "blob",
            "--operation",
            "same",
        ])
        .await;
    std::fs::write(&source, b"changed").unwrap();
    assert_failure(
        &fixture,
        &[
            "file",
            "upload",
            source.to_str().unwrap(),
            "blob",
            "--operation",
            "same",
        ],
    )
    .await;
    for args in [
        vec!["file", "list", "missing"],
        vec!["file", "list", "blob", "--member", "missing"],
        vec!["file", "upload", "does-not-exist", "blob"],
        vec!["file", "list", "--target", "missing"],
    ] {
        assert_failure(&fixture, &args).await;
    }
    let destination = fixture.root.path().join("output");
    std::fs::write(&destination, b"caller bytes").unwrap();
    assert_failure(
        &fixture,
        &["file", "download", "blob", destination.to_str().unwrap()],
    )
    .await;
    assert_eq!(std::fs::read(&destination).unwrap(), b"caller bytes");
    assert_failure(
        &fixture,
        &["file", "download", "missing", destination.to_str().unwrap()],
    )
    .await;
    std::fs::remove_file(&destination).unwrap();
    fixture
        .cli(&["file", "download", "blob", destination.to_str().unwrap()])
        .await;
    assert_eq!(std::fs::read(destination).unwrap(), b"original");
    assert_eq!(
        fixture.cli(&["file", "remove", "missing"]).await["removed"],
        false
    );
}

async fn assert_failure(fixture: &Fixture, args: &[&str]) -> String {
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        fixture.command().args(args).output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success(), "unexpected success: {args:?}");
    assert_ne!(output.status.code(), Some(101));
    assert!(output.stdout.is_empty());
    String::from_utf8(output.stderr).unwrap()
}

#[tokio::test]
async fn direct_download_refuses_invalid_http_framing_without_publishing_or_leaking() {
    let too_large = MAX_FILE_TRANSFER_BYTES + 1;
    for (head, body, expected) in [
        (
            "403 Forbidden\r\nContent-Length: 0".to_owned(),
            "",
            "file request",
        ),
        (
            "206 Partial Content\r\nContent-Length: 0".to_owned(),
            "",
            "complete framed file tree",
        ),
        (
            "200 OK\r\nContent-Type: text/plain\r\nContent-Length: 0".to_owned(),
            "",
            "complete framed file tree",
        ),
        (
            format!(
                "200 OK\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nTransfer-Encoding: chunked"
            ),
            "0\r\n\r\n",
            "complete framed file tree",
        ),
        (
            format!("200 OK\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}"),
            "",
            "invalid file download length",
        ),
        (
            format!(
                "200 OK\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nContent-Length: {too_large}"
            ),
            "",
            "invalid file download length",
        ),
        (
            format!("200 OK\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nContent-Length: 16"),
            "short",
            "file download",
        ),
    ] {
        let fixture = Fixture::new();
        let response = format!("HTTP/1.1 {head}\r\nConnection: close\r\n\r\n{body}").into_bytes();
        let (address, server) = download_server(response).await;
        super::regressions::register(&fixture, address).await;
        let destination = fixture.root.path().join("output");
        let error = assert_failure(
            &fixture,
            &[
                "file",
                "download",
                "blob",
                destination.to_str().unwrap(),
                "--target",
                "disk",
            ],
        )
        .await;
        assert!(error.contains(expected), "{error}");
        assert!(!destination.exists());
        assert!(
            !error.contains(fixture.root.path().to_str().unwrap()),
            "private scratch path in error: {error}"
        );
        tokio::time::timeout(Duration::from_secs(30), server)
            .await
            .unwrap()
            .unwrap();
        let scratch = fixture.root.path().join("config/file-transfers");
        if scratch.exists() {
            assert!(
                std::fs::read_dir(scratch)
                    .unwrap()
                    .all(|entry| entry.unwrap().file_name() == "transfers.lock")
            );
        }
    }
}

pub(super) async fn download_server(
    response: Vec<u8>,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(socket.read_u8().await.unwrap());
                assert!(bytes.len() < 32768);
            }
            let request = String::from_utf8(bytes).unwrap();
            let reply = if request.starts_with("GET ") {
                let mut discovery = TargetDiscoveryDocument::direct(TargetAuthentication::None)
                    .with_run_sizes(zeroshot_engine::native_v2_hosting::host_run_sizes());
                discovery.extensions.files = Some(TargetFilesDiscovery {
                    kind: TARGET_FILES_KIND.to_owned(),
                });
                let json = serde_json::to_string(&discovery).unwrap();
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                ).into_bytes()
            } else {
                assert!(request.starts_with(&format!("POST {TARGET_FILES_PATH} ")));
                let length = request
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                socket.read_exact(&mut vec![0; length]).await.unwrap();
                response.clone()
            };
            socket.write_all(&reply).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    (address, task)
}
