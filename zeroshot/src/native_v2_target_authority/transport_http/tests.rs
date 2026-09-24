use openengine_cluster_testkit::assertions::AssertValue;
use serde::ser::Error as _;
use tokio::io::AsyncWriteExt as _;

use super::*;

fn decode_problem(response: HttpResponse) -> TargetHttpProblem {
    assert_eq!(response.content_type, Some("application/json"));
    assert!(response.no_store);
    serde_json::from_slice(&response.body).assert_value()
}

fn assert_invalid<T>(result: io::Result<T>) {
    match result {
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::InvalidData),
        Ok(_) => panic!("request unexpectedly passed validation"),
    }
}

#[test]
fn authority_failures_are_structured_and_status_specific() {
    let cases = [
        (
            TargetAuthorityError::invalid("invalid target request"),
            400,
            INVALID_REQUEST_CODE,
            "invalid target request",
        ),
        (
            TargetAuthorityError::unauthorized(),
            401,
            UNAUTHORIZED_CODE,
            "unauthorized",
        ),
        (
            TargetAuthorityError::conflict("run already exists"),
            409,
            CONFLICT_CODE,
            "run already exists",
        ),
        (
            TargetAuthorityError::unavailable("internal provider failure"),
            503,
            TARGET_UNAVAILABLE_CODE,
            "target is temporarily unavailable",
        ),
    ];

    for (error, status, code, message) in cases {
        let response = authority_error_response(error);
        assert_eq!(response.status, status);
        let problem = decode_problem(response);
        assert_eq!(problem.code(), code);
        assert_eq!(problem.message(), message);
    }
}

#[test]
fn invalid_public_diagnostic_fails_closed_without_an_empty_body() {
    let response = authority_error_response(TargetAuthorityError::invalid("x".repeat(1_025)));
    assert_eq!(response.status, 400);
    let problem = decode_problem(response);
    assert_eq!(problem.code(), TARGET_INTERNAL_ERROR_CODE);
    assert_eq!(problem.message(), "target request failed");
}

#[test]
fn request_head_parser_enforces_unique_authorization_and_route_boundaries() {
    let duplicate = parse_request_head(
        b"GET / HTTP/1.1\r\nAuthorization: Bearer first\r\nAuthorization: Bearer second\r\n\r\n",
    )
    .assert_value()
    .assert_value();
    assert!(duplicate.bearer().is_err());

    for target in ["relative", "/api?query=true", "/api#fragment"] {
        let encoded = format!("GET {target} HTTP/1.1\r\n\r\n");
        assert_invalid(parse_request_head(encoded.as_bytes()));
    }
    assert!(parse_request_head(b"GET /ui?run=one HTTP/1.1\r\n\r\n").is_ok());
    assert!(
        parse_request_head(b"GET / HTTP/1.1\r\nheader")
            .assert_value()
            .is_none()
    );

    for authorization in ["Basic token", "Bearer ", "Bearer token with spaces"] {
        let encoded = format!("GET / HTTP/1.1\r\nAuthorization: {authorization}\r\n\r\n");
        let head = parse_request_head(encoded.as_bytes())
            .assert_value()
            .assert_value();
        assert!(head.bearer().is_err());
    }
}

async fn stream_pair() -> (TcpStream, TcpStream) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .assert_value();
    let address = listener.local_addr().assert_value();
    let client = TcpStream::connect(address);
    let server = listener.accept();
    let (client, accepted) = tokio::join!(client, server);
    (client.assert_value(), accepted.assert_value().0)
}

async fn connected_stream() -> TcpStream {
    let (client, server) = stream_pair().await;
    drop(server);
    client
}

#[tokio::test]
async fn request_body_reader_rejects_ambiguous_or_oversized_framing_before_io() {
    for encoded in [
        "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
        "POST / HTTP/1.1\r\nContent-Length: invalid\r\n\r\n",
        &format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_PRIVATE_REQUEST_BYTES + 1
        ),
    ] {
        let head = parse_request_head(encoded.as_bytes())
            .assert_value()
            .assert_value();
        let mut stream = connected_stream().await;
        assert_invalid(read_http_request(&mut stream, head).await);
    }
}

#[tokio::test]
async fn request_peek_and_empty_body_boundaries_are_exact() {
    let (peer, reader) = stream_pair().await;
    drop(peer);
    match peek_request_head(&reader).await {
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof),
        Ok(_) => panic!("closed connection unexpectedly contained a request"),
    }

    let (mut peer, reader) = stream_pair().await;
    let mut oversized = b"GET / HTTP/1.1\r\nX-Fill: ".to_vec();
    oversized.resize(MAX_HEADER_BYTES, b'a');
    peer.write_all(&oversized).await.assert_value();
    assert_invalid(peek_request_head(&reader).await);

    let encoded = b"POST /native-v2/run HTTP/1.1\r\nHost: local\r\n\r\n";
    let head = parse_request_head(encoded).assert_value().assert_value();
    let (mut peer, mut reader) = stream_pair().await;
    peer.write_all(encoded).await.assert_value();
    let request = read_http_request(&mut reader, head).await.assert_value();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/native-v2/run");
    assert!(request.body.is_empty());
}

struct SerializationFailure;

impl Serialize for SerializationFailure {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(S::Error::custom("intentional response encoding failure"))
    }
}

#[test]
fn response_fallbacks_and_status_vocabulary_are_complete() {
    let response = HttpResponse::json(200, &SerializationFailure);
    assert_eq!(response.status, 500);
    let problem = decode_problem(response);
    assert_eq!(problem.code(), TARGET_INTERNAL_ERROR_CODE);

    let timeout = request_timeout_response();
    assert_eq!(timeout.status, 408);
    assert_eq!(decode_problem(timeout).code(), REQUEST_TIMEOUT_CODE);

    let unavailable = run_error_response(TargetAuthorityError::unavailable("private"));
    assert_eq!(unavailable.status, 503);
    assert_eq!(decode_problem(unavailable).code(), TARGET_UNAVAILABLE_CODE);

    for (status, reason) in [
        (200, "OK"),
        (400, "Bad Request"),
        (401, "Unauthorized"),
        (404, "Not Found"),
        (408, "Request Timeout"),
        (409, "Conflict"),
        (503, "Service Unavailable"),
        (500, "Internal Server Error"),
    ] {
        assert_eq!(http_reason(status), reason);
    }
}
