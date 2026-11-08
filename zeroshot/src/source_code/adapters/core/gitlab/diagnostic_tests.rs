use super::*;

const PROJECT: &str = "/projects/group%2Fsub%2Fproject";

fn rejection_cases() -> impl IntoIterator<
    Item = (
        &'static str,
        &'static str,
        StatusCode,
        Value,
        Option<&'static str>,
    ),
> {
    [
        (
            "POST",
            "/merge_requests",
            StatusCode::BAD_REQUEST,
            json!({"message":{"title":["is too long (maximum is 255 characters)"]}}),
            Some("is too long (maximum is 255 characters)"),
        ),
        (
            "POST",
            "/merge_requests",
            StatusCode::BAD_REQUEST,
            json!({"message":["source_branch does not exist"]}),
            Some("source_branch does not exist"),
        ),
        (
            "POST",
            "/merge_requests",
            StatusCode::FORBIDDEN,
            json!({
                "error":"insufficient_scope",
                "error_description":"The request requires higher privileges than provided by the access token.",
                "scope":"api",
            }),
            Some("insufficient_scope"),
        ),
        (
            "GET",
            PROJECT,
            StatusCode::NOT_FOUND,
            json!({"message":"404 Project Not Found"}),
            Some("404 Project Not Found"),
        ),
        (
            "GET",
            PROJECT,
            StatusCode::MOVED_PERMANENTLY,
            Value::Null,
            None,
        ),
        (
            "GET",
            PROJECT,
            StatusCode::UNAUTHORIZED,
            json!({"message":"401 Unauthorized"}),
            Some("401 Unauthorized"),
        ),
        // A forge or proxy that echoes the bearer value must not leak it.
        (
            "GET",
            PROJECT,
            StatusCode::BAD_REQUEST,
            json!({"message":"proxy refused Authorization: Bearer test-private-token"}),
            Some("proxy refused Authorization"),
        ),
        (
            "GET",
            PROJECT,
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"message":"x".repeat(2 * 1024 * 1024)}),
            None,
        ),
    ]
}

#[tokio::test]
async fn api_rejections_keep_gitlab_reason_and_requested_resource() {
    for (method, suffix, status, body, reason) in rejection_cases() {
        let fixture = Fixture::new().await;
        fixture.response(method, suffix, (status, body));
        let error = fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err();
        let text = error.to_string();
        assert_eq!(error.api_status(), Some(status.as_u16()), "{text}");
        assert_eq!(
            error.retryable_operation(),
            status.is_server_error(),
            "{text}"
        );
        assert_eq!(
            error.authentication_failed(),
            status == StatusCode::UNAUTHORIZED,
            "{text}"
        );
        if let Some(reason) = reason {
            assert!(text.contains(reason), "GitLab's reason is missing: {text}");
        }
        assert!(
            text.contains("group%2Fsub%2Fproject") || text.contains(REPOSITORY),
            "requested project is missing: {text}"
        );
        if suffix == "/merge_requests" {
            assert!(text.contains("merge_requests"), "{text}");
        }
        assert!(!text.contains("test-private-token"), "{text}");
        assert!(
            text.len() <= MAX_SOURCE_API_DIAGNOSTIC_BYTES + 64,
            "{}",
            text.len()
        );
    }
}

#[tokio::test]
async fn refused_connection_names_the_transport_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = listener.local_addr().unwrap();
    drop(listener);
    let mut authority =
        GitLabDeliveryAuthority::new(SourceForgeBaseUrl::new(ORIGIN).unwrap(), None).unwrap();
    authority.api = Api::loopback(&format!("http://{closed}/api/v4/")).unwrap();
    let error = authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap_err();
    let text = error.to_string();
    assert_eq!(error.api_status(), None, "{text}");
    assert!(error.retryable_operation(), "{text}");
    assert!(
        text.to_ascii_lowercase().contains("refused"),
        "transport failure kind is missing: {text}"
    );
    assert!(!text.contains("test-private-token"), "{text}");
}

