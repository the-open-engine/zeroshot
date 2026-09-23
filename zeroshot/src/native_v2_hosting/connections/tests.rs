use std::collections::BTreeMap;
use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn resolver_wire(
    endpoint: impl Into<String>,
    bearer_token: impl Into<String>,
    keys: Vec<ConnectionKey>,
    source_connection: Option<ConnectionKey>,
) -> TargetConnectionResolver {
    TargetConnectionResolver {
        endpoint: endpoint.into(),
        bearer_token: bearer_token.into(),
        keys,
        source_connection,
    }
}

fn key(value: &str) -> ConnectionKey {
    ConnectionKey::new(value).assert_value()
}

#[test]
fn hosting_source_contract_resolver_accepts_only_bounded_https_authority() {
    let primary = key("primary");
    let secondary = key("secondary");
    let plan = build_connection_resolver(
        RunId::new("resolver-plan"),
        resolver_wire(
            "https://resolver.example/v1/connections",
            "private-token",
            vec![secondary.clone(), primary.clone()],
            Some(primary.clone()),
        ),
    )
    .assert_value();
    assert_eq!(plan.keys, BTreeSet::from([primary.clone(), secondary]));
    assert_eq!(plan.source_connection, Some(primary.clone()));

    for endpoint in [
        "not a URL",
        "http://resolver.example/connections",
        "https://",
        "https://user@resolver.example/connections",
        "https://user:secret@resolver.example/connections",
        "https://resolver.example/connections?run=1",
        "https://resolver.example/connections#fragment",
    ] {
        let error = build_connection_resolver(
            RunId::new("resolver-plan"),
            resolver_wire(endpoint, "private-token", vec![primary.clone()], None),
        )
        .err()
        .expect("invalid resolver endpoint must fail closed");
        assert_eq!(error.message(), "connection resolver endpoint is invalid");
    }
    for token in [
        String::new(),
        "line\nbreak".to_owned(),
        "x".repeat(16 * 1024 + 1),
    ] {
        let error = build_connection_resolver(
            RunId::new("resolver-plan"),
            resolver_wire(
                "https://resolver.example/connections",
                token,
                vec![primary.clone()],
                None,
            ),
        )
        .err()
        .expect("invalid resolver token must fail closed");
        assert_eq!(
            error.message(),
            "connection resolver bearer token is invalid"
        );
    }
    for (keys, source_connection) in [
        (Vec::new(), None),
        (vec![primary.clone(), primary.clone()], None),
        (vec![primary.clone()], Some(key("outside"))),
    ] {
        let error = build_connection_resolver(
            RunId::new("resolver-plan"),
            resolver_wire(
                "https://resolver.example/connections",
                "private-token",
                keys,
                source_connection,
            ),
        )
        .err()
        .expect("invalid dynamic key authority must fail closed");
        assert_eq!(error.message(), "connection resolver keys are invalid");
    }
}

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
