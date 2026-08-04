use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, DefaultBodyLimit, Path, State},
    http::{header::CACHE_CONTROL, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::put,
    Json, Router,
};
use openengine_cluster_protocol::LegacyShipRequest;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    credentials::CredentialBundle,
    run_intent_executor::{
        RunIntentExecutor, RunIntentIdentity, RunIntentLookup, RunIntentStatus, RunIntentSubmission,
    },
};

pub(super) const MAX_RUN_INTENT_BYTES: usize = 10 * 1_024 * 1_024 + 64 * 1_024;
const RUN_INTENT_DIGEST_HEADER: &str = "x-zero-run-intent-digest";
const RUN_INTENT_VERSION: &str = "zeroshot.run-intent/v1";
type RunIntentHttpError = (StatusCode, &'static str);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunIntent {
    version: String,
    credentials: CredentialBundle,
    request: LegacyShipRequest,
}

pub(super) fn router(executor: Arc<dyn RunIntentExecutor>) -> Router {
    Router::new()
        .route(
            "/internal/run-intents/{intent_id}",
            put(put_run_intent).get(get_run_intent),
        )
        .layer(DefaultBodyLimit::max(MAX_RUN_INTENT_BYTES))
        .with_state(executor)
}

async fn put_run_intent(
    State(executor): State<Arc<dyn RunIntentExecutor>>,
    Path(intent_id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let identity = match run_intent_identity(&intent_id, &headers) {
        Ok(identity) => identity,
        Err((status, code)) => return error_response(status, code),
    };
    let intent = match decode_run_intent(body, identity.digest()) {
        Ok(intent) => intent,
        Err((status, code)) => return error_response(status, code),
    };
    match executor
        .submit(RunIntentSubmission::new(
            identity,
            intent.credentials,
            intent.request,
        ))
        .await
    {
        Ok(status) => status_response(status),
        Err(()) => error_response(StatusCode::CONFLICT, "intent_conflict"),
    }
}

fn run_intent_identity(
    intent_id: &str,
    headers: &HeaderMap,
) -> Result<RunIntentIdentity, RunIntentHttpError> {
    let intent_id =
        canonical_uuid(intent_id).ok_or((StatusCode::BAD_REQUEST, "invalid_intent_id"))?;
    let digest = intent_digest(headers).ok_or((StatusCode::BAD_REQUEST, "invalid_digest"))?;
    Ok(RunIntentIdentity::new(intent_id, digest))
}

fn decode_run_intent(
    body: Result<Bytes, BytesRejection>,
    digest: &str,
) -> Result<RunIntent, RunIntentHttpError> {
    let body = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            (StatusCode::PAYLOAD_TOO_LARGE, "intent_too_large")
        } else {
            (StatusCode::BAD_REQUEST, "invalid_body")
        }
    })?;
    if digest_bytes(&body) != digest {
        return Err((StatusCode::CONFLICT, "digest_mismatch"));
    }
    let intent: RunIntent = serde_json::from_slice(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid_run_intent"))?;
    if intent.version != RUN_INTENT_VERSION || intent.credentials.validate().is_err() {
        return Err((StatusCode::BAD_REQUEST, "invalid_run_intent"));
    }
    Ok(intent)
}

async fn get_run_intent(
    State(executor): State<Arc<dyn RunIntentExecutor>>,
    Path(intent_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(intent_id) = canonical_uuid(&intent_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_intent_id");
    };
    let Some(digest) = intent_digest(&headers) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_digest");
    };
    let identity = RunIntentIdentity::new(intent_id, digest);
    match executor.lookup(&identity).await {
        RunIntentLookup::Found(status) => status_response(status),
        RunIntentLookup::NotFound => error_response(StatusCode::NOT_FOUND, "intent_not_found"),
        RunIntentLookup::Conflict => error_response(StatusCode::CONFLICT, "intent_conflict"),
    }
}

fn status_response(status: RunIntentStatus) -> Response {
    match status {
        RunIntentStatus::Running => {
            json_response(StatusCode::ACCEPTED, json!({ "state": "running" }))
        }
        RunIntentStatus::Succeeded(result) => json_response(
            StatusCode::OK,
            json!({ "state": "succeeded", "result": result }),
        ),
        RunIntentStatus::Failed(error_code) => {
            error_response(StatusCode::UNPROCESSABLE_ENTITY, error_code)
        }
    }
}

fn error_response(status: StatusCode, error_code: &'static str) -> Response {
    json_response(
        status,
        json!({ "state": "failed", "error_code": error_code }),
    )
}

