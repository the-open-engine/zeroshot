use super::*;

const MESSAGE: &str = "Something went wrong while executing your query on 2026-11-21T18:29:14Z. Please include `2790:35FB30:99F13F9:E90DBDA:6B01E2F9` when reporting this issue.";
const QUERY: &str = "query { repository }";
const CURSOR: &[&str] = &["data", "repository", "pageInfo"];

fn query_error(message: &str) -> Value {
    json!({"errors":[{"message": message}]})
}

fn page(has_next: bool, marker: &str) -> Value {
    json!({"data":{"repository":{
        "marker": marker,
        "pageInfo": {"hasNextPage": has_next, "endCursor": "next-page"},
    }}})
}

async fn policy_read(
    authority: &GhCliDeliveryAuthority,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Value, SourceDeliveryError> {
    authority
        .graphql_pages(
            GraphqlPages {
                document: QUERY,
                cursor_path: CURSOR,
                read: GraphqlRead::Policy,
            },
            json!({}),
            credential,
        )
        .await
}

#[test]
fn policy_read_exhaustion_is_typed_and_survives_error_context_and_retry_hints() {
    for status in [None, Some(503)] {
        let error = SourceDeliveryError::api(status, "read failed")
            .exhausted()
            .with_context("mutation failed")
            .temporary()
            .temporary_review_sync();
        assert!(error.retries_exhausted());
        assert!(!error.retryable_operation());
        assert!(!error.retryable_review_sync());
        assert_eq!(error.api_status(), status);
    }
    let error = SourceDeliveryError::api(Some(503), "retry allowance exhausted");
    assert!(!error.retries_exhausted());
    assert!(error.retryable_operation());
}

#[tokio::test]
async fn policy_read_classification_requires_complete_unmixed_graphql_evidence() {
    let failed = query_error(MESSAGE);
    for value in [
        Value::Null,
        json!([]),
        json!([failed, failed]),
        json!({"errors":[]}),
        query_error("unknown"),
        json!({"errors":[{"message":MESSAGE},{"message":"permission denied"}]}),
        json!({"data":null,"errors":[{"message":MESSAGE}]}),
        json!({"data":{"repository":{}},"errors":[{"message":MESSAGE}]}),
        json!({"errors":[{"message":MESSAGE,"type":"UNPROCESSABLE"}]}),
        json!({"errors":[{"message":MESSAGE}],"status":503}),
        query_error("Something went wrong while executing your query"),
        query_error(
            "Something went wrong while executing your query on . Please include `id` when reporting this issue.",
        ),
        query_error(
            "Something went wrong while executing your query on date. Please include `` when reporting this issue.",
        ),
        query_error(
            "Something went wrong while executing your query on date. Please include `id` when reporting this issue. extra",
        ),
        query_error(
            "Something went wrong while executing your query on date. Please include `id\nextra` when reporting this issue.",
        ),
    ] {
        let fixture = ApiFixture::serve(move |_, _| Json(value.clone()).into_response()).await;
        let error = policy_read(&fixture.authority(), credential())
            .await
            .unwrap_err();
        assert!(!error.retries_exhausted());
        assert!(!error.retryable_operation());
        assert_eq!(fixture.calls(), 1);
    }
}

#[tokio::test]
async fn generic_graphql_execution_errors_only_retry_policy_reads() {
    let fixture = ApiFixture::serve(move |_, _| Json(query_error(MESSAGE)).into_response()).await;
    let error = fixture
        .authority()
        .graphql(QUERY, &json!({}), credential())
        .await
        .unwrap_err();
    assert_eq!(error.api_status(), None);
    assert!(!error.retryable_operation());
    assert!(!error.retries_exhausted());
    assert_eq!(fixture.calls(), 1);
}

#[tokio::test]
async fn generic_error_on_later_page_restarts_policy_read_from_the_first_cursor() {
    let fixture = ApiFixture::serve(move |index, request| {
        let request: Value = serde_json::from_slice(&request.body).unwrap();
        let cursor = &request["variables"]["endCursor"];
        let result = match index {
            0 => {
                assert!(cursor.is_null());
                page(true, "obsolete page")
            }
            1 => {
                assert_eq!(cursor, "next-page");
                query_error(&MESSAGE.replace("2026-11-21T18:29:14Z", "2027-01-02T03:04:05Z"))
            }
            2 => {
                assert!(cursor.is_null());
                page(true, "new first page")
            }
            3 => {
                assert_eq!(cursor, "next-page");
                page(false, "new second page")
            }
            _ => panic!("unexpected policy retry"),
        };
        Json(result).into_response()
    })
    .await;
    assert_eq!(
        policy_read(&fixture.authority(), credential())
            .await
            .unwrap(),
        json!([page(true, "new first page"), page(false, "new second page")])
    );
    assert_eq!(fixture.calls(), 4);
}

#[tokio::test]
async fn exhausted_policy_read_keeps_the_last_bounded_redacted_diagnostic() {
    let fixture = ApiFixture::serve(move |_, _| Json(query_error(MESSAGE)).into_response()).await;
    let token = "2790:35FB30";
    let error = policy_read(&fixture.authority(), SourceDeliveryCredential::token(token))
        .await
        .unwrap_err();
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("exhausted after 3 attempts"));
    assert!(diagnostic.contains("Something went wrong while executing"));
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains(token));
    assert!(error.retries_exhausted());
    assert!(!error.retryable_operation());
    assert!(!error.retryable_review_sync());
    assert_eq!(fixture.calls(), 3);
}