#[tokio::test]
async fn lost_creation_keeps_the_original_failure() {
    let fixture = Fixture::new().await;
    fixture.response(
        "POST",
        "/merge_requests",
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"message":"503 upstream busy"}),
        ),
    );
    let error = fixture
        .authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("creation response was lost"), "{text}");
    assert!(
        text.contains("503 upstream busy"),
        "the original failure is missing: {text}"
    );
    assert_eq!(error.api_status(), None, "{text}");
    assert!(!error.retryable_operation(), "{text}");
}

async fn rejected_resource(
    token: &'static str,
    segment: &str,
    expected_path: &'static str,
    body: Value,
) -> SourceDeliveryError {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/api/v4/", listener.local_addr().unwrap());
    let server = tokio::spawn(http_fixture::serve(listener, move |request| {
        assert_eq!(request.uri.path(), expected_path);
        assert_eq!(request.headers["authorization"], format!("Bearer {token}"));
        (StatusCode::BAD_REQUEST, Json(body.clone())).into_response()
    }));
    let result = Api::loopback(&endpoint)
        .unwrap()
        .json::<Value>(
            ApiRequest::get(&["projects", segment]).query("search", "query-only-marker"),
            SourceDeliveryCredential::token(token),
        )
        .await;
    server.abort();
    result.unwrap_err()
}

#[tokio::test]
async fn api_rejections_redact_credentials_in_the_requested_resource() {
    for (token, encoded) in [
        ("test-private-token", "test-private-token"),
        ("test/private-token", "test%2Fprivate-token"),
        ("test/private%token", "test%2Fprivate%25token"),
    ] {
        let path = match token {
            "test-private-token" => "/api/v4/projects/test-private-token",
            "test/private-token" => "/api/v4/projects/test%2Fprivate-token",
            _ => "/api/v4/projects/test%2Fprivate%25token",
        };
        let error =
            rejected_resource(token, token, path, json!({"message":"project rejected"})).await;
        let text = error.to_string();
        assert!(text.contains("project rejected"), "{text}");
        assert!(!text.contains(token), "{text}");
        assert!(!text.contains(encoded), "{text}");
        assert!(!text.contains("query-only-marker"), "{text}");
        assert_eq!(error.api_status(), Some(400), "{text}");
        assert!(!error.retryable_operation(), "{text}");
    }
}

#[tokio::test]
async fn truncated_error_bodies_do_not_expose_encoded_credential_prefixes() {
    let encoded = "test%2Fprivate%25token";
    let prefix = "test%2Fprivate%25to";
    let padding = "x".repeat(4 * 1024 - "{\"message\":\"".len() - prefix.len());
    let error = rejected_resource(
        "test/private%token",
        "plain",
        "/api/v4/projects/plain",
        json!({"message":format!("{padding}{encoded} trailing bytes")}),
    )
    .await;
    let text = error.to_string();
    assert!(
        !text.contains(prefix),
        "encoded credential prefix was exposed"
    );
    assert!(text.contains("[truncated]"), "{text}");
    assert_eq!(error.api_status(), Some(400));
    assert!(!error.retryable_operation());
}

#[tokio::test]
async fn truncated_error_bodies_do_not_expose_partial_utf8_credentials() {
    // The retained body ends after the first byte of the credential's multibyte character.
    let partial = b"test-\xc3";
    let padding = "x".repeat(4 * 1024 - "{\"message\":\"".len() - partial.len());
    let error = rejected_resource(
        "test-é-private-token",
        "plain",
        "/api/v4/projects/plain",
        json!({"message":format!("{padding}test-é-private-token trailing bytes")}),
    )
    .await;
    let text = error.to_string();
    assert!(
        !text.contains("test-"),
        "partial UTF8 credential was exposed"
    );
    assert!(text.contains("[truncated]"), "{text}");
    assert_eq!(error.api_status(), Some(400));
    assert!(!error.retryable_operation());
}
