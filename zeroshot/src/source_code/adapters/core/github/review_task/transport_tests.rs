use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::*;

async fn response_fixture(response: Vec<u8>) -> (HttpMetadata, String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let url = format!("http://{}/metadata", listener.local_addr().assert_value());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.assert_value();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await.assert_value());
            assert!(
                request.len() <= 8192,
                "fixture request header exceeded its bound"
            );
        }
        // Bounds tests deliberately close their response before the server finishes writing it.
        let _ = stream.write_all(&response).await;
        String::from_utf8(request).assert_value()
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .assert_value();
    (
        HttpMetadata {
            client,
            token: "fixture-authority".into(),
        },
        url,
        task,
    )
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

#[tokio::test]
async fn metadata_http_reader_authenticates_and_preserves_json() {
    let body = br#"{"title":"requirements","body":"preserve the caller's context"}"#;
    let (metadata, url, server) = response_fixture(response(
        "200 OK",
        &format!("Content-Length: {}\r\n", body.len()),
        body,
    ))
    .await;
    let result = metadata
        .read(metadata.client.get(&url))
        .await
        .assert_value();
    assert_eq!(
        result,
        json!({"title":"requirements","body":"preserve the caller's context"})
    );
    let request = server.await.assert_value().to_ascii_lowercase();
    assert!(request.starts_with("get /metadata http/1.1\r\n"));
    assert!(request.contains("authorization: bearer fixture-authority\r\n"));
    assert!(request.contains("accept: application/vnd.github+json\r\n"));
    assert!(request.contains("x-github-api-version: 2022-11-28\r\n"));
}

#[tokio::test]
async fn metadata_http_reader_reports_status_decode_and_interrupted_body_failures() {
    for (status, headers, body, expected) in [
        (
            "403 Forbidden",
            "Content-Length: 2\r\n",
            b"{}".as_slice(),
            "status",
        ),
        ("200 OK", "Content-Length: 1\r\n", b"{".as_slice(), "decode"),
        (
            "200 OK",
            "Content-Length: 1000\r\n",
            b"{}".as_slice(),
            "transport",
        ),
    ] {
        let (metadata, url, server) = response_fixture(response(status, headers, body)).await;
        let result = metadata.read(metadata.client.get(&url)).await;
        match expected {
            "status" => assert!(matches!(result, Err(ReviewResolveError::Http(403)))),
            "decode" => assert!(matches!(result, Err(ReviewResolveError::Identity))),
            _ => assert!(matches!(result, Err(ReviewResolveError::Transport))),
        }
        server.await.assert_value();
    }
}

#[tokio::test]
async fn metadata_http_reader_bounds_declared_and_streamed_response_bytes() {
    for (headers, body) in [
        (
            format!("Content-Length: {}\r\n", MAX_API_BYTES + 1),
            Vec::new(),
        ),
        (String::new(), vec![b'x'; MAX_API_BYTES + 1]),
    ] {
        let (metadata, url, server) = response_fixture(response("200 OK", &headers, &body)).await;
        assert!(matches!(
            metadata.read(metadata.client.get(&url)).await,
            Err(ReviewResolveError::TooLarge)
        ));
        server.await.assert_value();
    }
}

#[tokio::test]
async fn github_metadata_requests_stop_at_a_failed_proxy_without_exposing_authority() {
    for graphql in [false, true] {
        let (_, proxy, server) =
            response_fixture(response("403 Forbidden", "Content-Length: 0\r\n", b"")).await;
        let metadata = HttpMetadata {
            client: reqwest::Client::builder()
                .no_proxy()
                .proxy(reqwest::Proxy::all(&proxy).assert_value())
                .timeout(Duration::from_secs(5))
                .build()
                .assert_value(),
            token: "fixture-authority".into(),
        };
        let result = if graphql {
            metadata
                .graphql("query { viewer { id } }", &json!({}))
                .await
        } else {
            metadata.get("repos/acme/project/issues/42").await
        };
        assert!(matches!(result, Err(ReviewResolveError::Transport)));
        let request = server.await.assert_value();
        assert!(request.starts_with("CONNECT api.github.com:443 HTTP/1.1\r\n"));
        assert!(!request.contains("fixture-authority"));
    }
}

#[tokio::test]
async fn invalid_credential_or_discussion_never_reaches_metadata_transport() {
    for token in [
        String::new(),
        "  ".into(),
        "x".repeat(4097),
        "token\ninjection".into(),
    ] {
        assert!(matches!(
            crate::native_v2_review::resolve_review(request("issues"), &token).await,
            Err(ReviewResolveError::Credential)
        ));
    }
    let mut invalid = request("issues");
    invalid.url = "not a discussion URL".into();
    assert!(matches!(
        crate::native_v2_review::resolve_review(invalid, "fixture-authority").await,
        Err(ReviewResolveError::InvalidUrl)
    ));
}
