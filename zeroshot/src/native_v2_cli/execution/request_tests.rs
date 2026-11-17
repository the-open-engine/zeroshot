use std::sync::Mutex;

use openengine_cluster_protocol::{
    RequestResolution, RunAttachEventNotification, RunResumeFrom, RunSuspension,
    UnixTimestampMillis,
};
use serde_json::json;

use super::*;
use crate::native_v2_cli::tests::support::FakeSubscription;
use crate::native_v2_cli::{
    CliRunForceResult, CliRunListResult, CliRunStatusResult, PreparedRunRequest, TargetAdd,
};

struct RequestBackend {
    suspension: RunSuspension,
    now: UnixTimestampMillis,
    resumes: Mutex<Vec<RunResumeParams>>,
    resolution: Mutex<Option<RequestResolution>>,
    refusal: Option<&'static str>,
    successor: Option<&'static str>,
}

impl RequestBackend {
    fn new(client_ahead: bool, expired: bool) -> Self {
        let client_now = crate::native_v2_runner::current_timestamp().get();
        let expires_at = if client_ahead {
            client_now - 3_600_000
        } else {
            client_now + 3_600_000
        };
        Self {
            suspension: serde_json::from_value(json!({
                "reason":"request", "node":"approval", "createdAt":expires_at - 60_000,
                "request":{"id":"question", "text":"Approve?",
                    "signals":{"decision":["approved","rejected"]},
                    "output":{"kind":"record","fields":{}}, "expiresAt":expires_at}
            }))
            .expect("request"),
            now: UnixTimestampMillis::new(if expired { expires_at } else { expires_at - 1 })
                .expect("time"),
            resumes: Mutex::default(),
            resolution: Mutex::default(),
            refusal: None,
            successor: None,
        }
    }
}

#[async_trait::async_trait]
impl NativeV2CliBackend for RequestBackend {
    type Watch = FakeSubscription<CliRunWatchEventNotification>;
    type Logs = FakeSubscription<RunLogEventNotification>;
    type Attach = FakeSubscription<RunAttachEventNotification>;

    async fn target_add(&self, _: TargetAdd) -> Result<(), NativeV2CliError> {
        unreachable!()
    }
    async fn target_login(&self, _: &str) -> Result<(), NativeV2CliError> {
        unreachable!()
    }
    async fn run_submit(
        &self,
        _: Option<&str>,
        _: PreparedRunRequest,
    ) -> Result<openengine_cluster_protocol::RunSubmitResult, NativeV2CliError> {
        unreachable!()
    }
    async fn run_list(
        &self,
        _: Option<&str>,
        _: RunListParams,
    ) -> Result<CliRunListResult, NativeV2CliError> {
        unreachable!()
    }
    async fn run_watch(
        &self,
        _: Option<&str>,
        _: RunWatchParams,
    ) -> Result<Self::Watch, NativeV2CliError> {
        unreachable!()
    }
    async fn run_logs(
        &self,
        _: Option<&str>,
        _: RunLogsParams,
    ) -> Result<Self::Logs, NativeV2CliError> {
        unreachable!()
    }
    async fn run_attach(
        &self,
        _: Option<&str>,
        _: RunAttachParams,
    ) -> Result<Self::Attach, NativeV2CliError> {
        unreachable!()
    }
    async fn run_force(
        &self,
        _: Option<&str>,
        _: RunForceParams,
    ) -> Result<CliRunForceResult, NativeV2CliError> {
        unreachable!()
    }

    async fn run_status(
        &self,
        target: Option<&str>,
        _: RunStatusParams,
    ) -> Result<CliRunStatusResult, NativeV2CliError> {
        assert_eq!(target, Some("remote"));
        let mut status = serde_json::to_value(&self.suspension).expect("suspension JSON");
        status["phase"] = json!("suspended");
        let mut status: CliRunStatusResult = serde_json::from_value(json!({"runId":"original", "title":"Request", "size":"host", "atCursor":"v2:1", "status":status})).expect("status");
        status.workspace_recovery.successor_run_id = self.successor.map(RunId::new);
        Ok(status)
    }

