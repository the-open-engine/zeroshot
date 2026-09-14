use std::collections::BTreeMap;
use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn response_result(
    response: Vec<u8>,
) -> Result<RunConnectionValues, ConnectionResolutionError> {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.assert_value();
        let mut request = [0_u8; 4096];
        let size = stream.read(&mut request).await.assert_value();
        assert!(String::from_utf8_lossy(&request[..size]).contains("Bearer private-token"));
        let _ = stream.write_all(&response).await;
    });
    let resolver = HttpRunConnectionResolver {
        client: Client::builder()
            .redirect(Policy::none())
            .timeout(RESOLUTION_TIMEOUT)
            .build()
            .assert_value(),
        endpoint: Url::parse(&format!("http://{address}/resolve")).assert_value(),
        bearer_token: Arc::from("private-token"),
        run_id: RunId::new("resolution-test"),
    };
    let result = resolver.resolve(BTreeMap::new()).await;
    server.await.assert_value();
    result
}

fn http_response(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    )
    .into_bytes()
}

#[tokio::test]
async fn actual_resolver_http_failures_preserve_retry_and_refusal_semantics_without_body_leaks() {
    for (status, expected) in [
        (401, ConnectionResolutionError::Refused),
        (403, ConnectionResolutionError::Refused),
        (408, ConnectionResolutionError::Unavailable),
        (429, ConnectionResolutionError::Unavailable),
        (503, ConnectionResolutionError::Unavailable),
        (302, ConnectionResolutionError::InvalidResponse),
        (400, ConnectionResolutionError::InvalidResponse),
    ] {
        let result =
            response_result(http_response(status, "private-token and upstream secrets")).await;
        assert_eq!(result, Err(expected));
        assert!(!format!("{result:?}").contains("private-token"));
        assert!(!expected.to_string().contains("secrets"));
    }
}

#[tokio::test]
async fn successful_status_requires_a_complete_bounded_valid_response() {
    assert!(
        response_result(http_response(200, r#"{"connections":{}}"#))
            .await
            .assert_value()
            .is_empty()
    );
    assert_eq!(
        response_result(http_response(200, "private-token")).await,
        Err(ConnectionResolutionError::InvalidResponse)
    );
    let oversized = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        MAX_RESOLUTION_RESPONSE_BYTES + 1,
    );
    assert_eq!(
        response_result(oversized.into_bytes()).await,
        Err(ConnectionResolutionError::InvalidResponse)
    );
    let truncated = b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n{}".to_vec();
    assert_eq!(
        response_result(truncated).await,
        Err(ConnectionResolutionError::Unavailable)
    );
}

#[tokio::test]
async fn connection_refusal_is_temporary_unavailability_not_authentication_refusal() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let endpoint = Url::parse(&format!(
        "http://{}/resolve",
        listener.local_addr().assert_value()
    ))
    .assert_value();
    drop(listener);
    let resolver = HttpRunConnectionResolver {
        client: Client::new(),
        endpoint,
        bearer_token: Arc::from("private-token"),
        run_id: RunId::new("resolution-test"),
    };
    assert_eq!(
        resolver.resolve(BTreeMap::new()).await,
        Err(ConnectionResolutionError::Unavailable)
    );
}