#[tokio::test]
async fn policy_retry_spends_one_deadline_and_retains_the_interrupted_reply_diagnostic() {
    let fixture = ApiFixture::serve(move |_, _| {
        let body = format!("Bad credentials {TOKEN}");
        Response::raw(body.clone().into_bytes())
            .status(StatusCode::UNAUTHORIZED)
            .header("x-github-request-id", "fixture-id")
            .content_length(body.len() + 1)
    })
    .await;
    let authority = authority(Duration::from_millis(500)).with_test_api(&fixture.origin);
    let error = policy_read(&authority, credential()).await.unwrap_err();
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("at the API deadline"), "{diagnostic}");
    assert!(diagnostic.contains("partial response: Bad credentials [REDACTED]"));
    assert!(diagnostic.contains("fixture-id"));
    assert!(!diagnostic.contains(TOKEN));
    assert!(error.retries_exhausted());
    assert_eq!(error.api_status(), None);
    assert!(!error.authentication_failed());
    assert!(!error.retryable_operation());
    assert_eq!(fixture.calls(), 1);
}

#[tokio::test]
async fn malformed_and_oversized_policy_error_bodies_do_not_supply_retry_authority() {
    for oversized in [false, true] {
        let fixture = ApiFixture::serve(move |_, _| {
            let mut body = serde_json::to_vec(&query_error(MESSAGE)).unwrap();
            if oversized {
                Response::raw(body).content_length(MAX_JOB_LOG_BYTES + 1)
            } else {
                body.push(0xff);
                Response::raw(body)
            }
        })
        .await;
        let error = policy_read(&fixture.authority(), credential())
            .await
            .unwrap_err();
        assert!(!error.retries_exhausted());
        assert_eq!(error.api_status(), None);
        assert!(!error.retryable_operation());
        assert_eq!(fixture.calls(), 1);
    }
}

#[tokio::test]
async fn initial_policy_timeout_stays_retryable_without_claiming_exhaustion() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    // The bound listener allows a connection but never provides a response.
    let authority = authority(Duration::from_millis(100)).with_test_api(&origin);
    let error = policy_read(&authority, credential()).await.unwrap_err();
    drop(listener);

    assert!(error.to_string().contains("timed out at the API deadline"));
    assert_eq!(error.api_status(), None);
    assert!(error.retryable_operation());
    assert!(!error.retries_exhausted());
}
