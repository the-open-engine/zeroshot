use super::*;

const PROJECT: &str = "/projects/group%2Fsub%2Fproject";

#[test]
fn object_credentials_are_redacted_before_error_and_context_bounds() {
    use crate::native_v2_delivery::git_auth;

    let token = "glpat-synthetic-review-secret-value";
    let password = "https-synthetic-response-secret";
    let encoded = git_auth::encode_user_password("git-user", password);
    let credential = SourceDeliveryCredential::token(token).with_git(SourceGitCredential::Https {
        username: "git-user",
        password,
    });
    let context = "GitLab merge request group/sub/project!17";
    let cutoff = MAX_SOURCE_API_DIAGNOSTIC_BYTES - "\n[diagnostic truncated]".len();
    for secret in [token, password, &encoded] {
        for offset in [0, context.len() + 1] {
            let prefix = &secret[..secret.len() / 2];
            let padding = "x".repeat(cutoff - offset - prefix.len());
            let error = super::super::api::identity_error(
                format!("{padding}{secret} trailing response text"),
                credential,
            );
            for error in [
                error.clone(),
                super::super::api::delivery_context(error, context.to_owned(), credential),
            ] {
                let text = error.to_string();
                assert!(
                    !text.contains(prefix),
                    "credential fragment survived the cutoff"
                );
                assert!(text.len() <= MAX_SOURCE_API_DIAGNOSTIC_BYTES + 64);
                assert!(matches!(error, SourceDeliveryError::Identity(_)));
                assert!(!error.authentication_failed());
                assert!(!error.retryable_review_sync());
                assert!(!error.retryable_operation());
            }
        }
    }
}

#[tokio::test]
async fn closed_and_merged_reviews_name_the_review_and_distinct_resume_steps() {
    for (state, expected) in [("closed", "reopen it"), ("merged", "start a new run")] {
        let fixture = Fixture::new().await;
        fixture.open_review();
        fixture.state.lock().unwrap().review.as_mut().unwrap()["state"] = json!(state);
        let error = fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err();
        let text = error.to_string();
        for detail in [REPOSITORY, "!17", "zeroshot/test", "main", HEAD, expected] {
            assert!(text.contains(detail), "{detail}: {text}");
        }
        assert!(
            text.contains(if state == "closed" {
                "was closed"
            } else {
                "already merged"
            }),
            "{text}"
        );
        assert!(!text.contains("Identity("), "{text}");
        assert_eq!(fixture.writes("/merge_requests/17"), 0);
    }
}

#[tokio::test]
async fn merge_401_distinguishes_missing_permission_from_a_rejected_token() {
    for authenticated in [true, false] {
        let fixture = Fixture::new().await;
        fixture.open_review();
        fixture.response(
            "PUT",
            "/merge_requests/17/merge",
            (
                StatusCode::UNAUTHORIZED,
                json!({"message":"401 Unauthorized test-private-token"}),
            ),
        );
        if !authenticated {
            fixture.response(
                "GET",
                "/user",
                (StatusCode::UNAUTHORIZED, json!({"message":"expired token"})),
            );
        }
        let error = fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .unwrap_err()
            .with_context("delivery refused");
        assert_eq!(error.api_status(), Some(401));
        assert_eq!(error.authentication_failed(), !authenticated, "{error}");
        assert!(!error.retryable_operation());
        assert!(!error.retryable_review_sync());
        let text = error.to_string();
        assert!(!text.contains("test-private-token"), "{text}");
        assert_eq!(
            text.contains("merge permission is missing"),
            authenticated,
            "{text}"
        );
        for detail in [REPOSITORY, "!17", "zeroshot/test", "main", HEAD, "HTTP 401"] {
            assert!(text.contains(detail), "{text}");
        }
        assert_eq!(fixture.writes("/merge_requests/17/merge"), 1);
        let withdrawal = fixture
            .authority
            .withdraw_merge_request(&receipt(), credential())
            .await
            .unwrap();
        assert_eq!(withdrawal, SourceMergeWithdrawal::Withdrawn);
    }
}

