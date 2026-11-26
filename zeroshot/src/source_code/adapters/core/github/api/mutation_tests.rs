use super::*;

const ENQUEUE: &str = "enqueuePullRequest";
const MERGE: &str = "mergePullRequest";
const CONVERSATION: &str =
    "A conversation must be resolved before this pull request can be merged.";
const QUEUED: &str = "Required status check \"Build / Linux (🚀)\" is queued.";

fn rejected(field: &str, message: &str) -> Value {
    json!({
        "data": {(field): null},
        "errors": [{"type":"UNPROCESSABLE", "path":[field], "message": message}],
    })
}

async fn failure(authority: &GhCliDeliveryAuthority, field: &str, token: &str) -> MutationFailure {
    authority
        .mutation(
            ("mutation { fixtureMutation }", field),
            &json!({}),
            SourceDeliveryCredential::token(token),
        )
        .await
        .unwrap_err()
}

#[tokio::test]
async fn complete_graphql_rejection_is_classified_before_diagnostic_redaction() {
    for (field, message, expected) in [
        (ENQUEUE, "Policy refused", MutationRejection::Policy),
        (ENQUEUE, QUEUED, MutationRejection::RequiredCheckQueued),
        (ENQUEUE, CONVERSATION, MutationRejection::Conversation),
        (MERGE, CONVERSATION, MutationRejection::Conversation),
        (MERGE, QUEUED, MutationRejection::Policy),
    ] {
        let fixture =
            ApiFixture::serve(move |_, _| Json(rejected(field, message)).into_response()).await;
        let authority = fixture.authority();
        for token in [TOKEN, "is queued", "conversation", "UNPROCESSABLE", field] {
            let failure = failure(&authority, field, token).await;
            assert_eq!(failure.rejection, Some(expected), "{field}: {message}");
            assert_eq!(failure.error.api_status(), None);
            assert!(!failure.error.retryable_operation());
            assert!(!failure.error.to_string().contains(token));
        }
    }
}

#[tokio::test]
async fn graphql_rejection_requires_null_result_and_exact_rooted_error_types() {
    for field in [ENQUEUE, MERGE] {
        let rejected = rejected(field, CONVERSATION);
        let mut nested = rejected.clone();
        nested["errors"][0]["path"] = json!([field, "child"]);
        let mut wrong_path = rejected.clone();
        wrong_path["errors"][0]["path"] = json!(["unknownMutation"]);
        let mut partial = rejected.clone();
        partial["data"][field] = json!({"child": null});
        let mut unexpected_data = rejected.clone();
        unexpected_data["data"]["anotherMutation"] = json!(null);
        let mut wrong_type = rejected.clone();
        wrong_type["errors"][0]["type"] = json!("GRAPHQL_VALIDATION_FAILED");
        let mut mixed = rejected.clone();
        mixed["errors"]
            .as_array_mut()
            .unwrap()
            .push(wrong_type["errors"][0].clone());
        let mut empty = rejected.clone();
        empty["errors"] = json!([]);
        for response in [
            nested,
            wrong_path,
            partial,
            unexpected_data,
            wrong_type,
            mixed,
            empty,
            json!({"errors":rejected["errors"]}),
        ] {
            let fixture =
                ApiFixture::serve(move |_, _| Json(response.clone()).into_response()).await;
            assert_eq!(
                failure(&fixture.authority(), field, TOKEN).await.rejection,
                None
            );
        }
    }
}

#[tokio::test]
async fn queued_check_exception_requires_the_entire_exact_enqueue_reason() {
    for message in [
        "Required status check \"semantic\" failed.",
        "Required status check \"\" is queued.",
        "Required status check \"semantic\" is queued. Policy refused",
        "Policy refused: Required status check \"semantic\" is queued.",
    ] {
        let fixture =
            ApiFixture::serve(move |_, _| Json(rejected(ENQUEUE, message)).into_response()).await;
        let failure = failure(&fixture.authority(), ENQUEUE, TOKEN).await;
        assert_eq!(
            failure.rejection,
            Some(MutationRejection::Policy),
            "{message}"
        );
    }
    let fixture = ApiFixture::serve(move |_, _| {
        let mut response = rejected(ENQUEUE, QUEUED);
        response["errors"]
            .as_array_mut()
            .unwrap()
            .push(rejected(ENQUEUE, "Policy refused")["errors"][0].clone());
        Json(response).into_response()
    })
    .await;
    let failure = failure(&fixture.authority(), ENQUEUE, TOKEN).await;
    assert_eq!(failure.rejection, Some(MutationRejection::Policy));
}

#[tokio::test]
async fn partial_oversized_or_unparseable_mutation_response_proves_no_rejection() {
    for kind in ["eof", "oversize", "invalid utf8", "trailing garbage"] {
        let fixture = ApiFixture::serve(move |_, _| {
            let mut bytes = serde_json::to_vec(&rejected(ENQUEUE, CONVERSATION)).unwrap();
            match kind {
                "eof" => {
                    let length = bytes.len();
                    Response::raw(bytes).content_length(length + 1)
                }
                "oversize" => Response::raw(bytes).content_length(MAX_JOB_LOG_BYTES + 1),
                "invalid utf8" => {
                    let index = bytes.iter().position(|byte| *byte == b'A').unwrap();
                    bytes[index] = 0xff;
                    Response::raw(bytes)
                }
                _ => {
                    bytes.extend_from_slice(b" trailing garbage");
                    Response::raw(bytes)
                }
            }
        })
        .await;
        let failure = failure(&fixture.authority(), ENQUEUE, TOKEN).await;
        assert_eq!(failure.rejection, None, "{kind}");
        assert_eq!(failure.error.api_status(), None);
        assert!(!failure.error.authentication_failed());
        assert!(!failure.error.retryable_operation());
    }
}

#[tokio::test]
async fn mutation_uses_complete_http_status_without_trusting_a_graphql_error_body() {
    for (status, interrupted, expected_status) in [
        (StatusCode::UNAUTHORIZED, false, Some(401)),
        (StatusCode::UNAUTHORIZED, true, None),
        (StatusCode::SERVICE_UNAVAILABLE, false, Some(503)),
        (StatusCode::SERVICE_UNAVAILABLE, true, None),
    ] {
        let fixture = ApiFixture::serve(move |_, _| {
            let mut body = rejected(MERGE, CONVERSATION);
            body["echo"] = json!({ "token": TOKEN, "encoded": encode_basic_credential(TOKEN) });
            let bytes = serde_json::to_vec(&body).unwrap();
            let length = bytes.len();
            let response = Response::raw(bytes).status(status);
            if interrupted {
                response.content_length(length + 1)
            } else {
                response
            }
        })
        .await;
        let failure = failure(&fixture.authority(), MERGE, TOKEN).await;
        assert_eq!(failure.rejection, None);
        assert_eq!(failure.error.api_status(), expected_status);
        assert_eq!(
            failure.error.authentication_failed(),
            expected_status == Some(401)
        );
        assert!(!failure.error.to_string().contains(TOKEN));
        assert!(
            !failure
                .error
                .to_string()
                .contains(&encode_basic_credential(TOKEN))
        );
    }
}