fn json_response(status: StatusCode, body: Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn intent_digest(headers: &HeaderMap) -> Option<String> {
    let mut values = headers.get_all(RUN_INTENT_DIGEST_HEADER).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() || !valid_digest(value) {
        return None;
    }
    Some(value.to_owned())
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn digest_bytes(body: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(body))
}

fn canonical_uuid(value: &str) -> Option<String> {
    let valid = value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        });
    valid.then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use async_trait::async_trait;
    use axum::body::to_bytes;
    use axum::http::HeaderValue;
    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct FakeExecutor {
        submitted: AtomicBool,
    }

    #[async_trait]
    impl RunIntentExecutor for FakeExecutor {
        async fn submit(&self, _submission: RunIntentSubmission) -> Result<RunIntentStatus, ()> {
            self.submitted.store(true, Ordering::SeqCst);
            Ok(RunIntentStatus::Running)
        }

        async fn lookup(&self, _identity: &RunIntentIdentity) -> RunIntentLookup {
            RunIntentLookup::NotFound
        }
    }

    fn golden() -> Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/hosted/run-intent-v1.json"
        ))
        .expect("golden fixture is JSON")
    }

    fn generic_intent() -> Value {
        golden()["envelope"].clone()
    }

    #[test]
    fn run_intent_contract_is_closed_versioned_and_generic() {
        let valid = generic_intent();
        let intent: RunIntent = serde_json::from_value(valid.clone()).expect("fixture is valid");
        assert_eq!(intent.version, RUN_INTENT_VERSION);
        assert!(
            !serde_json::to_string(&valid)
                .expect("fixture serializes")
                .contains("openrouterApiKey")
        );

        let mut open = valid;
        open.as_object_mut()
            .expect("fixture is an object")
            .insert("graph".to_owned(), Value::Null);
        assert!(serde_json::from_value::<RunIntent>(open).is_err());
    }

    #[tokio::test]
    async fn internal_status_responses_match_the_shared_golden_contract() {
        let fixture = golden();
        let cases = [
            (
                RunIntentStatus::Running,
                fixture["runtimeResponses"]["running"].clone(),
            ),
            (
                RunIntentStatus::Succeeded(
                    fixture["runtimeResponses"]["succeeded"]["body"]["result"].clone(),
                ),
                fixture["runtimeResponses"]["succeeded"].clone(),
            ),
            (
                RunIntentStatus::Failed("worker_failed"),
                fixture["runtimeResponses"]["failed"].clone(),
            ),
        ];
        for (status, expected) in cases {
            let response = status_response(status);
            assert_eq!(
                u64::from(response.status().as_u16()),
                expected["status"].as_u64().expect("golden HTTP status")
            );
            let body = to_bytes(response.into_body(), MAX_RUN_INTENT_BYTES)
                .await
                .expect("bounded response body");
            assert_eq!(
                serde_json::from_slice::<Value>(&body).expect("response is JSON"),
                expected["body"]
            );
        }
    }

    #[test]
    fn intent_identity_and_digest_are_canonical() {
        assert!(canonical_uuid("019f7437-8701-71e3-a056-2ba05c37609c").is_some());
        assert!(canonical_uuid("019F7437-8701-71E3-A056-2BA05C37609C").is_none());
        assert!(canonical_uuid("not-a-uuid").is_none());

        let body = b"opaque run intent";
        let digest = digest_bytes(body);
        assert!(valid_digest(&digest));
        let mut headers = HeaderMap::new();
        headers.insert(
            RUN_INTENT_DIGEST_HEADER,
            HeaderValue::from_str(&digest).expect("digest is a header value"),
        );
        assert_eq!(intent_digest(&headers).as_deref(), Some(digest.as_str()));
        assert!(!valid_digest(&format!("sha256:{}", "A".repeat(64))));
    }

    #[tokio::test]
    async fn endpoint_errors_are_bounded_json_and_do_not_reserve_a_run() {
        let fake = Arc::new(FakeExecutor::default());
        let executor: Arc<dyn RunIntentExecutor> = fake.clone();
        let intent_id = "019f7437-8701-71e3-a056-2ba05c37609c";
        let body = Bytes::from_static(br#"{"version":"unknown"}"#);
        let digest = digest_bytes(&body);
        let mut headers = HeaderMap::new();
        headers.insert(
            RUN_INTENT_DIGEST_HEADER,
            HeaderValue::from_str(&digest).expect("digest is a header value"),
        );

        let invalid = put_run_intent(
            State(Arc::clone(&executor)),
            Path(intent_id.to_owned()),
            headers.clone(),
            Ok(body),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(invalid.into_body(), MAX_RUN_INTENT_BYTES)
            .await
            .expect("bounded response body");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).expect("response is JSON"),
            json!({"state": "failed", "error_code": "invalid_run_intent"})
        );

        let missing = get_run_intent(State(executor), Path(intent_id.to_owned()), headers).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert!(!fake.submitted.load(Ordering::SeqCst));
    }
}