    async fn authorize_resume_connection_requirements(
        &self,
        _: Option<&str>,
        _: &RunId,
        requirements: openengine_cluster_protocol::RunConnectionRequirements,
    ) -> Result<openengine_cluster_protocol::RunConnectionRequirements, NativeV2CliError> {
        Ok(requirements)
    }

    async fn run_resume(
        &self,
        target: Option<&str>,
        params: RunResumeParams,
        discard: bool,
    ) -> Result<openengine_cluster_protocol::RunResumeResult, NativeV2CliError> {
        assert_eq!(target, Some("remote"));
        assert!(!discard);
        self.resumes.lock().expect("resumes").push(params.clone());
        if let Some(message) = self.refusal {
            return Err(request_error(message));
        }
        assert!(matches!(params.from, Some(RunResumeFrom::Answer { .. })));
        let resolution =
            crate::native_v2_request::resolve(&self.suspension, params.from.as_ref(), self.now)
                .map_err(request_error)?;
        let expired = matches!(resolution, RequestResolution::Expiry {});
        *self.resolution.lock().expect("resolution") = Some(resolution);
        if expired {
            return Err(request_error(format!(
                "request expired; successor {}",
                params.successor_run_id.as_str()
            )));
        }
        Ok(openengine_cluster_protocol::RunResumeResult {
            run_id: params.successor_run_id,
            resumed_from: params.run_id,
        })
    }
}

async fn answer(
    backend: &RequestBackend,
    path: std::path::PathBuf,
) -> (Result<CliOutcome, NativeV2CliError>, Vec<u8>) {
    let mut output = Vec::new();
    let result = execute_answer(
        (
            RunSelector {
                target: Some("remote".to_owned()),
                run_id: RunId::new("original"),
            },
            path,
            false,
        ),
        &CliExecutionContext::new(backend, &|_| None),
        &mut output,
    )
    .await;
    (result, output)
}

#[tokio::test]
async fn named_target_request_deadline_uses_controller_clock_and_preserves_valid_answers() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("answer.json");
    std::fs::write(&path, r#"{"signals":{"decision":"approved"},"output":{}}"#)
        .expect("answer file");
    for client_ahead in [true, false] {
        let backend = RequestBackend::new(client_ahead, false);
        let (result, output) = answer(&backend, path.clone()).await;
        assert_eq!(
            result.expect("answer accepted before controller deadline"),
            CliOutcome::Completed
        );
        let resumes = backend.resumes.lock().expect("resumes");
        assert_eq!(resumes.len(), 1);
        assert_eq!(
            serde_json::to_value(&resumes[0].from).expect("answer"),
            json!({"kind":"answer","signals":{"decision":"approved"},"output":{}})
        );
        let receipt: serde_json::Value = serde_json::from_slice(&output).expect("resume receipt");
        assert_eq!(receipt["runId"], resumes[0].successor_run_id.as_str());
        assert_eq!(receipt["resumedFrom"], "original");
        assert!(matches!(
            *backend.resolution.lock().expect("resolution"),
            Some(RequestResolution::Answer { .. })
        ));
    }
}