#[tokio::test]
async fn deleted_target_branch_names_the_project_and_branch() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.response(
        "GET",
        "/repository/branches/main",
        (StatusCode::NOT_FOUND, Value::Null),
    );
    let workspace = tempfile::tempdir().unwrap();
    let request = request();
    let target_error = fixture
        .authority
        .reconcile_delivery_target(
            SourceTargetReconciliation {
                workspace: workspace.path(),
                target: &request.target,
                commit_message: "Integrate target",
            },
            credential(),
        )
        .await
        .unwrap_err();
    let conflict_error = fixture
        .authority
        .materialize_merge_conflict(
            &SourceConflictRequest {
                workspace: workspace.path().to_owned(),
                review: receipt(),
            },
            credential(),
        )
        .await
        .unwrap_err();
    for error in [target_error, conflict_error] {
        let text = error.to_string();
        for detail in [REPOSITORY, "main", "Restore", "resuming"] {
            assert!(
                text.to_lowercase().contains(&detail.to_lowercase()),
                "{text}"
            );
        }
        assert!(matches!(error, SourceDeliveryError::Identity(_)));
    }
}

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
    assert!(!error.retryable_review_sync(), "{text}");
}

#[tokio::test]
async fn only_missing_source_branch_validation_retries_creation_within_review_sync() {
    for (body, retry) in [
        (json!({"message":["source_branch does not exist"]}), true),
        (json!({"message":"Source branch does not exist"}), true),
        (
            json!({"message":{"source_branch":["does not exist"]}}),
            true,
        ),
        (json!({"message":{"title":["is too long"]}}), false),
        (json!({"message":["target_branch does not exist"]}), false),
        (
            json!({"message":["source_branch does not exist", "title is too long"]}),
            false,
        ),
        (
            json!({"message":{"source_branch":["does not exist"],"title":["is too long"]}}),
            false,
        ),
        (
            json!({"message":"policy mentions source_branch does not exist"}),
            false,
        ),
        (json!({"message":[]}), false),
        (json!({"message":{"source_branch":[]}}), false),
    ] {
        let fixture = Fixture::new().await;
        fixture.response(
            "POST",
            "/merge_requests",
            (StatusCode::BAD_REQUEST, body.clone()),
        );
        let error = fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), Some(400), "{body}: {error}");
        assert_eq!(error.retryable_review_sync(), retry, "{body}: {error}");
        assert!(!error.retryable_operation(), "{body}: {error}");
        if retry {
            assert!(
                error.to_string().contains(&request().head_branch),
                "{error}"
            );
        }
        assert_eq!(
            error
                .with_context("synchronizing review")
                .retryable_review_sync(),
            retry
        );
    }
}

#[tokio::test]
async fn missing_branch_text_on_other_resources_does_not_enable_review_sync_retries() {
    for (method, suffix) in [("GET", PROJECT), ("PUT", "/merge_requests/17")] {
        let fixture = Fixture::new().await;
        if method == "PUT" {
            fixture.open_review();
        }
        fixture.response(
            method,
            suffix,
            (
                StatusCode::BAD_REQUEST,
                json!({"message":["source_branch does not exist"]}),
            ),
        );
        let mut request = request();
        request.title = "Updated title".into();
        let error = fixture
            .authority
            .open_or_update_review(&request, credential())
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), Some(400));
        assert!(!error.retryable_review_sync(), "{error}");
        assert!(!error.retryable_operation(), "{error}");
    }
}

#[tokio::test]
async fn missing_branch_context_redacts_credentials() {
    let fixture = Fixture::new().await;
    let mut request = request();
    request.head_branch = "test-private-token".into();
    fixture.response(
        "GET",
        "/repository/branches/test-private-token",
        (
            StatusCode::OK,
            json!({"name":request.head_branch,"commit":{"id":HEAD}}),
        ),
    );
    fixture.response(
        "POST",
        "/merge_requests",
        (
            StatusCode::BAD_REQUEST,
            json!({"message":["source_branch does not exist"]}),
        ),
    );
    let error = fixture
        .authority
        .open_or_update_review(&request, credential())
        .await
        .unwrap_err();
    assert!(error.retryable_review_sync(), "{error}");
    assert!(!error.to_string().contains("test-private-token"), "{error}");
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