#[tokio::test]
async fn named_target_request_expiry_precedes_file_errors_despite_client_clock_skew() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("answer.json");
    let oversized = "x".repeat(openengine_cluster_protocol::MAX_VECTOR_RESPONSE_BYTES + 1);
    let oversized_result = json!({"signals":{"decision":"approved"},"output":{"feedback":"x".repeat(crate::v2_run_ledger::MAX_EVENT_BYTES)}}).to_string();
    for contents in [
        Some("{"),
        Some(r#"{"extra":true}"#),
        Some(r#"{"signals":{},"output":{}}"#),
        Some(oversized.as_str()),
        Some(oversized_result.as_str()),
        None,
    ] {
        if let Some(contents) = contents {
            std::fs::write(&path, contents).expect("answer file");
        } else {
            std::fs::remove_file(&path).expect("remove answer file");
        }
        for client_ahead in [true, false] {
            let backend = RequestBackend::new(client_ahead, true);
            let (result, output) = answer(&backend, path.clone()).await;
            let error = result.expect_err("expired answer");
            let resumes = backend.resumes.lock().expect("resumes");
            assert_eq!(resumes.len(), 1);
            assert!(output.is_empty());
            assert!(
                matches!(error, NativeV2CliError::Remote { ref code, ref message, .. } if code == "invalid_request" && message.starts_with("request expired; successor ") && message.contains(resumes[0].successor_run_id.as_str()))
            );
            assert!(matches!(
                *backend.resolution.lock().expect("resolution"),
                Some(RequestResolution::Expiry {})
            ));
        }
    }
}

#[tokio::test]
async fn pending_target_request_refuses_malformed_files_without_admission_and_keeps_the_path() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("answer.json");
    for (contents, offending) in [
        (Some("{"), "answer file: $: EOF while parsing an object"),
        (
            Some(r#"{"extra":true}"#),
            "answer file: $.extra is undeclared",
        ),
        (
            Some(r#"{"signals":{"decision":1},"output":{}}"#),
            "answer file: $.signals.decision must be a label string",
        ),
        (
            Some(r#"{"signals":{"decision":"approved"}}"#),
            "answer file: $.output is required",
        ),
        (None, "answer file: $: cannot read answer file: "),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&path, contents).expect("answer file");
        } else {
            std::fs::remove_file(&path).expect("remove answer file");
        }
        let backend = RequestBackend::new(false, false);
        let (result, output) = answer(&backend, path.clone()).await;
        // The placeholder's own refusal ($.signals.decision is required) is not printed.
        assert!(
            matches!(result.expect_err("invalid file"), NativeV2CliError::Remote { code, message, .. } if code == "invalid_request" && message.starts_with(offending))
        );
        assert!(backend.resolution.lock().expect("resolution").is_none());
        assert!(output.is_empty());
    }
}

#[tokio::test]
async fn named_target_request_preserves_controller_refusals() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("answer.json");
    std::fs::write(&path, r#"{"signals":{"decision":"approved"},"output":{}}"#)
        .expect("answer file");
    let mut backend = RequestBackend::new(true, false);
    backend.refusal = Some("original authorization refuses these connection values");
    let (result, _) = answer(&backend, path.clone()).await;
    assert!(
        matches!(result.expect_err("refusal"), NativeV2CliError::Remote { message, .. } if Some(message.as_str()) == backend.refusal)
    );
    std::fs::write(&path, r#"{"extra":true}"#).expect("answer file");
    backend.refusal = Some("run already has successor next");
    let (result, _) = answer(&backend, path).await;
    assert!(
        matches!(result.expect_err("refusal"), NativeV2CliError::Remote { message, .. } if message == "run already has successor next; answer file: $.extra is undeclared")
    );
}

#[tokio::test]
async fn default_resume_of_a_request_names_the_successor_before_answer() {
    for (successor, expected) in [
        (None, "run requires zeroshot answer original --input FILE"),
        (Some("next"), "run already has successor next"),
    ] {
        let mut backend = RequestBackend::new(false, false);
        backend.successor = successor;
        let mut output = Vec::new();
        let error = execute_resume(
            RunResumeCommand {
                run: RunSelector {
                    target: Some("remote".to_owned()),
                    run_id: RunId::new("original"),
                },
                from: None,
                discard_workspace_changes: false,
                forward_github_token: false,
            },
            &CliExecutionContext::new(&backend, &|_| panic!("credentials resolved")),
            &mut output,
        )
        .await
        .expect_err("resume refused");
        let (NativeV2CliError::Usage(message) | NativeV2CliError::Remote { message, .. }) = error
        else {
            panic!("unexpected refusal {error}");
        };
        assert_eq!(message, expected);
        assert!(backend.resumes.lock().expect("resumes").is_empty());
        assert!(output.is_empty());
    }
}
